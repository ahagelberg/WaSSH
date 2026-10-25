//! The command approval rules.
//!
//! Straight port of `src/plugins/builtins/ai-agent/permissions.ts`: host rules
//! are checked before app defaults, deny always beats allow, and anything that
//! matches neither asks the user.

use regex::Regex;

use super::protocol::RULE_REGEX_PREFIX;

/// What a command may do without asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleDecision {
    /// Run it.
    Allow,
    /// Refuse it.
    Deny,
    /// Ask the user.
    Ask,
}

/// Pattern matching every command, used by "allow all commands this session".
pub const ALLOW_ALL_PATTERN: &str = "*";

/// How many leading command words are offered as "starts with" scopes.
pub const APPROVAL_PREFIX_WORD_MAX: usize = 4;

/// One "always allow/deny" choice: the leading words its rule would cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalScope {
    /// Leading words covered; `0` marks the exact command.
    pub word_count: usize,
    /// Text shown for this scope.
    pub text: String,
    /// Rule list entry saved when this scope is chosen.
    pub pattern: String,
}

/// Comment and empty lines are skipped.
fn is_comment_line(line: &str) -> bool {
    line.is_empty() || line.starts_with('#')
}

/// Regex for one rule list entry, or `None` when it is blank or invalid.
pub fn rule_to_regexp(raw: &str) -> Option<Regex> {
    let line = raw.trim();
    if is_comment_line(line) {
        return None;
    }
    if let Some(source) = line.strip_prefix(RULE_REGEX_PREFIX) {
        let source = source.trim();
        if source.is_empty() {
            return None;
        }
        return Regex::new(source).ok();
    }
    // A glob: everything is literal except `*`, then anchored like the JS.
    let mut pattern = String::with_capacity(line.len() * 2 + 4);
    pattern.push('^');
    for character in line.chars() {
        match character {
            '*' => pattern.push_str(".*"),
            '.' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                pattern.push('\\');
                pattern.push(character);
            }
            other => pattern.push(other),
        }
    }
    pattern.push('$');
    Regex::new(&pattern).ok()
}

/// Whether any entry in `rules` matches `command`.
pub fn matches_any(rules: Option<&[String]>, command: &str) -> bool {
    let Some(rules) = rules else {
        return false;
    };
    rules.iter().any(|rule| {
        rule_to_regexp(rule).is_some_and(|regex| regex.is_match(command))
    })
}

/// The words of a command, with runs of whitespace collapsed away.
fn command_words(command: &str) -> Vec<&str> {
    command.split_whitespace().filter(|word| !word.is_empty()).collect()
}

/// Escape every regex metacharacter, `*` included.
fn escape_meta(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() * 2);
    for character in text.chars() {
        if ".*+?^${}()|[]\\".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Rule list entries an "always allow/deny" choice can add for one command:
/// "starts with the first n words" for n up to [`APPROVAL_PREFIX_WORD_MAX`]
/// (fewer when the command is shorter), plus the exact command last.
pub fn approval_scopes(command: &str) -> Vec<ApprovalScope> {
    let words = command_words(command);
    let mut scopes = Vec::new();
    for count in 1..=APPROVAL_PREFIX_WORD_MAX.min(words.len()) {
        let text = words[..count].join(" ");
        scopes.push(ApprovalScope {
            word_count: count,
            pattern: format!("{RULE_REGEX_PREFIX}^{}( |$)", escape_meta(&text)),
            text,
        });
    }
    scopes.push(ApprovalScope {
        word_count: 0,
        text: command.to_owned(),
        pattern: command.to_owned(),
    });
    scopes
}

/// Permission decision for one command.
///
/// Host rules win over app defaults and deny always beats allow; the
/// session's own allow/deny lists (an "allow all" choice) are checked first.
pub fn decide_command(
    command: &str,
    host_allow: Option<&[String]>,
    host_deny: Option<&[String]>,
    app_allow: Option<&[String]>,
    app_deny: Option<&[String]>,
    extra_allow: &[String],
    extra_deny: &[String],
) -> RuleDecision {
    let deny_first: Vec<String> = extra_deny
        .iter()
        .chain(host_deny.unwrap_or_default())
        .cloned()
        .collect();
    if matches_any(Some(&deny_first), command) {
        return RuleDecision::Deny;
    }
    let allow_first: Vec<String> = extra_allow
        .iter()
        .chain(host_allow.unwrap_or_default())
        .cloned()
        .collect();
    if matches_any(Some(&allow_first), command) {
        return RuleDecision::Allow;
    }
    if matches_any(app_deny, command) {
        return RuleDecision::Deny;
    }
    if matches_any(app_allow, command) {
        return RuleDecision::Allow;
    }
    RuleDecision::Ask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn globs_match_the_whole_command() {
        let allow = rules(&["ls*", "git status*", "date"]);
        assert!(matches_any(Some(&allow), "ls -la /srv"));
        assert!(matches_any(Some(&allow), "git status --short"));
        assert!(matches_any(Some(&allow), "date"));
        // `date` is exact: an argument makes it a different command.
        assert!(!matches_any(Some(&allow), "date +%s"));
        assert!(!matches_any(Some(&allow), "rm -rf /"));
        // The glob is anchored, so a bare word cannot match a prefix.
        assert!(!matches_any(Some(&rules(&["rm"])), "rm -rf /tmp/x"));
    }

    #[test]
    fn dots_and_brackets_are_literal_in_a_glob() {
        let allow = rules(&["cat notes.txt", "ls [abc]"]);
        assert!(matches_any(Some(&allow), "cat notes.txt"));
        assert!(!matches_any(Some(&allow), "cat notesXtxt"));
        // A bracket in a glob is literal, not a character class.
        assert!(matches_any(Some(&allow), "ls [abc]"));
        assert!(!matches_any(Some(&allow), "ls a"));
    }

    #[test]
    fn the_regex_prefix_is_honoured_and_ignored_when_broken() {
        let allow = rules(&["regex:^git (push|pull)", "regex:(", "# a comment", ""]);
        assert!(matches_any(Some(&allow), "git push origin main"));
        assert!(matches_any(Some(&allow), "git pull"));
        assert!(!matches_any(Some(&allow), "git commit"));
        // The broken pattern and the blank entries simply do not match.
        assert!(!matches_any(Some(&allow), "anything"));
    }

    #[test]
    fn deny_beats_allow_and_host_beats_app() {
        let app_allow = rules(&["rm*"]);
        let app_deny = rules(&["rm -rf *"]);
        let host_allow = rules(&["rm -rf build"]);
        let host_deny = rules(&["rm -rf build"]);

        // Deny wins over allow at the same level.
        assert_eq!(
            decide_command("rm -rf /srv", None, None, Some(&app_allow), Some(&app_deny), &[], &[]),
            RuleDecision::Deny
        );
        // A host allow beats an app deny for that host.
        assert_eq!(
            decide_command(
                "rm -rf build",
                Some(&host_allow),
                None,
                Some(&app_allow),
                Some(&app_deny),
                &[],
                &[]
            ),
            RuleDecision::Allow
        );
        // A host deny beats an app allow.
        assert_eq!(
            decide_command(
                "rm -rf build",
                None,
                Some(&host_deny),
                Some(&app_allow),
                None,
                &[],
                &[]
            ),
            RuleDecision::Deny
        );
        // Nothing matches: ask.
        assert_eq!(
            decide_command("make install", None, None, Some(&app_allow), None, &[], &[]),
            RuleDecision::Ask
        );
    }

    #[test]
    fn a_session_allow_list_answers_before_the_stored_rules() {
        let app_deny = rules(&["apt *"]);
        assert_eq!(
            decide_command(
                "apt upgrade",
                None,
                None,
                None,
                Some(&app_deny),
                &rules(&["apt*"]),
                &[]
            ),
            RuleDecision::Allow
        );
        // A session deny refuses even a shipped safe rule.
        let app_allow = rules(&["ls*"]);
        assert_eq!(
            decide_command("ls -la", None, None, Some(&app_allow), None, &[], &rules(&["ls*"])),
            RuleDecision::Deny
        );
    }

    #[test]
    fn approval_scopes_walk_the_leading_words_then_the_command() {
        let scopes = approval_scopes("git push origin main");
        let texts: Vec<&str> = scopes.iter().map(|scope| scope.text.as_str()).collect();
        assert_eq!(
            texts,
            ["git", "git push", "git push origin", "git push origin main", "git push origin main"]
        );
        assert_eq!(scopes[0].pattern, "regex:^git( |$)");
        assert_eq!(scopes[2].pattern, "regex:^git push origin( |$)");
        // A four-word command fills every prefix slot, so the fourth entry is
        // the whole command — the reference does the same and then adds the
        // exact command again as the last choice.
        assert_eq!(scopes[3].pattern, "regex:^git push origin main( |$)");
        assert_eq!(scopes[4].word_count, 0);
        assert_eq!(scopes[4].pattern, "git push origin main");

        // A short command offers what it has.
        let short = approval_scopes("uptime");
        assert_eq!(short.len(), 2);
        assert_eq!(short[0].text, "uptime");
        assert_eq!(short[1].pattern, "uptime");
    }

    #[test]
    fn a_scope_pattern_matches_the_command_it_was_built_for() {
        let command = "docker compose up -d";
        for scope in approval_scopes(command) {
            let allow = vec![scope.pattern.clone()];
            assert!(
                matches_any(Some(&allow), command),
                "{} does not match {command}",
                scope.pattern
            );
        }
        // The exact scope matches the command alone.
        let exact = approval_scopes(command).pop().expect("exact scope");
        assert!(!matches_any(Some(&[exact.pattern]), "docker compose down"));
    }
}
