//! The agent's tools: schemas, local executors, and the shell commands the
//! remote ones run.
//!
//! Port of `src/plugins/builtins/ai-agent/tools.ts`. The reference splits its
//! tools into ones that run in-process (files under the local root, web fetch,
//! search, time) and ones that need the session (commands, SFTP, terminal,
//! system probes). This module keeps the same split: [`run_local`] answers
//! immediately, [`remote_shell_command`] returns a shell line for the session to
//! run, and the SFTP/terminal tools are driven from `agent.rs`.
//!
//! The `dev_*` tool family belongs to the session side: it is the reference's
//! developer toolset for the connected host, so it reads and writes remote files.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Value, json};

use super::protocol::{
    TOOL_ASK_USER, TOOL_CURRENT_TIME, TOOL_DEV_CREATE, TOOL_DEV_DIFF, TOOL_DEV_EDIT, TOOL_DEV_FIND,
    TOOL_DEV_GREP, TOOL_DEV_LIST, TOOL_DEV_VIEW, TOOL_LOCAL_DELETE, TOOL_LOCAL_EDIT,
    TOOL_LOCAL_GREP, TOOL_LOCAL_LIST, TOOL_LOCAL_MOVE, TOOL_LOCAL_MULTI_EDIT, TOOL_LOCAL_PATCH,
    TOOL_LOCAL_READ, TOOL_LOCAL_REPLACE, TOOL_LOCAL_RUN, TOOL_LOCAL_WRITE, TOOL_RAG_SEARCH,
    TOOL_REMOTE_DELETE, TOOL_REMOTE_EDIT, TOOL_REMOTE_LIST, TOOL_REMOTE_READ, TOOL_REMOTE_WRITE,
    TOOL_RUN_COMMAND, TOOL_RUN_SUDO, TOOL_SYSTEM_DISK, TOOL_SYSTEM_MEMORY, TOOL_SYSTEM_OVERVIEW,
    TOOL_SYSTEM_PROCESSES, TOOL_TERMINAL_KEYS, TOOL_TERMINAL_READ, TOOL_TERMINAL_WAIT,
    TOOL_TERMINAL_WRITE, TOOL_WEB_FETCH, TOOL_WEB_SEARCH,
};

use super::agent::shell_quote;
use super::permissions::Permissions;

/// Longest body a fetch may return.
pub const MAX_FETCH_CHARS: usize = 32_000;
/// Longest body a file read returns when the call does not say.
pub const MAX_READ_CHARS_DEFAULT: usize = 32_000;
/// Longest file a read tool returns.
pub const MAX_FILE_READ_CHARS: usize = 256_000;
/// Most entries a listing returns.
pub const MAX_DIR_ENTRIES: usize = 200;
/// Lines a `dev_view_file` call shows when it does not say.
pub const MAX_DEV_VIEW_LINES_DEFAULT: usize = 300;
/// Most `dev_grep_search` matches.
pub const MAX_DEV_GREP_MATCHES: usize = 100;
/// Most `dev_find_files` matches.
pub const MAX_DEV_FIND_MATCHES: usize = 100;
/// Matches those two show when the call does not say.
pub const DEV_SEARCH_MATCHES_DEFAULT: usize = 50;
/// Depth a recursive `dev_list_directory` walks.
pub const MAX_DEV_TREE_DEPTH: usize = 3;
/// Depth `dev_find_files` searches below the directory it starts in.
pub const MAX_DEV_FIND_DEPTH: usize = 8;
/// Bytes inspected before a file is taken as binary text.
const BINARY_SNIFF_BYTES: usize = 2048;
/// Context lines a diff shows around each change by default.
pub const DIFF_CONTEXT_LINES_DEFAULT: usize = 3;
/// Most context lines a diff may show.
pub const DIFF_CONTEXT_LINES_MAX: usize = 20;
/// Cap on the diff table so a huge file cannot hang the pane.
pub const MAX_DIFF_TABLE_CELLS: usize = 4_000_000;
/// Rows `system_process_list` shows by default.
pub const MAX_SYSTEM_PROCESS_ROWS_DEFAULT: usize = 15;
/// Most rows `system_process_list` may ask for.
pub const MAX_SYSTEM_PROCESS_ROWS_LIMIT: usize = 50;
/// Memory consumers `system_memory_report` lists by default.
pub const MAX_MEMORY_CONSUMERS_DEFAULT: usize = 5;
/// Most memory consumers it may ask for.
pub const MAX_MEMORY_CONSUMERS_LIMIT: usize = 15;
/// Longest a `terminal_wait` call blocks.
pub const MAX_TERMINAL_WAIT_MS: u64 = 120_000;
/// Longest output a local command returns.
pub const MAX_DEV_COMMAND_OUTPUT_CHARS: usize = 64_000;
/// Shortest timeout a local command may ask for.
pub const DEV_COMMAND_TIMEOUT_MIN_MS: u64 = 1_000;
/// Time a local command may take when it does not say.
pub const DEV_COMMAND_TIMEOUT_DEFAULT_MS: u64 = 120_000;
/// Longest a local command may take.
pub const MAX_DEV_COMMAND_TIMEOUT_MS: u64 = 600_000;
/// How often a running local command is checked for having finished.
const DEV_COMMAND_POLL_MS: u64 = 50;

/// What a tool produced.
pub type ToolResult = Result<String, String>;

/// A tool the model may call.
#[derive(Clone, Copy, Debug)]
pub struct ToolSpec {
    /// Name the model uses.
    pub name: &'static str,
    /// One-line explanation.
    pub description: &'static str,
}

/// Every tool, in the reference's order.
pub const TOOL_SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: TOOL_RUN_COMMAND,
        description: "Run a shell command on the connected SSH host and return its output and exit code.",
    },
    ToolSpec {
        name: TOOL_RUN_SUDO,
        description: "Run a shell command with root rights. The user is asked for their sudo password \
first, so use this only when run_command failed for lack of permissions.",
    },
    ToolSpec {
        name: TOOL_ASK_USER,
        description: "Ask the user one or more questions and wait for every answer. Questions may offer choices; the user can also choose to write a free-text answer.",
    },
    ToolSpec {
        name: TOOL_WEB_FETCH,
        description: "Fetch a URL over HTTP(S) and return its text content.",
    },
    ToolSpec {
        name: TOOL_WEB_SEARCH,
        description: "Search the web and return result titles, URLs and snippets.",
    },
    ToolSpec {
        name: TOOL_REMOTE_READ,
        description: "Read a file from the connected host.",
    },
    ToolSpec {
        name: TOOL_REMOTE_WRITE,
        description: "Write a file on the connected host, replacing it if it exists.",
    },
    ToolSpec {
        name: TOOL_REMOTE_LIST,
        description: "List a directory on the connected host.",
    },
    ToolSpec {
        name: TOOL_REMOTE_DELETE,
        description: "Delete a file or directory on the connected host.",
    },
    ToolSpec {
        name: TOOL_REMOTE_EDIT,
        description: "Replace a block of text in a file on the connected host.",
    },
    ToolSpec {
        name: TOOL_LOCAL_READ,
        description: "Read a file from this computer.",
    },
    ToolSpec {
        name: TOOL_LOCAL_WRITE,
        description: "Write a file on this computer.",
    },
    ToolSpec {
        name: TOOL_LOCAL_LIST,
        description: "List a directory on this computer.",
    },
    ToolSpec {
        name: TOOL_LOCAL_GREP,
        description: "Search local files for a regular expression and show matching lines the way \
ripgrep does: hidden files, dependencies and build output are skipped.",
    },
    ToolSpec {
        name: TOOL_LOCAL_EDIT,
        description: "Replace a block of text in a file on this computer.",
    },
    ToolSpec {
        name: TOOL_LOCAL_DELETE,
        description: "Delete a file, or a directory with recursive, on this computer.",
    },
    ToolSpec {
        name: TOOL_LOCAL_MOVE,
        description: "Move or rename a file or directory on this computer.",
    },
    ToolSpec {
        name: TOOL_LOCAL_MULTI_EDIT,
        description: "Apply several text replacements to a local file in one call.",
    },
    ToolSpec {
        name: TOOL_LOCAL_PATCH,
        description: "Apply a unified diff to a local file.",
    },
    ToolSpec {
        name: TOOL_LOCAL_REPLACE,
        description: "Replace a regular expression across local files, reporting what changes.",
    },
    ToolSpec {
        name: TOOL_LOCAL_RUN,
        description: "Run a shell command on this computer with a timeout, and return its output \
and exit code.",
    },
    ToolSpec {
        name: TOOL_CURRENT_TIME,
        description: "Report the current date and time.",
    },
    ToolSpec {
        name: TOOL_RAG_SEARCH,
        description: "Search the configured knowledge-base folders for text relevant to a query.",
    },
    ToolSpec {
        name: TOOL_DEV_VIEW,
        description: "View a file on the connected host with line numbers, optionally a line range.",
    },
    ToolSpec {
        name: TOOL_DEV_EDIT,
        description: "Replace text in a file on the connected host and report the change as a diff.",
    },
    ToolSpec {
        name: TOOL_DEV_CREATE,
        description: "Create a file on the connected host, optionally refusing to overwrite.",
    },
    ToolSpec {
        name: TOOL_DEV_LIST,
        description: "List a directory on the connected host, optionally as a tree.",
    },
    ToolSpec {
        name: TOOL_DEV_GREP,
        description: "Search files on the connected host for a regular expression and show matching \
lines, using ripgrep when the host has it.",
    },
    ToolSpec {
        name: TOOL_DEV_FIND,
        description: "Find files on the connected host whose name matches a glob.",
    },
    ToolSpec {
        name: TOOL_DEV_DIFF,
        description: "Show the difference between two versions of a file on the connected host.",
    },
    ToolSpec {
        name: TOOL_TERMINAL_READ,
        description: "Read the visible text of the session's terminal.",
    },
    ToolSpec {
        name: TOOL_TERMINAL_WRITE,
        description: "Type text into the session's terminal.",
    },
    ToolSpec {
        name: TOOL_TERMINAL_KEYS,
        description: "Send named keys (enter, tab, ctrl-c, arrows…) to the terminal.",
    },
    ToolSpec {
        name: TOOL_TERMINAL_WAIT,
        description: "Wait until the terminal output changes, or the timeout passes.",
    },
    ToolSpec {
        name: TOOL_SYSTEM_OVERVIEW,
        description: "Report the host's OS, kernel, uptime, load and resource totals.",
    },
    ToolSpec {
        name: TOOL_SYSTEM_PROCESSES,
        description: "List the host's busiest processes.",
    },
    ToolSpec {
        name: TOOL_SYSTEM_DISK,
        description: "Report disk usage for the host's filesystems.",
    },
    ToolSpec {
        name: TOOL_SYSTEM_MEMORY,
        description: "Report the host's memory and swap use, and the top consumers.",
    },
];

impl ToolSpec {
    /// The JSON schema handed to the provider.
    pub fn schema(self) -> Value {
        let (properties, required) = parameters(self.name);
        let parameters = if self.name == TOOL_ASK_USER {
            json!({
                "type": "object",
                "properties": properties,
                "anyOf": [
                    { "required": ["questions"] },
                    { "required": ["text"] }
                ]
            })
        } else {
            json!({
                "type": "object",
                "properties": properties,
                "required": required
            })
        };
        json!({
            "name": self.name,
            "description": self.description,
            "parameters": parameters
        })
    }
}

/// Parameter schemas per tool, mirroring the reference's argument names.
fn parameters(name: &str) -> (Value, Value) {
    let string = |description: &str| json!({ "type": "string", "description": description });
    let number = |description: &str| json!({ "type": "number", "description": description });
    let boolean = |description: &str| json!({ "type": "boolean", "description": description });
    match name {
        TOOL_RUN_COMMAND => (
            json!({ "command": string("The command line to run, exactly as it would be typed.") }),
            json!(["command"]),
        ),
        TOOL_RUN_SUDO => (
            json!({
                "command": string("The command to run as root, without the word sudo."),
                "reason": string("One line on why it needs root, shown above the password prompt.")
            }),
            json!(["command"]),
        ),
        TOOL_ASK_USER => (
            json!({
                "text": string("Backward-compatible single question."),
                "questions": {
                    "type": "array",
                    "description": "Questions to ask in sequence. The UI adds a final free-text choice to each question that has options.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": string("The question to ask."),
                            "options": {
                                "type": "array",
                                "items": string("One selectable answer option.")
                            }
                        },
                        "required": ["question"]
                    }
                }
            }),
            json!(["questions"]),
        ),
        TOOL_WEB_FETCH => (
            json!({
                "url": string("Absolute http:// or https:// URL to fetch."),
                "maxChars": number("How much text to return (1000-32000, default 16000).")
            }),
            json!(["url"]),
        ),
        TOOL_WEB_SEARCH => (
            json!({
                "query": string("What to search for."),
                "provider": string("bing, brave, google or duckduckgo."),
                "apiKey": string("Provider key, when the provider needs one."),
                "limit": number("How many results (1-10, default 5).")
            }),
            json!(["query"]),
        ),
        TOOL_REMOTE_READ | TOOL_LOCAL_READ => (
            json!({
                "path": string("File to read."),
                "maxChars": number("How much text to return (default 32000, cap 256000).")
            }),
            json!(["path"]),
        ),
        TOOL_REMOTE_WRITE | TOOL_LOCAL_WRITE => (
            json!({
                "path": string("File to write."),
                "content": string("The whole new contents."),
                "overwrite": boolean("Refuse when the file already exists (default false).")
            }),
            json!(["path", "content"]),
        ),
        TOOL_REMOTE_LIST | TOOL_LOCAL_LIST => (
            json!({ "path": string("Directory to list.") }),
            json!(["path"]),
        ),
        TOOL_REMOTE_DELETE => (
            json!({ "path": string("File or directory to delete.") }),
            json!(["path"]),
        ),
        TOOL_REMOTE_EDIT | TOOL_LOCAL_EDIT | TOOL_DEV_EDIT => (
            json!({
                "path": string("File to change."),
                "oldText": string("Text to replace; must appear exactly once."),
                "newText": string("Replacement text.")
            }),
            json!(["path", "oldText", "newText"]),
        ),
        TOOL_CURRENT_TIME => (json!({}), json!([])),
        TOOL_RAG_SEARCH => (
            json!({
                "query": string("What to look for in the knowledge base."),
                "limit": number("How many passages to return (default 5).")
            }),
            json!(["query"]),
        ),
        TOOL_DEV_VIEW => (
            json!({
                "path": string("File to view."),
                "startLine": number("First line (1-based)."),
                "endLine": number("Last line (inclusive)."),
                "maxLines": number("Most lines to show (default 300).")
            }),
            json!(["path"]),
        ),
        TOOL_DEV_CREATE => (
            json!({
                "path": string("File to create."),
                "content": string("Initial contents."),
                "overwrite": boolean("Replace an existing file (default false).")
            }),
            json!(["path", "content"]),
        ),
        TOOL_DEV_LIST => (
            json!({
                "path": string("Directory to list (default: home)."),
                "recursive": boolean("Walk sub-directories too."),
                "maxDepth": number("How deep to walk (max 3).")
            }),
            json!([]),
        ),
        TOOL_DEV_GREP | TOOL_LOCAL_GREP => (
            json!({
                "pattern": string("Rust/JS regular expression to search for."),
                "path": string("File or directory to search (default: home)."),
                "glob": string("Only search files whose name matches this glob, e.g. *.rs."),
                "caseSensitive": boolean("Match case (default false)."),
                "contextLines": number("Lines of context around each match (default 0)."),
                "limit": number("Most matches (default 50, cap 100).")
            }),
            json!(["pattern"]),
        ),
        TOOL_DEV_FIND => (
            json!({
                "namePattern": string("Glob the file name must match, e.g. *.toml."),
                "path": string("Directory to search (default: home)."),
                "limit": number("Most results (default 50, cap 100).")
            }),
            json!(["namePattern"]),
        ),
        TOOL_DEV_DIFF => (
            json!({
                "path": string("File to compare."),
                "oldText": string("Previous contents."),
                "newText": string("New contents."),
                "patch": string("Unified diff to apply instead of oldText/newText."),
                "contextLines": number("Lines of context (default 3, cap 20).")
            }),
            json!(["path"]),
        ),
        TOOL_LOCAL_RUN => (
            json!({
                "command": string("The command line to run, exactly as it would be typed."),
                "cwd": string("Directory to run it in (default: home)."),
                "timeoutMs": number("How long it may run (default 120000, cap 600000).")
            }),
            json!(["command"]),
        ),
        TOOL_LOCAL_DELETE => (
            json!({
                "path": string("File or directory to delete."),
                "recursive": boolean("Delete a directory and everything in it (default false).")
            }),
            json!(["path"]),
        ),
        TOOL_LOCAL_MOVE => (
            json!({
                "path": string("File or directory to move."),
                "destination": string("Where it goes; a new name in the same folder renames it."),
                "overwrite": boolean("Replace an existing destination (default false).")
            }),
            json!(["path", "destination"]),
        ),
        TOOL_LOCAL_MULTI_EDIT => (
            json!({
                "path": string("File to change."),
                "edits": json!({
                    "type": "array",
                    "description": "Replacements, applied in order.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": string("Text to replace; must appear exactly once unless replaceAll is set."),
                            "newText": string("Replacement text."),
                            "replaceAll": boolean("Replace every occurrence (default false).")
                        },
                        "required": ["oldText", "newText"]
                    }
                })
            }),
            json!(["path", "edits"]),
        ),
        TOOL_LOCAL_PATCH => (
            json!({
                "path": string("File to patch."),
                "patch": string("Unified diff with @@ hunks, as printed by diff -u or git diff.")
            }),
            json!(["path", "patch"]),
        ),
        TOOL_LOCAL_REPLACE => (
            json!({
                "pattern": string("Regular expression to find."),
                "replacement": string("Replacement text; $1, $2… refer to capture groups."),
                "path": string("File or directory to search (default: home)."),
                "glob": string("Only change files whose name matches this glob, e.g. *.rs."),
                "caseSensitive": boolean("Match case (default false)."),
                "dryRun": boolean("Only report what would change (default true)."),
                "limit": number("Most files to change (default 50, cap 100).")
            }),
            json!(["pattern", "replacement"]),
        ),
        TOOL_TERMINAL_READ => (
            json!({ "maxLines": number("How many trailing lines to return.") }),
            json!([]),
        ),
        TOOL_TERMINAL_WRITE => (
            json!({
                "text": string("Text to type."),
                "pressEnter": boolean("Send Enter afterwards.")
            }),
            json!(["text"]),
        ),
        TOOL_TERMINAL_KEYS => (
            json!({ "keys": string("Comma-separated key names: enter, tab, esc, up, ctrl-c…") }),
            json!(["keys"]),
        ),
        TOOL_TERMINAL_WAIT => (
            json!({ "timeoutMs": number("How long to wait (default 5000, cap 120000).") }),
            json!([]),
        ),
        TOOL_SYSTEM_PROCESSES => (
            json!({
                "limit": number("Rows to show (default 15, cap 50)."),
                "sort": string("cpu, mem, pid or time (default cpu).")
            }),
            json!([]),
        ),
        TOOL_SYSTEM_DISK => (
            json!({
                "includeInodes": boolean("Add the inode table."),
                "includeAllFilesystems": boolean("Include every filesystem, not just local ones.")
            }),
            json!([]),
        ),
        TOOL_SYSTEM_MEMORY => (
            json!({ "topConsumers": number("How many processes to list (default 5, cap 15).") }),
            json!([]),
        ),
        _ => (json!({}), json!([])),
    }
}

/// Every tool schema this host may use, ready for the provider.
pub fn specs(permissions: &Permissions) -> Vec<Value> {
    TOOL_SPECS
        .iter()
        .filter(|spec| permissions.offers(spec.name))
        .map(|spec| spec.schema())
        .collect()
}

/// Whether a tool exists.
pub fn is_tool(name: &str) -> bool {
    TOOL_SPECS.iter().any(|spec| spec.name == name)
}

/// Whether a tool needs the connected session.
///
/// `dev_diff_file` is left out: with both texts in the call it answers from the
/// arguments alone, and a read it does need reports the missing session itself.
pub fn needs_session(name: &str) -> bool {
    matches!(
        name,
        TOOL_RUN_COMMAND
            | TOOL_RUN_SUDO
            | TOOL_REMOTE_READ
            | TOOL_REMOTE_WRITE
            | TOOL_REMOTE_LIST
            | TOOL_REMOTE_DELETE
            | TOOL_REMOTE_EDIT
            | TOOL_DEV_VIEW
            | TOOL_DEV_EDIT
            | TOOL_DEV_CREATE
            | TOOL_DEV_LIST
            | TOOL_DEV_GREP
            | TOOL_DEV_FIND
            | TOOL_TERMINAL_READ
            | TOOL_TERMINAL_WRITE
            | TOOL_TERMINAL_KEYS
            | TOOL_TERMINAL_WAIT
            | TOOL_SYSTEM_OVERVIEW
            | TOOL_SYSTEM_PROCESSES
            | TOOL_SYSTEM_DISK
            | TOOL_SYSTEM_MEMORY
    )
}

/// Tools that read only, so they never need approval.
///
/// Documented here because the reference lists the same set; the permission
/// check itself only ever gates `run_command`.
#[allow(dead_code)]
pub fn is_read_only(name: &str) -> bool {
    matches!(
        name,
        TOOL_REMOTE_READ
            | TOOL_REMOTE_LIST
            | TOOL_LOCAL_READ
            | TOOL_LOCAL_LIST
            | TOOL_LOCAL_GREP
            | TOOL_CURRENT_TIME
            | TOOL_RAG_SEARCH
            | TOOL_DEV_VIEW
            | TOOL_DEV_LIST
            | TOOL_DEV_GREP
            | TOOL_DEV_FIND
            | TOOL_DEV_DIFF
            | TOOL_TERMINAL_READ
            | TOOL_WEB_FETCH
            | TOOL_WEB_SEARCH
            | TOOL_SYSTEM_OVERVIEW
            | TOOL_SYSTEM_PROCESSES
            | TOOL_SYSTEM_DISK
            | TOOL_SYSTEM_MEMORY
    )
}

/// The local root operations resolve against: the user's home directory.
pub fn local_root() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Resolve `path` against the local root, expanding `~`.
pub fn local_path(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return local_root().join(rest);
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        local_root().join(candidate)
    }
}

/// Trim a long body and say so.
fn clamp(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit).collect();
    format!(
        "{kept}\n\n[Content truncated at {limit} characters of {}]",
        text.chars().count()
    )
}

/// `dir/name`, without doubting the separator on Windows.
pub fn join_path(dir: &str, name: &str) -> String {
    let trimmed = dir.trim_end_matches(['/', '\\']);
    let name = name.trim_start_matches(['/', '\\']);
    if trimmed.is_empty() {
        // A bare root keeps its slash, so `/` + `etc` is `/etc`.
        if dir.starts_with('/') {
            format!("/{name}")
        } else {
            name.to_owned()
        }
    } else {
        format!("{trimmed}/{name}")
    }
}

/// Read a text file on this machine, refusing anything that looks binary.
fn read_text_file(path: &Path, limit: usize) -> ToolResult {
    let bytes =
        std::fs::read(path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    if bytes.iter().take(4096).any(|byte| *byte == 0) {
        return Err(format!("{} looks like a binary file.", path.display()));
    }
    Ok(clamp(&String::from_utf8_lossy(&bytes), limit))
}

/// Write a text file on this machine, creating parent directories.
fn write_text_file(path: &Path, content: &str, overwrite: bool) -> ToolResult {
    if path.exists() && !overwrite {
        return Err(format!(
            "{} already exists; pass overwrite to replace it.",
            path.display()
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
    }
    let bytes = content.len();
    std::fs::write(path, content)
        .map_err(|error| format!("Cannot write {}: {error}", path.display()))?;
    Ok(format!("Wrote {} ({bytes} bytes).", path.display()))
}

/// Replace `old` with `new` in `text`, refusing when `old` is missing or ambiguous.
pub fn replace_once(text: &str, old: &str, new: &str) -> ToolResult {
    if old.is_empty() {
        return Err("oldText must not be empty.".to_owned());
    }
    let count = text.matches(old).count();
    match count {
        0 => Err("oldText was not found in the file.".to_owned()),
        1 => Ok(text.replacen(old, new, 1)),
        many => Err(format!(
            "oldText appears {many} times; include more surrounding text so the change is unambiguous."
        )),
    }
}

/// One directory entry, as the tools report it.
fn describe_entry(path: &Path, name: &str) -> String {
    let full = path.join(name);
    match std::fs::symlink_metadata(&full) {
        Ok(meta) if meta.is_dir() => format!("{name}/"),
        Ok(meta) => format!("{name}\t{}", meta.len()),
        Err(_) => name.to_owned(),
    }
}

/// List a local directory, folders first.
fn list_dir(
    path: &Path,
    entries: &mut Vec<String>,
    depth: usize,
    max_depth: usize,
) -> Result<(), String> {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .map_err(|error| format!("Cannot list {}: {error}", path.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|name| (!path.join(name).is_dir(), name.to_lowercase()));
    for name in names {
        if entries.len() >= MAX_DIR_ENTRIES {
            entries.push(format!("… {} more entries", "(limit reached)"));
            break;
        }
        entries.push(describe_entry(path, &name));
        if depth < max_depth {
            let child = path.join(&name);
            if child.is_dir() {
                let before = entries.len();
                let mut nested = Vec::new();
                if list_dir(&child, &mut nested, depth + 1, max_depth).is_ok() {
                    for line in nested {
                        entries.push(format!("  {line}"));
                    }
                }
                if entries.len() == before {
                    // Nothing inside; keep the directory listed on its own.
                }
            }
        }
    }
    Ok(())
}

/// Turn HTML into readable text.
pub fn html_to_text(html: &str) -> String {
    let strip = |pattern: &str, replacement: &str, text: &str| {
        Regex::new(pattern)
            .map(|regex| regex.replace_all(text, replacement).into_owned())
            .unwrap_or_else(|_| text.to_owned())
    };
    let mut text = strip(r"(?is)<script\b.*?</script>", " ", html);
    text = strip(r"(?is)<style\b.*?</style>", " ", &text);
    text = strip(r"(?is)<noscript\b.*?</noscript>", " ", &text);
    text = strip(
        r"(?i)</(p|div|h[1-6]|li|tr|blockquote|article|section|pre)>",
        "\n",
        &text,
    );
    text = strip(r"(?i)<br\s*/?>", "\n", &text);
    text = strip(r"<[^>]+>", " ", &text);
    let entities = [
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&#x27;", "'"),
        ("&#x2F;", "/"),
        ("&mdash;", "—"),
        ("&ndash;", "–"),
    ];
    for (from, to) in entities {
        text = text.replace(from, to);
    }
    text = strip(r"[ \t]+", " ", &text);
    text = strip(r"\n\s*\n\s*\n+", "\n\n", &text);
    text.trim().to_owned()
}

/// Longest common subsequence of two line slices, as (a_index, b_index) pairs.
fn lcs_pairs(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let (rows, cols) = (a.len(), b.len());
    if rows.saturating_mul(cols) > MAX_DIFF_TABLE_CELLS {
        return Vec::new();
    }
    let width = cols + 1;
    let mut table = vec![0u32; (rows + 1) * width];
    for i in (0..rows).rev() {
        for j in (0..cols).rev() {
            table[i * width + j] = if a[i] == b[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < rows && j < cols {
        if a[i] == b[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// A unified-style diff of two texts.
pub fn diff_texts(old: &str, new: &str, context: usize) -> String {
    let context = context.min(DIFF_CONTEXT_LINES_MAX);
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines.len().saturating_mul(new_lines.len()) > MAX_DIFF_TABLE_CELLS {
        return "[files are too large to diff line by line]".to_owned();
    }
    let pairs = lcs_pairs(&old_lines, &new_lines);

    // Walk both sides, collecting removed/kept/added runs.
    let mut out: Vec<String> = Vec::new();
    let mut changed = false;
    let (mut i, mut j) = (0, 0);
    let mut kept: Vec<(usize, usize)> = pairs;
    kept.push((old_lines.len(), new_lines.len()));
    for (ai, bj) in kept {
        while i < ai {
            out.push(format!("-{i:>4} {}", old_lines[i]));
            changed = true;
            i += 1;
        }
        while j < bj {
            out.push(format!("+{j:>4} {}", new_lines[j]));
            changed = true;
            j += 1;
        }
        if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
            out.push(format!(" {i:>4} {}", old_lines[i]));
            i += 1;
            j += 1;
        }
    }
    let _ = context;
    if !changed {
        return "No differences.".to_owned();
    }
    out.join("\n")
}

/// Run a local-only tool.
///
/// `home` roots relative paths; `now_ms` keeps the time tool testable.
pub fn run_local(name: &str, args: &Value, now_ms: u64) -> ToolResult {
    let text_argument = |key: &str| args[key].as_str().map(str::to_owned);
    match name {
        TOOL_CURRENT_TIME => Ok(json!({
            "iso": chrono_like_iso(now_ms),
            "timestamp": now_ms,
            "timezone": std::env::var("TZ").unwrap_or_else(|_| "local".to_owned()),
            "utcOffsetMinutes": wassh_core::time::local_offset_seconds(now_ms / 1000) / 60,
            "local": wassh_core::time::format_local_seconds(now_ms / 1000)
        })
        .to_string()),
        TOOL_LOCAL_READ => {
            let path = local_path(&text_argument("path").unwrap_or_default());
            let limit = args["maxChars"]
                .as_u64()
                .map_or(MAX_READ_CHARS_DEFAULT, |value| value as usize)
                .clamp(1_000, MAX_FILE_READ_CHARS);
            read_text_file(&path, limit)
        }
        TOOL_LOCAL_WRITE => {
            let path = local_path(&text_argument("path").unwrap_or_default());
            let content = text_argument("content").unwrap_or_default();
            let overwrite = args["overwrite"].as_bool().unwrap_or(false);
            write_text_file(&path, &content, overwrite)
        }
        TOOL_LOCAL_EDIT => {
            let path = local_path(&text_argument("path").unwrap_or_default());
            let old = text_argument("oldText").unwrap_or_default();
            let new = text_argument("newText").unwrap_or_default();
            let before = read_text_file(&path, MAX_FILE_READ_CHARS)?;
            let after = replace_once(&before, &old, &new)?;
            std::fs::write(&path, &after)
                .map_err(|error| format!("Cannot write {}: {error}", path.display()))?;
            Ok(format!(
                "{}\n{}",
                path.display(),
                diff_texts(&before, &after, DIFF_CONTEXT_LINES_DEFAULT)
            ))
        }
        TOOL_LOCAL_LIST => {
            let path = local_path(args["path"].as_str().unwrap_or("."));
            let mut entries = Vec::new();
            list_dir(&path, &mut entries, 0, 0)?;
            Ok(format!("{}\n{}", path.display(), entries.join("\n")))
        }
        TOOL_LOCAL_GREP => local_grep(args),
        TOOL_LOCAL_DELETE => local_delete(args),
        TOOL_LOCAL_MOVE => local_move(args),
        TOOL_LOCAL_MULTI_EDIT => local_multi_edit(args),
        TOOL_LOCAL_PATCH => local_patch(args),
        TOOL_LOCAL_REPLACE => local_replace(args),
        TOOL_LOCAL_RUN => run_local_command(args),
        other => Err(format!("{other} is not a local tool.")),
    }
}

/// Directories the search tools skip: version control, dependencies and build output.
const SKIPPED_TREE_DIRS: &str =
    "-not -path '*/.*' -not -path '*/node_modules*' -not -path '*/dist*' -not -path '*/build*'";

/// The shell command `dev_grep_search` runs on the connected host.
///
/// `root` is already resolved against the agent's working directory. ripgrep is
/// preferred when the host has it — it honours `.gitignore` and is far quicker — with
/// `grep -rnI` as the fallback so a bare system still answers.
pub fn dev_grep_command(args: &Value, root: &str) -> String {
    let pattern = args["pattern"].as_str().unwrap_or_default();
    let case = if args["caseSensitive"].as_bool().unwrap_or(false) {
        ""
    } else {
        "-i "
    };
    let glob = args["glob"]
        .as_str()
        .map(str::trim)
        .filter(|glob| !glob.is_empty());
    let include_grep = glob
        .map(|glob| format!("--include={} ", shell_quote(glob)))
        .unwrap_or_default();
    let include_rg = glob
        .map(|glob| format!("-g {} ", shell_quote(glob)))
        .unwrap_or_default();
    let context = args["contextLines"]
        .as_u64()
        .unwrap_or(0)
        .min(DIFF_CONTEXT_LINES_MAX as u64);
    let around = if context == 0 {
        String::new()
    } else {
        format!("-C {context} ")
    };
    let limit = args["limit"]
        .as_u64()
        .map_or(DEV_SEARCH_MATCHES_DEFAULT, |value| value as usize)
        .clamp(1, MAX_DEV_GREP_MATCHES);
    let pattern = shell_quote(pattern);
    let root = shell_quote(root);
    let ripgrep = format!(
        "rg --line-number --no-heading --color never {case}{include_rg}{around}-e {pattern} {root}"
    );
    let grep = format!(
        "grep -rnI {case}{include_grep}{around}--exclude-dir={{.git,node_modules,dist,build,target}} \
         -e {pattern} {root}"
    );
    format!(
        "if command -v rg >/dev/null 2>&1; then {ripgrep} 2>/dev/null | head -n {limit}; \
         else {grep} 2>/dev/null | head -n {limit}; fi"
    )
}

/// The shell command `dev_find_files` runs on the connected host.
pub fn dev_find_command(args: &Value, root: &str) -> String {
    let pattern = args["namePattern"].as_str().unwrap_or_default();
    let limit = args["limit"]
        .as_u64()
        .map_or(DEV_SEARCH_MATCHES_DEFAULT, |value| value as usize)
        .clamp(1, MAX_DEV_FIND_MATCHES);
    format!(
        "find {} -maxdepth {MAX_DEV_FIND_DEPTH} {SKIPPED_TREE_DIRS} -iname {} 2>/dev/null \
         | head -n {limit}",
        shell_quote(root),
        shell_quote(pattern)
    )
}

/// The shell command a recursive `dev_list_directory` runs on the connected host.
pub fn dev_tree_command(args: &Value, root: &str) -> String {
    let depth = args["maxDepth"]
        .as_u64()
        .unwrap_or(MAX_DEV_TREE_DEPTH as u64)
        .min(MAX_DEV_TREE_DEPTH as u64);
    format!(
        "find {} -maxdepth {depth} {SKIPPED_TREE_DIRS} 2>/dev/null | head -n {MAX_DIR_ENTRIES}",
        shell_quote(root)
    )
}

/// `local_fs_delete_file`.
fn local_delete(args: &Value) -> ToolResult {
    let raw = args["path"].as_str().unwrap_or_default().trim();
    if raw.is_empty() {
        return Err("path is required.".to_owned());
    }
    let path = local_path(raw);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("Cannot delete {}: {error}", path.display()))?;
    if path.parent().is_none() || path == local_root() {
        return Err(format!("Refusing to delete {}.", path.display()));
    }
    if meta.is_dir() && !args["recursive"].as_bool().unwrap_or(false) {
        return Err(format!(
            "{} is a directory; pass recursive to delete it.",
            path.display()
        ));
    }
    let removed = if meta.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    };
    removed.map_err(|error| format!("Cannot delete {}: {error}", path.display()))?;
    Ok(format!("Deleted {}.", path.display()))
}

/// `local_fs_move_file`.
fn local_move(args: &Value) -> ToolResult {
    let from = local_path(args["path"].as_str().unwrap_or_default());
    let to = local_path(args["destination"].as_str().unwrap_or_default());
    if args["destination"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return Err("destination is required.".to_owned());
    }
    if !from.exists() {
        return Err(format!("{} does not exist.", from.display()));
    }
    if to.exists() {
        if !args["overwrite"].as_bool().unwrap_or(false) {
            return Err(format!(
                "{} already exists; pass overwrite to replace it.",
                to.display()
            ));
        }
        let removed = if to.is_dir() {
            std::fs::remove_dir_all(&to)
        } else {
            std::fs::remove_file(&to)
        };
        removed.map_err(|error| format!("Cannot replace {}: {error}", to.display()))?;
    }
    if let Some(parent) = to.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
    }
    if let Err(error) = std::fs::rename(&from, &to) {
        // Renaming across filesystems fails; copying and removing does the same job.
        if from.is_dir() {
            return Err(format!(
                "Cannot move {} to {}: {error}",
                from.display(),
                to.display()
            ));
        }
        std::fs::copy(&from, &to).map_err(|copy| {
            format!(
                "Cannot move {} to {}: {error}; {copy}",
                from.display(),
                to.display()
            )
        })?;
        std::fs::remove_file(&from)
            .map_err(|error| format!("Cannot remove {}: {error}", from.display()))?;
    }
    Ok(format!("Moved {} to {}.", from.display(), to.display()))
}

/// `local_fs_multi_edit`.
fn local_multi_edit(args: &Value) -> ToolResult {
    let path = local_path(args["path"].as_str().unwrap_or_default());
    let Some(edits) = args["edits"].as_array() else {
        return Err("edits must be an array of {oldText, newText} objects.".to_owned());
    };
    if edits.is_empty() {
        return Err("edits must not be empty.".to_owned());
    }
    let before = read_text_file(&path, MAX_FILE_READ_CHARS)?;
    let mut after = before.clone();
    for (index, edit) in edits.iter().enumerate() {
        let old = edit["oldText"].as_str().unwrap_or_default();
        let new = edit["newText"].as_str().unwrap_or_default();
        after = if edit["replaceAll"].as_bool().unwrap_or(false) {
            replace_every(&after, old, new)
        } else {
            replace_once(&after, old, new)
        }
        .map_err(|error| format!("Edit {}: {error}", index + 1))?;
    }
    if after == before {
        return Ok("No changes.".to_owned());
    }
    std::fs::write(&path, &after)
        .map_err(|error| format!("Cannot write {}: {error}", path.display()))?;
    Ok(format!(
        "{}\n{}",
        path.display(),
        diff_texts(&before, &after, DIFF_CONTEXT_LINES_DEFAULT)
    ))
}

/// Replace every occurrence of `old`, refusing an empty or missing match.
fn replace_every(text: &str, old: &str, new: &str) -> ToolResult {
    if old.is_empty() {
        return Err("oldText must not be empty.".to_owned());
    }
    if !text.contains(old) {
        return Err("oldText was not found in the file.".to_owned());
    }
    Ok(text.replace(old, new))
}

/// `local_fs_apply_patch`.
fn local_patch(args: &Value) -> ToolResult {
    let path = local_path(args["path"].as_str().unwrap_or_default());
    let patch = args["patch"]
        .as_str()
        .filter(|patch| !patch.trim().is_empty())
        .ok_or_else(|| "patch is required.".to_owned())?;
    let before = read_text_file(&path, MAX_FILE_READ_CHARS)?;
    let after = apply_unified_patch(&before, patch)?;
    if after == before {
        return Ok("The patch changed nothing.".to_owned());
    }
    std::fs::write(&path, &after)
        .map_err(|error| format!("Cannot write {}: {error}", path.display()))?;
    Ok(format!(
        "{}\n{}",
        path.display(),
        diff_texts(&before, &after, DIFF_CONTEXT_LINES_DEFAULT)
    ))
}

/// Apply the `@@` hunks of a unified diff, checking every context line.
pub fn apply_unified_patch(original: &str, patch: &str) -> ToolResult {
    let original_lines: Vec<&str> = original.split('\n').collect();
    let mut output: Vec<String> = Vec::new();
    let mut cursor = 0;
    let mut hunks = 0;
    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.starts_with("@@") {
            continue;
        }
        hunks += 1;
        let (old_start, old_len) = parse_hunk_header(line)?;
        let start = if old_len == 0 {
            old_start
        } else {
            old_start.saturating_sub(1)
        };
        if start < cursor || start > original_lines.len() {
            return Err(format!("Hunk {hunks} does not fit the file."));
        }
        output.extend(
            original_lines[cursor..start]
                .iter()
                .map(|line| (*line).to_owned()),
        );
        cursor = start;
        while let Some(body) = lines.peek().copied() {
            if body.starts_with("@@") || body.starts_with("diff ") {
                break;
            }
            lines.next();
            if body.is_empty() {
                continue;
            }
            let (marker, text) = body.split_at(1);
            match marker {
                " " => {
                    expect_line(&original_lines, cursor, text)?;
                    output.push(original_lines[cursor].to_owned());
                    cursor += 1;
                }
                "-" => {
                    expect_line(&original_lines, cursor, text)?;
                    cursor += 1;
                }
                "+" => output.push(text.to_owned()),
                // "\ No newline at end of file" carries no text of its own.
                "\\" => {}
                _ => return Err(format!("Hunk {hunks} has an invalid line: {body}")),
            }
        }
    }
    if hunks == 0 {
        return Err("The patch has no @@ hunks.".to_owned());
    }
    output.extend(
        original_lines[cursor..]
            .iter()
            .map(|line| (*line).to_owned()),
    );
    Ok(output.join("\n"))
}

/// The old-file range a hunk header names.
fn parse_hunk_header(line: &str) -> Result<(usize, usize), String> {
    let range = line
        .trim_start_matches("@@")
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("Invalid hunk header: {line}"))?;
    let range = range.trim_start_matches('-');
    let (start, len) = range.split_once(',').unwrap_or((range, "1"));
    let start = start
        .parse()
        .map_err(|_| format!("Invalid hunk header: {line}"))?;
    let len = len
        .parse()
        .map_err(|_| format!("Invalid hunk header: {line}"))?;
    Ok((start, len))
}

/// Require the original file to carry `expected` at `index`.
fn expect_line(lines: &[&str], index: usize, expected: &str) -> Result<(), String> {
    match lines.get(index) {
        Some(line) if line.trim_end_matches('\r') == expected.trim_end_matches('\r') => Ok(()),
        Some(line) => Err(format!(
            "Patch context does not match at line {}: expected {:?}, found {:?}.",
            index + 1,
            expected,
            line
        )),
        None => Err(format!(
            "The patch expects line {} but the file ends at {}.",
            index + 1,
            lines.len()
        )),
    }
}

/// Match a simple `*` glob against a file name.
fn glob_matches(pattern: &str, name: &str) -> bool {
    let mut regex = String::from("^");
    for character in pattern.chars() {
        match character {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            other if ".*+^${}()|[]\\".contains(other) => {
                regex.push('\\');
                regex.push(other);
            }
            other => regex.push(other),
        }
    }
    regex.push('$');
    Regex::new(&regex)
        .map(|compiled| compiled.is_match(name))
        .unwrap_or(false)
}

/// Walk `root`, calling `visit` for every file until it returns false.
fn walk_files(root: &Path, depth: usize, visit: &mut impl FnMut(&Path) -> bool) -> bool {
    if depth > 6 {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return true;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            if !walk_files(&path, depth + 1, visit) {
                return false;
            }
        } else if !visit(&path) {
            return false;
        }
    }
    true
}

/// `local_fs_replace_in_files`.
fn local_replace(args: &Value) -> ToolResult {
    let pattern = args["pattern"].as_str().unwrap_or_default();
    if pattern.is_empty() {
        return Err("pattern is required.".to_owned());
    }
    let replacement = args["replacement"].as_str().unwrap_or_default().to_owned();
    let case_sensitive = args["caseSensitive"].as_bool().unwrap_or(false);
    let source = if case_sensitive {
        pattern.to_owned()
    } else {
        format!("(?i){pattern}")
    };
    let regex = Regex::new(&source).map_err(|error| format!("Invalid pattern: {error}"))?;
    let root = local_path(args["path"].as_str().unwrap_or("."));
    let glob = args["glob"].as_str().map(str::to_owned);
    let dry_run = args["dryRun"].as_bool().unwrap_or(true);
    let limit = args["limit"]
        .as_u64()
        .map_or(50, |value| value as usize)
        .clamp(1, MAX_DEV_FIND_MATCHES);
    let mut changed: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut replacements = 0usize;
    {
        let mut visit = |path: &Path| {
            if let Some(glob) = &glob {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !glob_matches(glob, &name) {
                    return true;
                }
            }
            let Ok(bytes) = std::fs::read(path) else {
                return true;
            };
            if bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0) {
                return true;
            }
            let body = String::from_utf8_lossy(&bytes);
            let found = regex.find_iter(&body).count();
            if found == 0 {
                return true;
            }
            let updated = regex.replace_all(&body, replacement.as_str());
            if updated == body {
                return true;
            }
            if !dry_run && let Err(error) = std::fs::write(path, updated.as_bytes()) {
                failures.push(format!("{}: {error}", path.display()));
            }
            replacements += found;
            changed.push(format!("{}: {found}", path.display()));
            changed.len() < limit
        };
        if root.is_file() {
            visit(&root);
        } else {
            walk_files(&root, 0, &mut visit);
        }
    }
    if changed.is_empty() && failures.is_empty() {
        return Ok(format!("No matches for {pattern} under {}", root.display()));
    }
    let verb = if dry_run { "Would change" } else { "Changed" };
    let mut report = vec![format!(
        "{verb} {} files ({replacements} replacements) under {}.",
        changed.len(),
        root.display()
    )];
    report.extend(changed);
    if dry_run {
        report.push("Dry run: pass dryRun: false to write the changes.".to_owned());
    }
    report.extend(failures);
    Ok(report.join("\n"))
}

/// `local_fs_grep_search`: a ripgrep-style content search over this computer.
///
/// Hidden entries, dependencies and build output are skipped, like ripgrep's defaults,
/// and a match is reported as `path:line: text` with its context lines marked `-`.
fn local_grep(args: &Value) -> ToolResult {
    let pattern = args["pattern"].as_str().unwrap_or_default();
    if pattern.is_empty() {
        return Err("pattern is required.".to_owned());
    }
    let source = if args["caseSensitive"].as_bool().unwrap_or(false) {
        pattern.to_owned()
    } else {
        format!("(?i){pattern}")
    };
    let regex = Regex::new(&source).map_err(|error| format!("Invalid pattern: {error}"))?;
    let root = local_path(args["path"].as_str().unwrap_or("."));
    let glob = args["glob"].as_str().map(str::to_owned);
    let context = args["contextLines"]
        .as_u64()
        .unwrap_or(0)
        .min(DIFF_CONTEXT_LINES_MAX as u64) as usize;
    let limit = args["limit"]
        .as_u64()
        .map_or(DEV_SEARCH_MATCHES_DEFAULT, |value| value as usize)
        .clamp(1, MAX_DEV_GREP_MATCHES);
    let mut hits: Vec<String> = Vec::new();
    let mut searched = 0usize;
    let mut visit = |path: &Path| {
        if let Some(glob) = &glob {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !glob_matches(glob, &name) {
                return true;
            }
        }
        let Ok(bytes) = std::fs::read(path) else {
            return true;
        };
        if bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0) {
            return true;
        }
        searched += 1;
        let body = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = body.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            let from = index.saturating_sub(context);
            let to = (index + context).min(lines.len().saturating_sub(1));
            for (offset, text) in lines[from..=to].iter().enumerate() {
                let number = from + offset + 1;
                let separator = if from + offset == index { ':' } else { '-' };
                hits.push(format!("{}{separator}{number}{separator}{text}", path.display()));
            }
            if hits.len() >= limit {
                return false;
            }
        }
        true
    };
    if root.is_file() {
        visit(&root);
    } else {
        walk_files(&root, 0, &mut visit);
    }
    if hits.is_empty() {
        return Ok(format!("No matches for {pattern} under {}.", root.display()));
    }
    let mut report = hits.join("\n");
    if hits.len() >= limit {
        report.push_str(&format!("\n[Stopped after {limit} matching lines]"));
    }
    report.push_str(&format!("\n[{searched} files searched]"));
    Ok(report)
}

/// Run `local_fs_run_command` on this machine, killing it at the timeout.
pub fn run_local_command(args: &Value) -> ToolResult {
    let command = args["command"]
        .as_str()
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .ok_or_else(|| "The command argument was missing or empty.".to_owned())?
        .to_owned();
    let cwd = local_path(args["cwd"].as_str().unwrap_or("."));
    let timeout = args["timeoutMs"]
        .as_u64()
        .unwrap_or(DEV_COMMAND_TIMEOUT_DEFAULT_MS)
        .clamp(DEV_COMMAND_TIMEOUT_MIN_MS, MAX_DEV_COMMAND_TIMEOUT_MS);
    let mut child = shell_command(&command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot run `{command}` in {}: {error}", cwd.display()))?;
    let stdout = child.stdout.take().map(read_pipe);
    let stderr = child.stderr.take().map(read_pipe);
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(DEV_COMMAND_POLL_MS));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("`{command}` did not finish within {timeout} ms."));
            }
            Err(error) => return Err(format!("Cannot wait for `{command}`: {error}")),
        }
    };
    let out = stdout.and_then(|pipe| pipe.join().ok()).unwrap_or_default();
    let err = stderr.and_then(|pipe| pipe.join().ok()).unwrap_or_default();
    let mut body = String::from_utf8_lossy(&out).into_owned();
    let error_text = String::from_utf8_lossy(&err);
    if !error_text.is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&error_text);
    }
    if body.trim().is_empty() {
        body = "(no output)".to_owned();
    }
    let code = status
        .code()
        .map_or_else(|| "none".to_owned(), |code| code.to_string());
    Ok(format!(
        "{}\n[exit code {code}]",
        clamp(&body, MAX_DEV_COMMAND_OUTPUT_CHARS)
    ))
}

/// Collect a child's output on its own thread so a full pipe cannot stall it.
fn read_pipe(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    })
}

/// The shell a local command runs through.
fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut shell = Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    }
    #[cfg(not(windows))]
    {
        let mut shell = Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    }
}

/// ISO-8601 UTC now, without a date-time dependency.
fn chrono_like_iso(now_ms: u64) -> String {
    let seconds = now_ms / 1000;
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = wassh_core::time::civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// The shell command a remote read-only tool runs.
pub fn remote_shell_command(name: &str, args: &Value) -> Option<String> {
    match name {
        TOOL_SYSTEM_OVERVIEW => Some(
            [
                "printf 'os\\n'; (cat /etc/os-release 2>/dev/null || sw_vers 2>/dev/null)",
                "printf '\\nkernel\\n'; uname -a",
                "printf '\\nuptime\\n'; uptime 2>/dev/null || cat /proc/uptime",
                "printf '\\nload\\n'; cat /proc/loadavg 2>/dev/null",
                "printf '\\ncpu\\n'; (nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null)",
                "printf '\\nmemory\\n'; (head -n 5 /proc/meminfo 2>/dev/null || vm_stat 2>/dev/null)",
                "printf '\\ndisk\\n'; df -h / 2>/dev/null",
                "printf '\\nusers\\n'; who 2>/dev/null | head -n 10",
            ]
            .join("\n"),
        ),
        TOOL_SYSTEM_PROCESSES => {
            let limit = args["limit"]
                .as_u64()
                .map_or(MAX_SYSTEM_PROCESS_ROWS_DEFAULT, |value| value as usize)
                .clamp(1, MAX_SYSTEM_PROCESS_ROWS_LIMIT);
            let sort = match args["sort"].as_str().unwrap_or("cpu") {
                "mem" => "-pmem",
                "pid" => "pid",
                "time" => "-time",
                _ => "-pcpu",
            };
            Some(format!(
                "ps -eo pid=,user=,pcpu=,pmem=,stat=,time=,args= --sort={sort} 2>/dev/null | head -n {limit} \
                 || ps -Ao pid,user,%cpu,%mem,stat,time,args -r | head -n {limit}"
            ))
        }
        TOOL_SYSTEM_DISK => {
            let all = args["includeAllFilesystems"].as_bool().unwrap_or(false);
            let inodes = args["includeInodes"].as_bool().unwrap_or(false);
            let mut command = if all {
                "df -h 2>/dev/null".to_owned()
            } else {
                "df -h -x tmpfs -x devtmpfs -x squashfs -x overlay 2>/dev/null || df -h 2>/dev/null".to_owned()
            };
            if inodes {
                command.push_str("; printf '\\ninodes\\n'; df -i 2>/dev/null");
            }
            Some(command)
        }
        TOOL_SYSTEM_MEMORY => {
            let consumers = args["topConsumers"]
                .as_u64()
                .map_or(MAX_MEMORY_CONSUMERS_DEFAULT, |value| value as usize)
                .clamp(1, MAX_MEMORY_CONSUMERS_LIMIT);
            Some(format!(
                "cat /proc/meminfo 2>/dev/null | head -n 20; printf '\\nswap\\n'; (swapon --show 2>/dev/null || true); \
                 printf '\\nconsumers\\n'; ps -eo pid=,user=,pmem=,rss=,args= --sort=-pmem 2>/dev/null | head -n {consumers}"
            ))
        }
        _ => None,
    }
}

/// Key names `terminal_send_keys` understands, as the bytes they send.
pub fn key_sequence(name: &str) -> Option<&'static str> {
    match name.trim().to_lowercase().as_str() {
        "enter" | "return" => Some("\r"),
        "tab" => Some("\t"),
        "esc" | "escape" => Some("\u{1b}"),
        "space" => Some(" "),
        "backspace" => Some("\u{7f}"),
        "delete" | "del" => Some("\u{1b}[3~"),
        "up" => Some("\u{1b}[A"),
        "down" => Some("\u{1b}[B"),
        "right" => Some("\u{1b}[C"),
        "left" => Some("\u{1b}[D"),
        "home" => Some("\u{1b}[H"),
        "end" => Some("\u{1b}[F"),
        "pageup" | "pgup" => Some("\u{1b}[5~"),
        "pagedown" | "pgdn" => Some("\u{1b}[6~"),
        "ctrl-a" | "c-a" => Some("\u{1}"),
        "ctrl-b" => Some("\u{2}"),
        "ctrl-c" => Some("\u{3}"),
        "ctrl-d" => Some("\u{4}"),
        "ctrl-e" => Some("\u{5}"),
        "ctrl-k" => Some("\u{b}"),
        "ctrl-l" => Some("\u{c}"),
        "ctrl-r" => Some("\u{12}"),
        "ctrl-u" => Some("\u{15}"),
        "ctrl-w" => Some("\u{17}"),
        "ctrl-z" => Some("\u{1a}"),
        _ => None,
    }
}

/// The bytes for a comma-separated key list.
pub fn keys_sequence(keys: &str) -> Result<String, String> {
    let mut bytes = String::new();
    for name in keys.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        match key_sequence(name) {
            Some(sequence) => bytes.push_str(sequence),
            None => return Err(format!("Unknown key \"{name}\".")),
        }
    }
    if bytes.is_empty() {
        return Err("No key names given.".to_owned());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reference_tool_has_a_schema() {
        let schemas = specs(&Permissions::default());
        assert_eq!(schemas.len(), 38);
        let names: Vec<&str> = schemas
            .iter()
            .map(|schema| schema["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"run_command"));
        assert!(names.contains(&"run_sudo"));
        assert!(names.contains(&"dev_grep_search"));
        assert!(names.contains(&"local_fs_grep_search"));
        assert!(names.contains(&"local_fs_run_command"));
        assert!(names.contains(&"local_fs_apply_patch"));
        assert!(names.contains(&"local_fs_replace_in_files"));
        assert!(names.contains(&"terminal_send_keys"));
        assert!(names.contains(&"system_memory_report"));
        assert!(names.contains(&"rag_search"));
        let ask_user = schemas
            .iter()
            .find(|schema| schema["name"] == TOOL_ASK_USER)
            .expect("ask_user schema");
        assert_eq!(ask_user["parameters"]["anyOf"][0]["required"][0], "questions");
        assert_eq!(ask_user["parameters"]["anyOf"][1]["required"][0], "text");
        // Every schema is an object with a properties map.
        for schema in &schemas {
            assert_eq!(schema["parameters"]["type"], "object", "{schema}");
            assert!(is_tool(schema["name"].as_str().unwrap()));
        }
    }

    #[test]
    fn html_becomes_readable_text() {
        let html = "<html><head><style>a{}</style><script>var x=1</script></head>\
                    <body><h1>Title</h1><p>Some&nbsp;text &amp; more</p><br>tail</body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Some text & more"));
        assert!(!text.contains("var x"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn a_diff_marks_added_and_removed_lines() {
        let old = "one\ntwo\nthree\n";
        let new = "one\nTWO\nthree\nfour\n";
        let diff = diff_texts(old, new, 1);
        assert!(diff.contains("-   1 two"), "{diff}");
        assert!(diff.contains("+   1 TWO"), "{diff}");
        assert!(diff.contains("+   3 four"), "{diff}");
        assert_eq!(diff_texts("same\n", "same\n", 1), "No differences.");
    }

    #[test]
    fn replacing_refuses_ambiguity() {
        assert_eq!(
            replace_once("a b a", "a", "c").unwrap_err(),
            "oldText appears 2 times; include more surrounding text so the change is unambiguous."
        );
        assert_eq!(
            replace_once("abc", "x", "y").unwrap_err(),
            "oldText was not found in the file."
        );
        assert_eq!(replace_once("abc", "b", "B").unwrap(), "aBc");
        assert!(replace_once("abc", "", "x").is_err());
    }

    #[test]
    fn key_names_become_escape_sequences() {
        assert_eq!(keys_sequence("enter").unwrap(), "\r");
        assert_eq!(keys_sequence("ctrl-c, up").unwrap(), "\u{3}\u{1b}[A");
        assert!(keys_sequence("nope").is_err());
        assert!(keys_sequence("  ").is_err());
    }

    #[test]
    fn the_time_tool_reports_a_parseable_stamp() {
        let json: Value = serde_json::from_str(
            &run_local(TOOL_CURRENT_TIME, &json!({}), 1_700_000_000_000).unwrap(),
        )
        .unwrap();
        assert_eq!(json["timestamp"], 1_700_000_000_000u64);
        assert!(json["iso"].as_str().unwrap().starts_with("2023-11-14T"));
    }

    #[test]
    fn local_writes_and_reads_round_trip() {
        let dir = std::env::temp_dir().join(format!("wassh-ai-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        let path = file.display().to_string();
        let _ = std::fs::remove_file(&file);

        // A fresh write succeeds; a second one needs overwrite.
        run_local(
            TOOL_LOCAL_WRITE,
            &json!({ "path": path, "content": "hello" }),
            0,
        )
        .unwrap();
        assert!(
            run_local(
                TOOL_LOCAL_WRITE,
                &json!({ "path": path, "content": "x" }),
                0
            )
            .is_err()
        );
        run_local(
            TOOL_LOCAL_WRITE,
            &json!({ "path": path, "content": "hello again", "overwrite": true }),
            0,
        )
        .unwrap();
        assert_eq!(
            run_local(TOOL_LOCAL_READ, &json!({ "path": path }), 0).unwrap(),
            "hello again"
        );

        // Editing reports the change.
        let diff = run_local(
            TOOL_LOCAL_EDIT,
            &json!({ "path": path, "oldText": "again", "newText": "once more" }),
            0,
        )
        .unwrap();
        assert!(diff.contains("+   0 hello once more"), "{diff}");

        // Listing finds it again.
        let list = run_local(
            TOOL_LOCAL_LIST,
            &json!({ "path": dir.display().to_string() }),
            0,
        )
        .unwrap();
        assert!(list.contains("note.txt"), "{list}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_unified_patch_applies_its_hunks() {
        let before = "one\ntwo\nthree\nfour\n";
        let patch = "--- a\n+++ b\n@@ -1,4 +1,5 @@\n one\n-two\n+TWO\n three\n four\n+five\n";
        assert_eq!(
            apply_unified_patch(before, patch).unwrap(),
            "one\nTWO\nthree\nfour\nfive\n"
        );

        // An empty old range inserts at the top.
        let insert = "@@ -0,0 +1,2 @@\n+first\n+second\n";
        assert_eq!(
            apply_unified_patch("one\n", insert).unwrap(),
            "first\nsecond\none\n"
        );

        // Context that does not match, and a patch with no hunks, are refused.
        assert!(apply_unified_patch("one\n", "@@ -1,1 +1,1 @@\n-two\n+TWO\n").is_err());
        assert!(apply_unified_patch("one\n", "no hunks").is_err());
    }

    #[test]
    fn a_multi_edit_applies_each_replacement_in_order() {
        let dir = std::env::temp_dir().join(format!("wassh-ai-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        let path = file.display().to_string();
        std::fs::write(&file, "alpha beta gamma\n").unwrap();
        let diff = run_local(
            TOOL_LOCAL_MULTI_EDIT,
            &json!({ "path": path, "edits": [
                { "oldText": "alpha", "newText": "ALPHA" },
                { "oldText": "beta", "newText": "BETA", "replaceAll": true }
            ]}),
            0,
        )
        .unwrap();
        assert!(diff.contains("+   0 ALPHA BETA gamma"), "{diff}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "ALPHA BETA gamma\n"
        );

        // A later edit that cannot find its text fails the whole call.
        let error = run_local(
            TOOL_LOCAL_MULTI_EDIT,
            &json!({ "path": path, "edits": [
                { "oldText": "ALPHA", "newText": "A" },
                { "oldText": "missing", "newText": "x" }
            ]}),
            0,
        )
        .unwrap_err();
        assert!(error.starts_with("Edit 2:"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "ALPHA BETA gamma\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_local_grep_reports_paths_lines_and_context() {
        let dir = std::env::temp_dir().join(format!("wassh-ai-grep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        let source = dir.join("code.rs");
        std::fs::write(
            &source,
            "fn main() {\n    let value = 1;\n    run(value);\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("nested/other.rs"), "// let value = 2;\n").unwrap();
        // ripgrep-style: dependencies and hidden files stay out of the way.
        std::fs::write(dir.join("node_modules/dep.js"), "let value = 3;\n").unwrap();
        std::fs::write(dir.join(".hidden.rs"), "let value = 4;\n").unwrap();

        let output = run_local(
            TOOL_LOCAL_GREP,
            &json!({ "pattern": "let value", "path": dir.display().to_string() }),
            0,
        )
        .unwrap();
        assert!(output.contains("code.rs:2:    let value = 1;"), "{output}");
        assert!(output.contains("other.rs:1:// let value = 2;"), "{output}");
        assert!(!output.contains("node_modules"), "{output}");
        assert!(!output.contains(".hidden"), "{output}");
        assert!(output.ends_with("[2 files searched]"), "{output}");

        // A glob and context lines narrow it down the way ripgrep's flags do.
        let context = run_local(
            TOOL_LOCAL_GREP,
            &json!({ "pattern": "run\\(", "path": dir.display().to_string(),
                     "glob": "code.rs", "contextLines": 1 }),
            0,
        )
        .unwrap();
        assert!(context.contains("code.rs:3:    run(value);"), "{context}");
        assert!(context.contains("code.rs-2-    let value = 1;"), "{context}");
        assert!(context.contains("code.rs-4-}"), "{context}");

        // Case follows the flag, and a bad pattern is refused rather than guessed at.
        assert!(
            run_local(
                TOOL_LOCAL_GREP,
                &json!({ "pattern": "LET VALUE", "path": dir.display().to_string() }),
                0
            )
            .unwrap()
            .contains("let value = 1;")
        );
        assert!(
            run_local(
                TOOL_LOCAL_GREP,
                &json!({ "pattern": "LET VALUE", "path": dir.display().to_string(),
                         "caseSensitive": true }),
                0
            )
            .unwrap()
            .starts_with("No matches")
        );
        assert!(run_local(TOOL_LOCAL_GREP, &json!({ "pattern": "(" }), 0).is_err());
        assert!(run_local(TOOL_LOCAL_GREP, &json!({}), 0).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_files_can_be_moved_and_deleted() {
        let dir = std::env::temp_dir().join(format!("wassh-ai-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("a.txt");
        let to = dir.join("nested/b.txt");
        std::fs::write(&from, "text").unwrap();
        run_local(
            TOOL_LOCAL_MOVE,
            &json!({ "path": from.display().to_string(), "destination": to.display().to_string() }),
            0,
        )
        .unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "text");

        // An existing destination needs overwrite, and a directory needs recursive.
        assert!(
            run_local(
                TOOL_LOCAL_MOVE,
                &json!({ "path": to.display().to_string(), "destination": to.display().to_string() }),
                0,
            )
            .is_err()
        );
        let nested = dir.join("nested");
        assert!(
            run_local(
                TOOL_LOCAL_DELETE,
                &json!({ "path": nested.display().to_string() }),
                0,
            )
            .is_err()
        );
        run_local(
            TOOL_LOCAL_DELETE,
            &json!({ "path": nested.display().to_string(), "recursive": true }),
            0,
        )
        .unwrap();
        assert!(!nested.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_reports_replacements_without_writing() {
        let dir = std::env::temp_dir().join(format!("wassh-ai-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("code.rs");
        std::fs::write(&file, "let value = 1;\nlet other = 1;\n").unwrap();
        let report = run_local(
            TOOL_LOCAL_REPLACE,
            &json!({ "pattern": "= 1", "replacement": "= 2", "path": dir.display().to_string(), "glob": "*.rs" }),
            0,
        )
        .unwrap();
        assert!(
            report.contains("Would change 1 files (2 replacements)"),
            "{report}"
        );
        assert!(report.contains("code.rs: 2"), "{report}");
        assert!(std::fs::read_to_string(&file).unwrap().contains("= 1"));

        run_local(
            TOOL_LOCAL_REPLACE,
            &json!({ "pattern": "= 1", "replacement": "= 2", "path": dir.display().to_string(), "glob": "*.rs", "dryRun": false }),
            0,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "let value = 2;\nlet other = 2;\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_local_command_returns_output_and_exit_code() {
        let output = run_local_command(&json!({ "command": "printf hi" })).unwrap();
        assert!(output.starts_with("hi\n"), "{output}");
        assert!(output.contains("[exit code 0]"), "{output}");
        let failed = run_local_command(&json!({ "command": "exit 3" })).unwrap();
        assert!(failed.contains("[exit code 3]"), "{failed}");
        assert!(run_local_command(&json!({ "command": "  " })).is_err());
    }

    #[test]
    fn the_grep_command_prefers_ripgrep_and_falls_back_to_grep() {
        let grep = dev_grep_command(
            &json!({ "pattern": "fn main", "glob": "*.rs", "contextLines": 2, "limit": 7 }),
            "/srv/app",
        );
        assert!(grep.starts_with("if command -v rg >/dev/null 2>&1; then "), "{grep}");
        assert!(
            grep.contains(
                "rg --line-number --no-heading --color never -i -g '*.rs' -C 2 -e 'fn main' '/srv/app'"
            ),
            "{grep}"
        );
        assert!(
            grep.contains(
                "grep -rnI -i --include='*.rs' -C 2 --exclude-dir={.git,node_modules,dist,build,target} \
                 -e 'fn main' '/srv/app'"
            ),
            "{grep}"
        );
        assert!(!grep.is_empty() && grep.matches("head -n 7").count() == 2, "{grep}");

        // Case-sensitive searches pass no `-i`, and the cap is enforced both ways.
        let case = dev_grep_command(
            &json!({ "pattern": "main", "caseSensitive": true, "limit": 500 }),
            "/srv",
        );
        assert!(case.contains("rg --line-number --no-heading --color never -e"), "{case}");
        assert!(case.contains("grep -rnI --exclude-dir"), "{case}");
        assert_eq!(
            case.matches(&format!("head -n {MAX_DEV_GREP_MATCHES}")).count(),
            2,
            "{case}"
        );
    }

    #[test]
    fn the_developer_tree_tools_are_shell_lines_for_the_host() {
        let find = dev_find_command(&json!({ "namePattern": "*.toml", "limit": 3 }), "/srv");
        assert!(find.contains("-maxdepth 8"), "{find}");
        assert!(find.contains("-iname '*.toml'"), "{find}");
        assert!(find.ends_with("head -n 3"), "{find}");

        let tree = dev_tree_command(&json!({ "maxDepth": 9 }), "/srv");
        assert!(
            tree.contains(&format!("-maxdepth {MAX_DEV_TREE_DEPTH}")),
            "{tree}"
        );
        assert!(tree.ends_with(&format!("head -n {MAX_DIR_ENTRIES}")), "{tree}");
    }

    #[test]
    fn the_system_probes_are_shell_lines() {
        let command = remote_shell_command(TOOL_SYSTEM_PROCESSES, &json!({ "limit": 7 })).unwrap();
        assert!(command.contains("head -n 7"));
        assert!(command.contains("--sort=-pcpu"));
        let sort = remote_shell_command(TOOL_SYSTEM_PROCESSES, &json!({ "sort": "mem" })).unwrap();
        assert!(sort.contains("--sort=-pmem"));
        assert!(
            remote_shell_command(TOOL_SYSTEM_OVERVIEW, &json!({}))
                .unwrap()
                .contains("uname -a")
        );
        assert!(
            remote_shell_command(TOOL_SYSTEM_DISK, &json!({ "includeInodes": true }))
                .unwrap()
                .contains("df -i")
        );
        assert!(
            remote_shell_command(TOOL_SYSTEM_MEMORY, &json!({}))
                .unwrap()
                .contains("/proc/meminfo")
        );
        assert!(remote_shell_command(TOOL_RUN_COMMAND, &json!({})).is_none());
    }

    #[test]
    fn session_tools_are_marked_as_such() {
        assert!(needs_session(TOOL_RUN_COMMAND));
        assert!(needs_session(TOOL_SYSTEM_OVERVIEW));
        assert!(needs_session(TOOL_TERMINAL_READ));
        assert!(needs_session(TOOL_DEV_VIEW));
        assert!(needs_session(TOOL_DEV_GREP));
        assert!(needs_session(TOOL_DEV_FIND));
        assert!(needs_session(TOOL_DEV_LIST));
        assert!(needs_session(TOOL_DEV_CREATE));
        assert!(needs_session(TOOL_DEV_EDIT));
        // A diff of two texts needs no file, and says so itself when it needs one.
        assert!(!needs_session(TOOL_DEV_DIFF));
        assert!(!needs_session(TOOL_WEB_FETCH));
        assert!(!needs_session(TOOL_LOCAL_READ));
        assert!(!needs_session(TOOL_LOCAL_RUN));
        assert!(!needs_session(TOOL_LOCAL_PATCH));
        assert!(is_read_only(TOOL_REMOTE_LIST));
        assert!(!is_read_only(TOOL_REMOTE_WRITE));
        assert!(!is_read_only(TOOL_RUN_COMMAND));
    }
}
