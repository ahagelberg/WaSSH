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

/// Whether this machine can run host commands at all.
///
/// Unix gets `/bin/sh`, Windows its own PowerShell; other targets have no shell
/// this code knows how to drive.
pub fn supported() -> bool {
    cfg!(any(unix, windows))
}

/// Run `command` with `/bin/sh -c`, answering on a later frame.
pub fn exec(command: &str, input: Option<&str>) -> Receiver<Result<ExecOutput, String>> {
    run_with(command, input, TIMEOUT)
}

/// Run `command` with a deadline; the timeout is a parameter so a test need not
/// wait two minutes.
fn run_with(
    command: &str,
    input: Option<&str>,
    timeout: Duration,
) -> Receiver<Result<ExecOutput, String>> {
    let (sender, receiver) = channel();
    let unused = sender.clone();
    let command = command.to_owned();
    let input = input.map(str::to_owned);
    let spawned = std::thread::Builder::new()
        .name("wassh-local-command".to_owned())
        .spawn(move || {
            let _ = sender.send(run(&command, input.as_deref(), timeout));
        });
    if let Err(error) = spawned {
        let _ = unused.send(Err(format!("starting the command: {error}")));
    }
    receiver
}

/// A shell that runs `command` as one line, whichever shell the platform has.
#[cfg(unix)]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("/bin/sh");
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn shell(command: &str) -> Command {
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

fn run(command: &str, input: Option<&str>, timeout: Duration) -> Result<ExecOutput, String> {
    if !supported() {
        return Err("Local host commands need a shell on this platform.".to_owned());
    }
    let mut child = shell(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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

    /// Collect a finished command, waiting for it like the UI does.
    fn wait(receiver: Receiver<Result<ExecOutput, String>>) -> Result<ExecOutput, String> {
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the command finished")
    }

    #[test]
    fn a_command_reports_its_output_and_status() {
        let output = wait(exec(ECHO, None)).expect("output");
        assert_eq!(output.stdout.trim(), "hello");
        assert_eq!(output.exit_status, Some(0));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn a_failing_command_keeps_its_output_and_exit_code() {
        let output = wait(exec(FAIL, None)).expect("output");
        assert_eq!(output.exit_status, Some(3));
        assert!(output.stderr.contains("boom"), "{:?}", output.stderr);
    }

    #[test]
    fn stdin_reaches_the_command() {
        // The command only finishes because stdin is closed after the write.
        let output = wait(exec(STDIN, Some("s3cret\n"))).expect("output");
        assert_eq!(output.stdout.trim(), "s3cret");
    }

    #[test]
    fn a_hung_command_is_killed_at_the_deadline() {
        let started = Instant::now();
        let error = run_with("sleep 30", None, Duration::from_millis(200))
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
