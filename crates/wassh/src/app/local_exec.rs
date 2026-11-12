//! Running a host command on this machine.
//!
//! A Local tab's "host" is the machine WaSSH runs on, so a pane that probes the
//! host — the server monitor's sample, the process table's details and signals —
//! runs its shell command here instead of over SSH.
//!
//! The command goes to `/bin/sh -c` on Unix and to `powershell.exe` on Windows, with
//! the same timeout and output cap as the reference's `runHostCommand`.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use wassh_ssh::ExecOutput;

/// Longest a local command may run before it is killed (the reference's
/// `SERVICE_COMMAND_TIMEOUT_MS`).
const TIMEOUT: Duration = Duration::from_secs(120);

/// Largest amount of output kept per stream (the reference's
/// `SERVICE_COMMAND_MAX_BUFFER_BYTES`).
const MAX_OUTPUT: u64 = 4 * 1024 * 1024;

/// How often a running command is checked for exit.
const POLL: Duration = Duration::from_millis(5);

/// The Windows program that runs a command inside a WSL distribution.
#[cfg(windows)]
const WSL: &str = "wsl.exe";

/// Whether this machine can run host commands at all.
///
/// Unix gets `/bin/sh`, Windows its own PowerShell; other targets have no shell
/// this code knows how to drive.
pub fn supported() -> bool {
    cfg!(any(unix, windows))
}

/// Run `command` with `/bin/sh -c`, answering on a later frame.
pub fn exec(command: &str, input: Option<&str>) -> LocalCommand {
    run_with(&Shell::Native, command, input, TIMEOUT)
}

/// Run `command` inside a WSL distribution, through `wsl.exe`.
///
/// A WSL tab's host is the distribution, not the Windows machine under it, so a pane that
/// probes the host (Docker, the server monitor) has to reach the distribution's own
/// `docker`, `apt-get` and `/proc`.
pub fn exec_in_wsl(distro: Option<&str>, command: &str, input: Option<&str>) -> LocalCommand {
    run_with(&Shell::Wsl(distro), command, input, TIMEOUT)
}

/// Which shell one command goes through.
enum Shell<'a> {
    /// This machine's own shell.
    Native,
    /// A WSL distribution; `None` is WSL's default one.
    Wsl(Option<&'a str>),
}

/// A command running on this machine, and the handle to stop it.
pub struct LocalCommand {
    /// The answer, once the command finishes or is stopped.
    pub output: Receiver<Result<ExecOutput, String>>,
    /// Stops the command.
    pub stop: LocalStop,
}

/// Stops a command running on this machine.
#[derive(Clone)]
pub struct LocalStop(Arc<AtomicBool>);

impl LocalStop {
    /// Ask the shell to stop: it kills the child at its next check.
    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Run `command` with a deadline; the timeout is a parameter so a test need not
/// wait two minutes.
fn run_with(
    which: &Shell<'_>,
    command: &str,
    input: Option<&str>,
    timeout: Duration,
) -> LocalCommand {
    let (sender, receiver) = channel();
    let unused = sender.clone();
    let command = command.to_owned();
    let distro = match which {
        Shell::Native => None,
        Shell::Wsl(distro) => Some(distro.map(str::to_owned)),
    };
    let input = input.map(str::to_owned);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("wassh-local-command".to_owned())
        .spawn(move || {
            let shell = match &distro {
                None => Shell::Native,
                Some(distro) => Shell::Wsl(distro.as_deref()),
            };
            let _ = sender.send(run(&shell, &command, input.as_deref(), timeout, &thread_stop));
        });
    if let Err(error) = spawned {
        let _ = unused.send(Err(format!("starting the command: {error}")));
    }
    LocalCommand {
        output: receiver,
        stop: LocalStop(stop),
    }
}

/// The process that runs `command` in the chosen shell.
fn command_for(which: &Shell<'_>, command: &str) -> Result<Command, String> {
    match which {
        Shell::Native => Ok(native_shell(command)),
        Shell::Wsl(distro) => wsl_shell(*distro, command),
    }
}

/// A shell that runs `command` as one line, whichever shell the platform has.
#[cfg(unix)]
fn native_shell(command: &str) -> Command {
    let mut shell = Command::new("/bin/sh");
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn native_shell(command: &str) -> Command {
    // `-Command` takes the whole line as one script, so `;`, pipes and blocks in
    // the probe reach PowerShell exactly as written.
    let mut shell = Command::new("powershell.exe");
    shell.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        command,
    ]);
    shell
}

/// The process that runs `command` in a WSL distribution.
#[cfg(windows)]
fn wsl_shell(distro: Option<&str>, command: &str) -> Result<Command, String> {
    let mut shell = Command::new(WSL);
    if let Some(distro) = distro {
        shell.args(["--distribution", distro]);
    }
    // `--exec` hands the rest of the line to the distribution without a login shell, and
    // `/bin/sh -c` then runs the script the same way an SSH host would.
    shell.args(["--exec", "/bin/sh", "-c", command]);
    Ok(shell)
}

#[cfg(not(windows))]
fn wsl_shell(_distro: Option<&str>, _command: &str) -> Result<Command, String> {
    Err("A WSL shell only exists on Windows.".to_owned())
}

fn run(
    which: &Shell<'_>,
    command: &str,
    input: Option<&str>,
    timeout: Duration,
    stop: &AtomicBool,
) -> Result<ExecOutput, String> {
    if !supported() {
        return Err("Local host commands need a shell on this platform.".to_owned());
    }
    let mut shell = command_for(which, command)?;
    shell
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    wassh_sys::hide_console(&mut shell);
    let mut child = shell
        .spawn()
        .map_err(|error| format!("running the local shell: {error}"))?;
    // Writing stdin and dropping it closes the pipe, so a command that reads it
    // (such as `sudo -S`) finishes instead of waiting.
    if let Some(mut stdin) = child.stdin.take()
        && let Some(input) = input
    {
        let _ = stdin.write_all(input.as_bytes());
    }
    // Both pipes are drained on their own threads: a command that fills one
    // while we wait on the other would otherwise deadlock.
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if stop.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(wassh_ssh::STOPPED_COMMAND.to_owned());
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("`{command}` timed out"));
                }
                std::thread::sleep(POLL);
            }
            Err(error) => return Err(format!("waiting for the command: {error}")),
        }
    };
    Ok(ExecOutput {
        stdout: stdout.and_then(|reader| reader.join().ok()).unwrap_or_default(),
        stderr: stderr.and_then(|reader| reader.join().ok()).unwrap_or_default(),
        exit_status: status.code().map(|code| code as u32),
    })
}

/// Read one pipe to the end, keeping at most [`MAX_OUTPUT`] bytes.
fn drain<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.by_ref().take(MAX_OUTPUT).read_to_end(&mut buffer);
        String::from_utf8_lossy(&buffer).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line that prints `hello` on this platform's shell.
    #[cfg(unix)]
    const ECHO: &str = "printf 'hello'";
    #[cfg(windows)]
    const ECHO: &str = "'hello'";

    /// A line that writes `boom` to stderr and exits with 3.
    #[cfg(unix)]
    const FAIL: &str = "echo boom >&2; exit 3";
    #[cfg(windows)]
    const FAIL: &str = "Write-Error boom; exit 3";

    /// A line that echoes its stdin back.
    #[cfg(unix)]
    const STDIN: &str = "cat";
    #[cfg(windows)]
    const STDIN: &str = "[Console]::In.ReadToEnd()";

    /// A line that runs far longer than any test waits.
    #[cfg(unix)]
    const HANG: &str = "sleep 30";
    #[cfg(windows)]
    const HANG: &str = "Start-Sleep -Seconds 30";

    /// How long a test waits for its command; a busy machine spawns a shell slowly.
    const COMMAND_WAIT: Duration = Duration::from_secs(60);

    /// Collect a finished command, waiting for it like the UI does.
    fn wait(receiver: Receiver<Result<ExecOutput, String>>) -> Result<ExecOutput, String> {
        receiver
            .recv_timeout(COMMAND_WAIT)
            .expect("the command finished")
    }

    #[test]
    fn a_command_reports_its_output_and_status() {
        let output = wait(exec(ECHO, None).output).expect("output");
        assert_eq!(output.stdout.trim(), "hello");
        assert_eq!(output.exit_status, Some(0));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn a_failing_command_keeps_its_output_and_exit_code() {
        let output = wait(exec(FAIL, None).output).expect("output");
        assert_eq!(output.exit_status, Some(3));
        assert!(output.stderr.contains("boom"), "{:?}", output.stderr);
    }

    #[test]
    fn stdin_reaches_the_command() {
        // The command only finishes because stdin is closed after the write.
        let output = wait(exec(STDIN, Some("s3cret\n")).output).expect("output");
        assert_eq!(output.stdout.trim(), "s3cret");
    }

    #[test]
    fn a_running_command_can_be_stopped() {
        let started = Instant::now();
        let job = exec(HANG, None);
        std::thread::sleep(POLL);
        job.stop.stop();
        let error = wait(job.output).expect_err("a stopped command");
        assert!(error.contains("stopped"), "{error}");
        assert!(
            started.elapsed() < COMMAND_WAIT,
            "the command was not stopped"
        );
    }

    #[test]
    fn a_hung_command_is_killed_at_the_deadline() {
        let started = Instant::now();
        let error = run_with(&Shell::Native, HANG, None, Duration::from_millis(200))
            .output
            .recv_timeout(Duration::from_secs(10))
            .expect("an answer")
            .expect_err("a timeout");
        assert!(error.contains("timed out"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline was not enforced"
        );
    }
}
