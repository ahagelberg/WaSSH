//! What the agent may do without asking.
//!
//! Three questions decide one action, in this order. A protected path is refused outright:
//! nothing the user saved and nothing the model says opens `~/.ssh` to a read or a write.
//! The user's rules come next — host before app, deny before allow — and they are matched
//! against the command as it was typed, because that is what the card showed when they were
//! saved. What is left falls to the zones and the shipped rules: a command that cannot be
//! parsed always asks, a write outside the working directory asks, a known-safe command runs.
//!
//! The split exists because a glob cannot see what a shell does. `git status*` matches
//! `git status && rm -rf /`, so an allow is only trusted once the whole line parses into
//! simple commands whose paths zone clean; a substitution, a heredoc or an interpreter given
//! `-c` is not proof of anything and asks instead.

use serde_json::Value;

use super::protocol::{
    TOOL_DEV_DIFF, TOOL_DEV_FIND, TOOL_DEV_GREP, TOOL_DEV_IMAGE, TOOL_DEV_LIST, TOOL_DEV_VIEW,
    TOOL_LOCAL_GREP, TOOL_LOCAL_LIST, TOOL_LOCAL_READ, TOOL_REMOTE_LIST, TOOL_REMOTE_READ,
    TOOL_REMOTE_STAT, TOOL_REMOTE_ZIP_LIST, TOOL_REMOTE_ZIP_READ,
};
use super::rules::{self, RuleDecision};

/// Where a path lies, by how much care it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// The connected host's working directory.
    Work,
    /// The chat's scratch directory, emptied between steps.
    Scratch,
    /// This computer's home directory.
    Home,
    /// System directories: `/etc`, `/usr`, `/var` and the like.
    System,
    /// Keys, credentials and environment files.
    Secrets,
    /// Anywhere else.
    Other,
}

/// A decision with the reason that made it, for the card and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// What to do.
    pub decision: RuleDecision,
    /// One sentence the user reads on the card.
    pub reason: String,
}

impl Verdict {
    fn allow(reason: impl Into<String>) -> Self {
        Self {
            decision: RuleDecision::Allow,
            reason: reason.into(),
        }
    }

    fn ask(reason: impl Into<String>) -> Self {
        Self {
            decision: RuleDecision::Ask,
            reason: reason.into(),
        }
    }

    fn deny(reason: impl Into<String>) -> Self {
        Self {
            decision: RuleDecision::Deny,
            reason: reason.into(),
        }
    }
}

/// Directory prefixes whose contents are never read or written by the agent.
const SECRET_DIRS: &[&str] = &[
    "~/.ssh",
    "~/.gnupg",
    "~/.aws",
    "~/.azure",
    "~/.kube",
    "~/.config/gh",
    "~/.docker",
    "~/.password-store",
];

/// File names that hold credentials wherever they sit.
const SECRET_FILES: &[&str] = &[
    ".env",
    ".netrc",
    ".npmrc",
    ".pgpass",
    ".git-credentials",
    "credentials",
    "shadow",
    "gshadow",
    "sudoers",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "authorized_keys",
];

/// System directories a write never reaches without asking, and a write is refused for now.
const SYSTEM_DIRS: &[&str] = &[
    "/etc", "/usr", "/bin", "/sbin", "/boot", "/lib", "/lib64", "/opt", "/proc", "/sys", "/dev",
    "/var",
];

/// Binaries that read only, so a command made of them is safe once its paths zone clean.
const READ_ONLY_BINS: &[&str] = &[
    "pwd", "whoami", "id", "hostname", "uname", "date", "uptime", "lscpu", "free", "df", "du",
    "ps", "ls", "stat", "wc", "head", "tail", "cat", "file", "strings", "which", "type", "env",
    "printenv", "echo", "sort", "uniq", "cut", "tr", "jq", "diff", "cmp", "grep", "rg", "basename",
    "dirname", "realpath", "readlink", "tree", "less", "true", "false",
];

/// Git subcommands that never change the tree or the remote.
const GIT_READ_ONLY: &[&str] = &[
    "status",
    "log",
    "diff",
    "branch",
    "remote",
    "show",
    "rev-parse",
    "blame",
    "describe",
    "ls-files",
    "grep",
    "tag",
    "config",
];

/// Git subcommands that change files, so their paths are checked like any other write.
const GIT_WRITES: &[&str] = &[
    "checkout", "restore", "clean", "reset", "stash", "apply", "merge",
];

/// Binaries whose path arguments are written; the rest of a segment may also read.
const MUTATING_BINS: &[&str] = &[
    "rm", "rmdir", "mv", "cp", "install", "touch", "mkdir", "ln", "chmod", "chown", "chgrp", "dd",
    "tee", "truncate", "shred", "rsync", "unzip", "tar", "gunzip", "gzip", "patch", "sed", "perl",
    "python", "python3", "awk",
];

/// Interpreters and wrappers whose argv says nothing about what runs.
const OPAQUE_BINS: &[&str] = &[
    "sh", "bash", "zsh", "ksh", "dash", "fish", "eval", "source", "exec", "xargs", "env",
];

/// One simple command of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Segment {
    /// Its words, as a shell would pass them.
    words: Vec<String>,
    /// Whether it is the target of `>` or `>>`, which is a write.
    redirect: bool,
}

/// The policy one conversation applies: the user's rules and where its paths may go.
#[derive(Clone, Debug, Default)]
pub struct Policy<'a> {
    /// The working directory the connected host's relative paths resolve against.
    pub cwd: &'a str,
    /// This computer's home directory, which `~` stands for.
    pub home: &'a str,
    /// The scratch directory the chat may write freely in.
    pub scratch: &'a str,
    /// Rules saved for this host.
    pub host_allow: &'a [String],
    /// Rules that refuse commands on this host.
    pub host_deny: &'a [String],
    /// App-wide allow rules.
    pub app_allow: &'a [String],
    /// App-wide deny rules.
    pub app_deny: &'a [String],
    /// Answers given this session.
    pub session_allow: &'a [String],
    /// Refusals given this session.
    pub session_deny: &'a [String],
}

impl Policy<'_> {
    /// What to do with `command`.
    pub fn command(&self, command: &str) -> Verdict {
        let deny: Vec<String> = self
            .session_deny
            .iter()
            .chain(self.host_deny)
            .chain(self.app_deny)
            .cloned()
            .collect();
        if rules::matches_any(Some(&deny), command) {
            return Verdict::deny("Your rules refuse this command.");
        }

        let segments = match parse(command) {
            Ok(segments) => segments,
            Err(why) => return self.unreadable(command, why),
        };
        if let Some(why) = opaque(&segments) {
            return self.unreadable(command, why);
        }

        // Paths first: a rule that allows everything still does not open `~/.ssh`, and a glob
        // cannot see the `rm` hidden behind `&&`.
        for segment in &segments {
            if let Some(verdict) = self.segment_paths(segment) {
                return verdict;
            }
        }

        // A rule the user saved for this exact line covers all of it; anything looser is
        // matched one command at a time, so `git status*` cannot cover `&& rm -rf`.
        let allow: Vec<String> = self
            .session_allow
            .iter()
            .chain(self.host_allow)
            .cloned()
            .collect();
        if self.saved_exactly(command) {
            return Verdict::allow("You allowed this exact command.");
        }
        if segments
            .iter()
            .all(|segment| rules::matches_any(Some(&allow), &segment.words.join(" ")))
        {
            return Verdict::allow("A rule you saved allows this command.");
        }
        if segments
            .iter()
            .all(|segment| rules::matches_any(Some(self.app_allow), &segment.words.join(" ")))
        {
            return Verdict::allow("A default rule allows this command.");
        }
        if segments
            .iter()
            .filter(|segment| !segment.redirect)
            .all(is_safe)
        {
            return Verdict::allow("A command that only reads.");
        }
        Verdict::ask("No rule covers this command.")
    }

    /// The verdict for a line the harness cannot read: only the exact text the user saved
    /// opens it, because a looser rule would be a promise about a command nobody has seen.
    fn unreadable(&self, command: &str, why: &str) -> Verdict {
        if self.saved_exactly(command) {
            Verdict::allow("You allowed this exact command.")
        } else {
            Verdict::ask(format!("This command has {why}, so it cannot be checked."))
        }
    }

    /// Whether the user saved a rule that is this line, character for character.
    fn saved_exactly(&self, command: &str) -> bool {
        self.session_allow
            .iter()
            .chain(self.host_allow)
            .chain(self.app_allow)
            .any(|rule| rule.trim() == command.trim())
    }

    /// What to do with a path a tool wants to touch.
    ///
    /// `base` is what a relative path resolves against: the host's working directory for the
    /// remote tools, the user's home for the ones that run here.
    pub fn path(&self, path: &str, base: &str, writes: bool) -> Verdict {
        let resolved = resolve(&expand(path, self.home), base);
        match zone(&resolved, self.cwd, self.scratch, self.home) {
            Zone::Secrets => Verdict::deny(format!("{resolved} holds credentials.")),
            Zone::System if writes => Verdict::deny(format!("{resolved} is a system location.")),
            Zone::Work | Zone::Scratch => {
                Verdict::allow(format!("{resolved} is in the working tree."))
            }
            Zone::Home | Zone::Other | Zone::System => {
                if writes {
                    Verdict::ask(format!("{resolved} is outside the working directory."))
                } else {
                    Verdict::allow("A read.")
                }
            }
        }
    }

    /// The path verdict for one simple command, when it carries a path worth checking.
    fn segment_paths(&self, segment: &Segment) -> Option<Verdict> {
        let binary = segment.words.first()?.rsplit('/').next()?.to_owned();
        let mut writes = segment.redirect;
        if MUTATING_BINS.contains(&binary.as_str()) {
            writes = true;
        }
        if binary == "sed" && !segment.words.iter().any(|word| word.starts_with("-i")) {
            writes = false;
        }
        if binary == "git" {
            let subcommand = segment.words.get(1).map_or("", String::as_str);
            writes = GIT_WRITES.contains(&subcommand);
        }
        let mut worst: Option<Verdict> = None;
        for word in &segment.words {
            let Some(candidate) = path_word(word) else {
                continue;
            };
            let verdict = self.path(candidate, self.cwd, writes);
            match verdict.decision {
                RuleDecision::Deny => return Some(verdict),
                RuleDecision::Ask => {
                    // A read of `sed`'s expression is not a path; only a write asks here, and
                    // the argv already said whether this segment writes.
                    if writes {
                        worst = Some(verdict);
                    }
                }
                RuleDecision::Allow => {}
            }
        }
        worst
    }
}

/// The reason a line cannot be read, when one of its commands is a wrapper or an interpreter
/// that decides at run time what actually runs.
fn opaque(segments: &[Segment]) -> Option<&'static str> {
    segments
        .iter()
        .filter_map(|segment| segment.words.first())
        .map(|word| word.rsplit('/').next().unwrap_or(word))
        .find(|binary| OPAQUE_BINS.contains(binary))
        .map(|_| "a wrapper or an interpreter, whose arguments say nothing about what runs")
}

/// Where a tool's relative paths start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// The connected host's working directory.
    Host,
    /// This computer.
    ThisComputer,
}

/// What a tool does with a path it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Touch {
    /// Reads it.
    Read,
    /// Creates, replaces or removes it.
    Write,
}

/// Tools that only read the path they are given.
///
/// One that is not named here is taken to write it: a tool nobody classified is treated as
/// dangerous, not as harmless.
const READING_TOOLS: &[&str] = &[
    TOOL_LOCAL_READ,
    TOOL_LOCAL_LIST,
    TOOL_LOCAL_GREP,
    TOOL_REMOTE_READ,
    TOOL_REMOTE_LIST,
    TOOL_REMOTE_STAT,
    TOOL_REMOTE_ZIP_LIST,
    TOOL_REMOTE_ZIP_READ,
    TOOL_DEV_VIEW,
    TOOL_DEV_LIST,
    TOOL_DEV_GREP,
    TOOL_DEV_FIND,
    TOOL_DEV_DIFF,
    TOOL_DEV_IMAGE,
];

/// The paths a call names, how the tool treats each one, and where its relatives start.
///
/// Empty for a tool that works on no path: a command is judged by its argv, a key press by
/// nothing at all.
pub fn tool_paths<'a>(tool: &str, arguments: &'a Value) -> Vec<(&'a str, Touch, Base)> {
    let base = if tool.starts_with("local_fs_") || tool.starts_with("env_") {
        Base::ThisComputer
    } else if tool.starts_with("remote_fs_") || tool.starts_with("dev_") {
        Base::Host
    } else {
        return Vec::new();
    };
    let touch = if READING_TOOLS.contains(&tool) {
        Touch::Read
    } else {
        Touch::Write
    };
    ["path", "destination"]
        .into_iter()
        .filter_map(|argument| {
            let path = arguments[argument].as_str()?.trim();
            (!path.is_empty()).then_some((path, touch, base))
        })
        .collect()
}

/// `path` with `~` written out as the home directory `home`, when it is known.
fn expand(path: &str, home: &str) -> String {
    let path = path.trim();
    if home.is_empty() {
        return path.to_owned();
    }
    match path {
        "~" => home.to_owned(),
        _ => match path.strip_prefix("~/") {
            Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
            None => path.to_owned(),
        },
    }
}

/// Whether a segment is a command that cannot change anything.
fn is_safe(segment: &Segment) -> bool {
    if segment.redirect {
        return false;
    }
    let Some(binary) = segment
        .words
        .first()
        .and_then(|word| word.rsplit('/').next())
    else {
        return false;
    };
    if binary == "git" {
        return segment
            .words
            .get(1)
            .is_some_and(|subcommand| GIT_READ_ONLY.contains(&subcommand.as_str()));
    }
    if binary == "find" {
        // `find` reads until it is told to delete or run something.
        return !segment
            .words
            .iter()
            .any(|word| word == "-delete" || word == "-exec" || word == "-execdir");
    }
    READ_ONLY_BINS.contains(&binary)
}

/// The path one word names, when it names one.
fn path_word(word: &str) -> Option<&str> {
    let word = word.trim_matches(|c| c == '"' || c == '\'');
    if let Some(rest) = word.split_once('=').map(|(_, rest)| rest)
        && looks_like_path(rest)
    {
        return Some(rest);
    }
    if word.starts_with('-') || word.is_empty() {
        return None;
    }
    looks_like_path(word).then_some(word)
}

fn looks_like_path(word: &str) -> bool {
    word.starts_with('/')
        || word.starts_with("~/")
        || word.starts_with("./")
        || word.starts_with("../")
        || word.contains('/')
}

/// Where `path` lies once resolved against `base`.
fn zone(path: &str, work: &str, scratch: &str, home: &str) -> Zone {
    let resolved = resolve(path, "");
    if SECRET_DIRS.iter().any(|dir| within(&resolved, dir, home)) {
        return Zone::Secrets;
    }
    let name = resolved.rsplit('/').next().unwrap_or("");
    if SECRET_FILES.contains(&name)
        || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
    {
        return Zone::Secrets;
    }
    if resolved.starts_with("~/") || resolved == "~" {
        return Zone::Home;
    }
    if resolved == "/"
        || SYSTEM_DIRS
            .iter()
            .any(|dir| resolved == **dir || resolved.starts_with(&format!("{dir}/")))
    {
        return Zone::System;
    }
    let work = resolve(work, "");
    if !work.is_empty() && (resolved == work || resolved.starts_with(&format!("{work}/"))) {
        return Zone::Work;
    }
    let scratch = resolve(scratch, "");
    if !scratch.is_empty() && (resolved == scratch || resolved.starts_with(&format!("{scratch}/")))
    {
        return Zone::Scratch;
    }
    if resolved.starts_with('/') {
        Zone::Other
    } else {
        Zone::Home
    }
}

/// Whether `path` is `dir`, or below it, with `~` spelled out as `home` when it is known.
fn within(path: &str, dir: &str, home: &str) -> bool {
    [dir.to_owned(), expand(dir, home)]
        .iter()
        .any(|dir| path == dir.trim_end_matches('/') || path.starts_with(&format!("{dir}/")))
}

/// `path` made absolute-ish: `~` kept as the home marker, `..` folded away, `base` joined
/// onto a relative path. No filesystem is touched, so a symlink is not followed.
pub fn resolve(path: &str, base: &str) -> String {
    let path = path.trim();
    let joined = if path.starts_with('/') || path.starts_with('~') {
        path.to_owned()
    } else if base.is_empty() {
        path.to_owned()
    } else {
        format!("{}/{path}", base.trim_end_matches('/'))
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if !parts.is_empty() && *parts.last().unwrap_or(&"") != "~" {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    if joined.starts_with('/') {
        format!("/{}", parts.join("/"))
    } else {
        parts.join("/")
    }
}

/// Parse `command` into simple commands, or say why it cannot be read.
///
/// Refused shapes are the ones whose argv says nothing about what runs: a substitution, a
/// heredoc, an interpreter handed `-c`, a wrapper that builds its own argv.
fn parse(command: &str) -> Result<Vec<Segment>, &'static str> {
    let mut segments = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut redirect = false;
    let mut chars = command.chars().peekable();

    fn flush(words: &mut Vec<String>, current: &mut String) {
        if !current.is_empty() {
            words.push(std::mem::take(current));
        }
    }
    fn end_segment(words: &mut Vec<String>, redirect: &mut bool, segments: &mut Vec<Segment>) {
        if !words.is_empty() {
            segments.push(Segment {
                words: std::mem::take(words),
                redirect: *redirect,
            });
        }
        *redirect = false;
    }

    while let Some(character) = chars.next() {
        match character {
            '\'' => {
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        closed = true;
                        break;
                    }
                    current.push(inner);
                }
                if !closed {
                    return Err("an unterminated quote");
                }
            }
            '"' => {
                let mut closed = false;
                while let Some(inner) = chars.next() {
                    match inner {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => {
                            if let Some(escaped) = chars.next() {
                                current.push(escaped);
                            }
                        }
                        '$' if chars.peek() == Some(&'(') => return Err("a command substitution"),
                        '`' => return Err("a command substitution"),
                        other => current.push(other),
                    }
                }
                if !closed {
                    return Err("an unterminated quote");
                }
            }
            '\\' => {
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            }
            '$' if chars.peek() == Some(&'(') => return Err("a command substitution"),
            '`' => return Err("a command substitution"),
            '<' | '>' => {
                if character == '<' && chars.peek() == Some(&'<') {
                    return Err("a heredoc");
                }
                flush(&mut words, &mut current);
                end_segment(&mut words, &mut redirect, &mut segments);
                if chars.peek() == Some(&character) {
                    chars.next();
                }
                if character == '>' {
                    redirect = true;
                }
                // The `-d` of `2>` is part of the `2`, already flushed as a word; drop it.
                if let Some(last) = words.last()
                    && last.chars().all(|c| c.is_ascii_digit())
                {
                    words.pop();
                }
            }
            '&' | '|' | ';' | '\n' => {
                flush(&mut words, &mut current);
                end_segment(&mut words, &mut redirect, &mut segments);
                if (character == '&' || character == '|') && chars.peek() == Some(&character) {
                    chars.next();
                }
            }
            other if other.is_whitespace() => {
                flush(&mut words, &mut current);
            }
            other => current.push(other),
        }
    }
    flush(&mut words, &mut current);
    end_segment(&mut words, &mut redirect, &mut segments);
    if segments.is_empty() {
        return Err("nothing to run");
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_agent::protocol::{
        TOOL_DEV_EDIT, TOOL_LOCAL_MOVE, TOOL_LOCAL_WRITE, TOOL_RUN_COMMAND,
    };
    use crate::ai_agent::rules::decide_command;

    fn policy<'a>(allow: &'a [String], deny: &'a [String], cwd: &'a str) -> Policy<'a> {
        Policy {
            cwd,
            home: "",
            scratch: "",
            app_allow: allow,
            app_deny: deny,
            ..Policy::default()
        }
    }

    #[test]
    fn a_glob_allow_stops_at_the_next_command() {
        let allow = vec!["git status*".to_owned()];
        let p = policy(&allow, &[], "/srv/app");
        assert_eq!(
            p.command("git status --short").decision,
            RuleDecision::Allow
        );
        // The old engine allowed this, because a glob covers `&& rm -rf /`.
        assert_eq!(
            decide_command(
                "git status && rm -rf /",
                None,
                None,
                Some(&allow),
                None,
                &[],
                &[]
            ),
            RuleDecision::Allow
        );
        assert_ne!(
            p.command("git status && rm -rf /").decision,
            RuleDecision::Allow
        );
    }

    #[test]
    fn a_protected_path_is_refused_even_when_everything_is_allowed() {
        let allow = vec!["*".to_owned()];
        let p = policy(&allow, &[], "/srv/app");
        assert_eq!(
            p.path("~/.ssh/id_ed25519", "", true).decision,
            RuleDecision::Deny
        );
        assert_eq!(
            p.path("/srv/app/.env", "/srv/app", true).decision,
            RuleDecision::Deny
        );
        assert_eq!(
            p.path("/etc/shadow", "/", false).decision,
            RuleDecision::Deny
        );
        assert_eq!(p.command("cat ~/.ssh/id_rsa").decision, RuleDecision::Deny);
        assert_eq!(
            p.path("/srv/app/main.rs", "/srv/app", true).decision,
            RuleDecision::Allow
        );
    }

    #[test]
    fn a_write_outside_the_working_tree_asks() {
        let p = policy(&[], &[], "/srv/app");
        assert_eq!(
            p.path("/srv/app/src/main.rs", "/srv/app", true).decision,
            RuleDecision::Allow
        );
        assert_eq!(
            p.path("../elsewhere/x", "/srv/app", true).decision,
            RuleDecision::Ask
        );
        assert_eq!(p.path("/etc/hosts", "/", true).decision, RuleDecision::Deny);
        assert_eq!(
            p.path("/etc/hosts", "/", false).decision,
            RuleDecision::Allow
        );
        // A read anywhere but a secret is fine; the tool's own permission still applies.
        assert_eq!(
            p.path("/srv/other/x", "/srv/app", false).decision,
            RuleDecision::Allow
        );
    }

    #[test]
    fn a_shell_shape_that_cannot_be_read_asks() {
        let p = policy(&[], &[], "/srv/app");
        for command in [
            "sh -c 'rm -rf /'",
            "echo $(rm -rf /)",
            "bash <<'EOF'\nrm -rf /\nEOF",
        ] {
            let verdict = p.command(command);
            assert_eq!(
                verdict.decision,
                RuleDecision::Ask,
                "{command}: {verdict:?}"
            );
            assert!(verdict.reason.contains("cannot be checked"), "{verdict:?}");
            assert_eq!(
                p.path("/srv/app/x", "/srv/app", true).decision,
                RuleDecision::Allow
            );
        }
    }

    #[test]
    fn an_exact_allow_still_covers_an_unreadable_command() {
        let allow = vec!["sh -c 'rm -rf /tmp/x'".to_owned()];
        let p = policy(&allow, &[], "/srv/app");
        assert_eq!(
            p.command("sh -c 'rm -rf /tmp/x'").decision,
            RuleDecision::Allow
        );
        // A looser glob is not enough.
        let loose = vec!["sh -c *".to_owned()];
        let p = policy(&loose, &[], "/srv/app");
        assert_eq!(
            p.command("sh -c 'rm -rf /tmp/x'").decision,
            RuleDecision::Ask
        );
    }

    #[test]
    fn a_rule_covers_one_command_not_a_pipeline() {
        let allow = vec!["git status*".to_owned(), "cat *".to_owned()];
        let p = policy(&allow, &[], "/srv/app");
        assert_eq!(
            p.command("cat notes.txt | grep x").decision,
            RuleDecision::Allow
        );
        assert_eq!(
            p.command("cat notes.txt > /srv/app/out.txt").decision,
            RuleDecision::Allow
        );
        // The second command of the line is judged on its own.
        assert_eq!(
            p.command("git status && cat /etc/shadow").decision,
            RuleDecision::Deny
        );
        assert_eq!(
            p.command("git status && curl example.com").decision,
            RuleDecision::Ask
        );
    }

    #[test]
    fn a_command_that_only_reads_is_safe() {
        let p = policy(&[], &[], "/srv/app");
        for command in [
            "ls -la",
            "git log --oneline -5",
            "ps aux | grep nginx",
            "uptime",
            "find . -name '*.rs'",
        ] {
            assert_eq!(
                p.command(command).decision,
                RuleDecision::Allow,
                "{command}"
            );
        }
        // A glob that hides a write is not safe.
        for command in [
            "find . -delete",
            "git checkout .",
            "sed -i 's/a/b/' main.rs",
            "echo hi > /etc/motd",
            "rm -rf /srv/app/target",
        ] {
            assert_ne!(
                p.command(command).decision,
                RuleDecision::Allow,
                "{command}"
            );
        }
    }

    #[test]
    fn a_deny_rule_holds_whatever_else_matches() {
        let allow = vec!["*".to_owned()];
        let deny = vec!["git push*".to_owned()];
        let p = policy(&allow, &deny, "/srv/app");
        assert_eq!(
            p.command("git push origin main").decision,
            RuleDecision::Deny
        );
        assert_eq!(p.command("git status").decision, RuleDecision::Allow);
    }

    #[test]
    fn a_file_tool_names_the_paths_it_touches() {
        let call = serde_json::json!({ "path": "src/main.rs" });
        assert_eq!(
            tool_paths(TOOL_LOCAL_READ, &call),
            vec![("src/main.rs", Touch::Read, Base::ThisComputer)]
        );
        assert_eq!(
            tool_paths(TOOL_LOCAL_WRITE, &call),
            vec![("src/main.rs", Touch::Write, Base::ThisComputer)]
        );
        assert_eq!(
            tool_paths(TOOL_REMOTE_READ, &call),
            vec![("src/main.rs", Touch::Read, Base::Host)]
        );
        // A tool nobody classified is taken to write, and a move writes both ends.
        assert_eq!(
            tool_paths(TOOL_DEV_EDIT, &call),
            vec![("src/main.rs", Touch::Write, Base::Host)]
        );
        assert_eq!(
            tool_paths(
                TOOL_LOCAL_MOVE,
                &serde_json::json!({ "path": "a", "destination": "b" })
            ),
            vec![
                ("a", Touch::Write, Base::ThisComputer),
                ("b", Touch::Write, Base::ThisComputer)
            ]
        );
        // A tool that names no path is judged elsewhere.
        assert!(tool_paths(TOOL_RUN_COMMAND, &call).is_empty());
    }

    #[test]
    fn a_local_write_inside_the_tree_is_allowed_once_home_is_known() {
        let allow: Vec<String> = Vec::new();
        let deny: Vec<String> = Vec::new();
        let p = Policy {
            cwd: "/home/me/app",
            home: "/home/me",
            scratch: "",
            host_allow: &allow,
            host_deny: &deny,
            app_allow: &allow,
            app_deny: &deny,
            session_allow: &allow,
            session_deny: &deny,
        };
        assert_eq!(
            p.path("~/app/src/main.rs", "", true).decision,
            RuleDecision::Allow
        );
        assert_eq!(p.path("~/notes.txt", "", true).decision, RuleDecision::Ask);
        assert_eq!(
            p.path("~/.ssh/config", "", false).decision,
            RuleDecision::Deny
        );
        // The same file, written by a remote tool that resolves relative paths itself.
        assert_eq!(
            p.path("src/main.rs", "/home/me/app", true).decision,
            RuleDecision::Allow
        );
    }

    #[test]
    fn a_relative_path_resolves_against_the_base() {
        assert_eq!(resolve("src/main.rs", "/srv/app"), "/srv/app/src/main.rs");
        assert_eq!(resolve("../x", "/srv/app/sub"), "/srv/app/x");
        assert_eq!(resolve("~/notes", ""), "~/notes");
        assert_eq!(resolve("./a/./b", "/srv"), "/srv/a/b");
    }

    #[test]
    fn a_redirect_target_is_a_write() {
        let p = policy(&[], &[], "/srv/app");
        // Writing into the tree is fine, out of it asks, into a system file refuses.
        assert_eq!(p.command("ls > out.txt").decision, RuleDecision::Allow);
        assert_eq!(p.command("ls > ../../out.txt").decision, RuleDecision::Ask);
        assert_eq!(
            p.command("cat /etc/hosts > /etc/motd").decision,
            RuleDecision::Deny
        );
    }
}
