//! Commands a pane can run on the host.
//!
//! The session owns the connection, so a pane only asks for a command to be run under a key
//! and reads the result of that key on a later frame.

/// Result of a command a pane asked the session to run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Exit code, when the host reported one.
    pub code: Option<u32>,
}

impl CommandOutput {
    /// Whether the command reported success (or said nothing about its status).
    pub fn succeeded(&self) -> bool {
        matches!(self.code, None | Some(0))
    }

    /// The first non-empty line of standard error, for a one-line status.
    pub fn error_line(&self) -> Option<&str> {
        self.stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
    }
}

/// Running commands on the host, handed to panes by the session.
///
/// Keys are per pane (`"docker.list"`, `"docker.action"`), so one pane can keep two questions
/// in flight without confusing their answers.
pub trait CommandAccess {
    /// Run `command` under `key`, replacing any cached result for that key.
    fn exec(&mut self, key: &str, command: &str);

    /// Run `command` with `input` on its stdin.
    ///
    /// `sudo -S` reads its password there, so the password never reaches argv.
    fn exec_input(&mut self, key: &str, command: &str, input: &str) {
        let _ = input;
        self.exec(key, command);
    }

    /// The result cached for `key` once its command finished.
    fn output(&self, key: &str) -> Option<&CommandOutput>;

    /// Why the command for `key` failed to run, or why it exited non-zero.
    fn failure(&self, key: &str) -> Option<&str>;

    /// Whether the command for `key` is still running.
    fn is_running(&self, key: &str) -> bool;
}

/// TCP, forwarded through the session.
///
/// A pane that needs a plain TCP client — the MQTT analyser talking to a broker
/// on the remote host — asks for a loopback forward and dials that instead, so
/// the bytes travel over SSH exactly like the reference's `openTcpStream`.
pub trait TcpAccess {
    /// Ask for a loopback forward to `host:port` (as the remote host sees it),
    /// under `key`.
    fn open_forward(&mut self, key: &str, host: &str, port: u16);

    /// The local address to dial once the forward is up, e.g. `127.0.0.1:41234`.
    fn forward_address(&self, key: &str) -> Option<&str>;

    /// Why the forward could not be opened.
    fn forward_error(&self, key: &str) -> Option<&str>;

    /// Whether the request is still in flight.
    fn forward_pending(&self, key: &str) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_and_failure_are_told_apart() {
        let ok = CommandOutput {
            stdout: "fine".to_owned(),
            stderr: String::new(),
            code: Some(0),
        };
        assert!(ok.succeeded());
        assert_eq!(ok.error_line(), None);

        let failed = CommandOutput {
            stdout: String::new(),
            stderr: "\n  permission denied\nmore\n".to_owned(),
            code: Some(1),
        };
        assert!(!failed.succeeded());
        assert_eq!(failed.error_line(), Some("permission denied"));

        // No exit status at all counts as "did not complain".
        assert!(CommandOutput::default().succeeded());
    }
}
