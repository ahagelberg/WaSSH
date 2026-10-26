//! Running a host command on this machine.
//!
//! A Local tab's "host" is the machine WaSSH runs on, so a pane that probes the
//! host — the server monitor's sample, the process table's details and signals —
//! runs its shell command here instead of over SSH.
//!
//! This is the reference's `runHostCommand`: `execFile('/bin/sh', ['-c',
//! command], { timeout, maxBuffer })`, with the same timeout and output cap. The
//! command runs on its own thread and answers on a later frame, so the window
//! never waits for it.

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
/// The reference restricts local host commands to Linux, macOS and FreeBSD;
/// elsewhere it refuses rather than guessing at a shell.
pub fn supported() -> bool {
    cfg!(unix)
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

fn run(command: &str, input: Option<&str>, timeout: Duration) -> Result<ExecOutput, String> {
    if !supported() {
        return Err("Local host commands need a Unix shell.".to_owned());
    }
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
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

    /// Collect a finished command, waiting for it like the UI does.
    fn wait(receiver: Receiver<Result<ExecOutput, String>>) -> Result<ExecOutput, String> {
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the command finished")
    }

    #[test]
    fn a_command_reports_its_output_and_status() {
        let output = wait(exec("printf 'hello'", None)).expect("output");
        assert_eq!(output.stdout, "hello");
        assert_eq!(output.exit_status, Some(0));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn a_failing_command_keeps_its_output_and_exit_code() {
        let output = wait(exec("echo boom >&2; exit 3", None)).expect("output");
        assert_eq!(output.exit_status, Some(3));
        assert_eq!(output.stderr.trim(), "boom");
    }

    #[test]
    fn stdin_reaches_the_command() {
        // `cat` only finishes because stdin is closed after the write.
        let output = wait(exec("cat", Some("s3cret\n"))).expect("output");
        assert_eq!(output.stdout, "s3cret\n");
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

    #[test]
    fn the_monitor_probe_runs_here() {
        // The very command the Server Monitor sends to an SSH host also works on
        // this one, which is what makes a Local tab sample itself.
        let output = wait(exec(
            "cat /proc/loadavg; echo @@; cat /proc/uptime",
            None,
        ))
        .expect("output");
        assert!(
            output.stdout.contains("@@"),
            "the probe printed {:?}",
            output.stdout
        );
        assert_eq!(output.exit_status, Some(0));
    }
}
