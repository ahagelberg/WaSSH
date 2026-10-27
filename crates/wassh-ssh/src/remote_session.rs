//! tmux/screen attach planning.
//!
//! The remote-session flow: probe first, then either attach
//! to an existing session, create one, or fall back to a plain login shell. The rules are
//! pure so they can be unit-tested without a server.

use wassh_core::{BusyPolicy, Multiplexer};

/// What the probe found on the remote host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presence {
    /// The named session does not exist yet.
    #[default]
    Missing,
    /// The session exists and a client is attached.
    Attached,
    /// The session exists but nobody is attached.
    Detached,
    /// The probe ran but produced output we could not parse.
    Unknown,
    /// tmux/screen is not installed.
    Unavailable,
}

/// What to run instead of a plain login shell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemotePlan {
    /// Command to execute; `None` means "open a normal interactive shell".
    pub command: Option<String>,
    /// Note shown to the user, e.g. why the fallback happened.
    pub message: Option<String>,
}

/// Command used to look for existing sessions.
pub fn probe_command(kind: Multiplexer) -> &'static str {
    match kind {
        Multiplexer::Tmux => "tmux list-sessions",
        Multiplexer::Screen => "screen -ls",
    }
}

/// Name of the multiplexer as shown to users.
pub fn multiplexer_label(kind: Multiplexer) -> &'static str {
    match kind {
        Multiplexer::Tmux => "tmux",
        Multiplexer::Screen => "screen",
    }
}

/// Quote a value for a POSIX shell (single quotes, `'` escaped as `'\''`).
pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Interpret the output of [`probe_command`].
pub fn classify(
    kind: Multiplexer,
    session: &str,
    exit_status: Option<u32>,
    stdout: &str,
    stderr: &str,
) -> Presence {
    if exit_status == Some(127) {
        return Presence::Unavailable;
    }
    if let Some(found) = find_session(kind, session, stdout) {
        return found;
    }
    // tmux exits non-zero with "no server running" when nothing exists yet; screen
    // prints "No Sockets found". Both mean the session is simply missing.
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if combined.contains("no server running")
        || combined.contains("no sockets found")
        || combined.contains("error connecting to")
        || stdout.trim().is_empty()
    {
        return Presence::Missing;
    }
    if stderr.to_ascii_lowercase().contains("command not found")
        || stderr.to_ascii_lowercase().contains("not found")
    {
        return Presence::Unavailable;
    }
    Presence::Unknown
}

fn find_session(kind: Multiplexer, session: &str, stdout: &str) -> Option<Presence> {
    match kind {
        Multiplexer::Tmux => stdout.lines().find_map(|line| {
            let (name, rest) = line.split_once(':')?;
            if name.trim() != session {
                return None;
            }
            Some(if rest.to_ascii_lowercase().contains("(attached)") {
                Presence::Attached
            } else {
                Presence::Detached
            })
        }),
        Multiplexer::Screen => stdout.lines().find_map(|line| {
            let trimmed = line.trim_start();
            let (id, rest) = trimmed.split_once('.')?;
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let name = rest.split_whitespace().next()?;
            if name != session {
                return None;
            }
            Some(if rest.to_ascii_lowercase().contains("(attached)") {
                Presence::Attached
            } else {
                Presence::Detached
            })
        }),
    }
}

/// The command that attaches to (or creates) `session`.
pub fn attach_command(kind: Multiplexer, session: &str, busy: BusyPolicy) -> String {
    let quoted = shell_single_quote(session);
    match (kind, busy) {
        (Multiplexer::Tmux, BusyPolicy::Share) => {
            format!("tmux new-session -A -s {quoted}")
        }
        (Multiplexer::Tmux, BusyPolicy::ForceDetach) => {
            format!("tmux new-session -A -D -s {quoted}")
        }
        (Multiplexer::Tmux, BusyPolicy::DoNotAttach) => {
            format!("tmux new-session -s {quoted}")
        }
        (Multiplexer::Screen, BusyPolicy::Share) => format!("screen -S {quoted} -xR"),
        (Multiplexer::Screen, BusyPolicy::ForceDetach) => {
            format!("screen -S {quoted} -d -RR")
        }
        (Multiplexer::Screen, BusyPolicy::DoNotAttach) => format!("screen -S {quoted}"),
    }
}

/// Command that attaches to an existing session without creating one.
pub fn reattach_command(kind: Multiplexer, session: &str) -> String {
    let quoted = shell_single_quote(session);
    match kind {
        Multiplexer::Tmux => format!("tmux attach-session -t {quoted}"),
        Multiplexer::Screen => format!("screen -S {quoted} -r"),
    }
}

/// Decide what to run, given the probe result.
pub fn plan(
    kind: Multiplexer,
    session: &str,
    busy: BusyPolicy,
    presence: Presence,
) -> RemotePlan {
    if presence == Presence::Unavailable {
        return RemotePlan {
            command: None,
            message: Some(format!(
                "{} is not installed on the remote host; opening a plain shell.",
                multiplexer_label(kind)
            )),
        };
    }

    if matches!(busy, BusyPolicy::Share | BusyPolicy::ForceDetach) {
        return RemotePlan {
            command: Some(attach_command(kind, session, busy)),
            message: None,
        };
    }

    match presence {
        Presence::Attached => RemotePlan {
            command: None,
            message: Some(format!(
                "{} session '{session}' is already attached elsewhere; opening a plain shell.",
                multiplexer_label(kind)
            )),
        },
        Presence::Detached => RemotePlan {
            command: Some(reattach_command(kind, session)),
            message: None,
        },
        Presence::Missing | Presence::Unknown => RemotePlan {
            command: Some(attach_command(kind, session, BusyPolicy::DoNotAttach)),
            message: None,
        },
        Presence::Unavailable => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_probe_detects_states() {
        let list = "main: 2 windows (created Mon Oct 20 10:00:00 2026)\nother: 1 windows (created Mon Oct 20 10:00:00 2026) (attached)";
        assert_eq!(
            classify(Multiplexer::Tmux, "main", Some(0), list, ""),
            Presence::Detached
        );
        assert_eq!(
            classify(Multiplexer::Tmux, "other", Some(0), list, ""),
            Presence::Attached
        );
        assert_eq!(
            classify(Multiplexer::Tmux, "missing", Some(0), list, ""),
            Presence::Unknown
        );
    }

    #[test]
    fn screen_probe_detects_states() {
        let list = "\t12345.main\t(Detached)\n\t23456.work\t(Attached)";
        assert_eq!(
            classify(Multiplexer::Screen, "main", Some(0), list, ""),
            Presence::Detached
        );
        assert_eq!(
            classify(Multiplexer::Screen, "work", Some(0), list, ""),
            Presence::Attached
        );
    }

    #[test]
    fn missing_sessions_and_missing_tools_are_distinguished() {
        assert_eq!(
            classify(
                Multiplexer::Tmux,
                "main",
                Some(1),
                "",
                "no server running on /tmp/tmux-1000/default"
            ),
            Presence::Missing
        );
        assert_eq!(
            classify(
                Multiplexer::Tmux,
                "main",
                Some(127),
                "",
                "bash: line 1: tmux: command not found"
            ),
            Presence::Unavailable
        );
        assert_eq!(
            classify(
                Multiplexer::Screen,
                "main",
                Some(1),
                "No Sockets found in /run/screen/S-alice.\n",
                ""
            ),
            Presence::Missing
        );
    }

    #[test]
    fn attach_commands_match_the_electron_build() {
        assert_eq!(
            attach_command(Multiplexer::Tmux, "main", BusyPolicy::Share),
            "tmux new-session -A -s 'main'"
        );
        assert_eq!(
            attach_command(Multiplexer::Tmux, "main", BusyPolicy::ForceDetach),
            "tmux new-session -A -D -s 'main'"
        );
        assert_eq!(
            attach_command(Multiplexer::Tmux, "main", BusyPolicy::DoNotAttach),
            "tmux new-session -s 'main'"
        );
        assert_eq!(
            attach_command(Multiplexer::Screen, "main", BusyPolicy::Share),
            "screen -S 'main' -xR"
        );
        assert_eq!(
            attach_command(Multiplexer::Screen, "main", BusyPolicy::ForceDetach),
            "screen -S 'main' -d -RR"
        );
        assert_eq!(
            reattach_command(Multiplexer::Tmux, "main"),
            "tmux attach-session -t 'main'"
        );
        assert_eq!(
            reattach_command(Multiplexer::Screen, "main"),
            "screen -S 'main' -r"
        );
    }

    #[test]
    fn quotes_are_escaped_for_the_shell() {
        assert_eq!(shell_single_quote("it's"), r"'it'\''s'");
        assert_eq!(
            attach_command(Multiplexer::Tmux, "it's", BusyPolicy::Share),
            r"tmux new-session -A -s 'it'\''s'"
        );
    }

    #[test]
    fn plan_falls_back_with_a_message_when_the_tool_is_missing() {
        let plan = plan(
            Multiplexer::Tmux,
            "main",
            BusyPolicy::Share,
            Presence::Unavailable,
        );
        assert!(plan.command.is_none());
        assert!(plan.message.unwrap().contains("not installed"));
    }

    #[test]
    fn plan_respects_busy_handling() {
        let shared = plan(
            Multiplexer::Tmux,
            "main",
            BusyPolicy::Share,
            Presence::Attached,
        );
        assert_eq!(
            shared.command.as_deref(),
            Some("tmux new-session -A -s 'main'")
        );

        let cautious = plan(
            Multiplexer::Tmux,
            "main",
            BusyPolicy::DoNotAttach,
            Presence::Attached,
        );
        assert!(cautious.command.is_none());
        assert!(cautious.message.unwrap().contains("already attached"));

        let detached = plan(
            Multiplexer::Tmux,
            "main",
            BusyPolicy::DoNotAttach,
            Presence::Detached,
        );
        assert_eq!(
            detached.command.as_deref(),
            Some("tmux attach-session -t 'main'")
        );

        let fresh = plan(
            Multiplexer::Screen,
            "main",
            BusyPolicy::DoNotAttach,
            Presence::Missing,
        );
        assert_eq!(fresh.command.as_deref(), Some("screen -S 'main'"));
    }
}
