//! The agent's tools: schemas, local executors, and the shell commands the
//! remote ones run.
//!
//! Port of `src/plugins/builtins/ai-agent/tools.ts`. The reference splits its
//! tools into ones that run in-process (files under the local root, web fetch,
//! search, time) and ones that need the session (commands, SFTP, terminal,
//! system probes). This module keeps the same split: [`run_local`] answers
//! immediately, [`remote_shell_command`] returns a shell line for the session to
//! run, and the SFTP/terminal tools are driven from `agent.rs`.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use super::protocol::{
    TOOL_ASK_USER, TOOL_DEV_CREATE, TOOL_DEV_DIFF, TOOL_DEV_EDIT,
    TOOL_DEV_FIND, TOOL_DEV_GREP, TOOL_DEV_LIST, TOOL_DEV_VIEW, TOOL_CURRENT_TIME,
    TOOL_LOCAL_EDIT, TOOL_LOCAL_LIST, TOOL_LOCAL_READ, TOOL_LOCAL_WRITE,
    TOOL_RAG_SEARCH, TOOL_REMOTE_DELETE, TOOL_REMOTE_EDIT, TOOL_REMOTE_LIST, TOOL_REMOTE_READ,
    TOOL_REMOTE_WRITE, TOOL_RUN_COMMAND, TOOL_RUN_SUDO, TOOL_SYSTEM_DISK, TOOL_SYSTEM_MEMORY,
    TOOL_SYSTEM_OVERVIEW, TOOL_SYSTEM_PROCESSES, TOOL_TERMINAL_KEYS, TOOL_TERMINAL_READ,
    TOOL_TERMINAL_WAIT, TOOL_TERMINAL_WRITE, TOOL_WEB_FETCH, TOOL_WEB_SEARCH,
};

/// Longest body a fetch may return.
pub const MAX_FETCH_CHARS: usize = 32_000;
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
/// Depth a recursive `dev_list_directory` walks.
pub const MAX_DEV_TREE_DEPTH: usize = 3;
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
        description: "Ask the user a question and wait for their answer.",
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
        name: TOOL_LOCAL_EDIT,
        description: "Replace a block of text in a file on this computer.",
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
        description: "View a local file with line numbers, optionally a line range.",
    },
    ToolSpec {
        name: TOOL_DEV_EDIT,
        description: "Replace text in a local file and report the change as a diff.",
    },
    ToolSpec {
        name: TOOL_DEV_CREATE,
        description: "Create a local file, optionally refusing to overwrite.",
    },
    ToolSpec {
        name: TOOL_DEV_LIST,
        description: "List a local directory, optionally as a tree.",
    },
    ToolSpec {
        name: TOOL_DEV_GREP,
        description: "Search local files for a regular expression and show matching lines.",
    },
    ToolSpec {
        name: TOOL_DEV_FIND,
        description: "Find local files whose name matches a glob.",
    },
    ToolSpec {
        name: TOOL_DEV_DIFF,
        description: "Show the difference between two versions of a local file.",
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
        json!({
            "name": self.name,
            "description": self.description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required
            }
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
            json!({ "text": string("The question to ask the user.") }),
            json!(["text"]),
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
        TOOL_REMOTE_LIST | TOOL_LOCAL_LIST => {
            (json!({ "path": string("Directory to list.") }), json!(["path"]))
        }
        TOOL_REMOTE_DELETE => {
            (json!({ "path": string("File or directory to delete.") }), json!(["path"]))
        }
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
        TOOL_DEV_GREP => (
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

/// Every tool schema, ready for the provider.
pub fn specs() -> Vec<Value> {
    TOOL_SPECS.iter().map(|spec| spec.schema()).collect()
}

/// Whether a tool exists.
pub fn is_tool(name: &str) -> bool {
    TOOL_SPECS.iter().any(|spec| spec.name == name)
}

/// Whether a tool needs the connected session.
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
    format!("{kept}\n\n[Content truncated at {limit} characters of {}]", text.chars().count())
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
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    if bytes.iter().take(4096).any(|byte| *byte == 0) {
        return Err(format!("{} looks like a binary file.", path.display()));
    }
    Ok(clamp(&String::from_utf8_lossy(&bytes), limit))
}

/// Write a text file on this machine, creating parent directories.
fn write_text_file(path: &Path, content: &str, overwrite: bool) -> ToolResult {
    if path.exists() && !overwrite {
        return Err(format!("{} already exists; pass overwrite to replace it.", path.display()));
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
    text = strip(r"(?i)</(p|div|h[1-6]|li|tr|blockquote|article|section|pre)>", "\n", &text);
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
        TOOL_LOCAL_READ | TOOL_DEV_VIEW => {
            let path = local_path(&text_argument("path").unwrap_or_default());
            let limit = if name == TOOL_LOCAL_READ {
                args["maxChars"].as_u64().map_or(32_000, |value| value as usize)
                    .clamp(1_000, MAX_FILE_READ_CHARS)
            } else {
                MAX_FILE_READ_CHARS
            };
            let body = read_text_file(&path, limit)?;
            if name == TOOL_DEV_VIEW {
                let start = args["startLine"].as_u64().unwrap_or(1).max(1) as usize;
                let max_lines = args["maxLines"]
                    .as_u64()
                    .map_or(MAX_DEV_VIEW_LINES_DEFAULT, |value| value as usize);
                let end = args["endLine"]
                    .as_u64()
                    .map_or(start + max_lines - 1, |value| value as usize);
                let numbered: Vec<String> = body
                    .lines()
                    .enumerate()
                    .skip(start - 1)
                    .take(end.saturating_sub(start) + 1)
                    .map(|(index, line)| format!("{:>5}\t{line}", index + 1))
                    .collect();
                Ok(format!("{}\n{}", path.display(), numbered.join("\n")))
            } else {
                Ok(body)
            }
        }
        TOOL_LOCAL_WRITE | TOOL_DEV_CREATE => {
            let path = local_path(&text_argument("path").unwrap_or_default());
            let content = text_argument("content").unwrap_or_default();
            let overwrite = args["overwrite"].as_bool().unwrap_or(false);
            write_text_file(&path, &content, overwrite)
        }
        TOOL_LOCAL_EDIT | TOOL_DEV_EDIT => {
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
        TOOL_LOCAL_LIST | TOOL_DEV_LIST => {
            let path = local_path(args["path"].as_str().unwrap_or("."));
            let recursive = args["recursive"].as_bool().unwrap_or(false);
            let depth = args["maxDepth"]
                .as_u64()
                .map_or(if recursive { MAX_DEV_TREE_DEPTH } else { 0 }, |value| {
                    value as usize
                })
                .min(MAX_DEV_TREE_DEPTH);
            let mut entries = Vec::new();
            list_dir(&path, &mut entries, 0, depth)?;
            Ok(format!("{}\n{}", path.display(), entries.join("\n")))
        }
        TOOL_DEV_GREP => dev_grep(args),
        TOOL_DEV_FIND => dev_find(args),
        TOOL_DEV_DIFF => {
            let path = local_path(&text_argument("path").unwrap_or_default());
            let context = args["contextLines"]
                .as_u64()
                .map_or(DIFF_CONTEXT_LINES_DEFAULT, |value| value as usize);
            let old = match text_argument("oldText") {
                Some(text) => text,
                None => read_text_file(&path, MAX_FILE_READ_CHARS)?,
            };
            let new = text_argument("newText")
                .or_else(|| text_argument("patch"))
                .ok_or_else(|| "Pass newText or patch to diff against.".to_owned())?;
            Ok(diff_texts(&old, &new, context))
        }
        other => Err(format!("{other} is not a local tool.")),
    }
}

/// Match a simple `*` glob against a file name.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
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
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
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

/// `dev_grep_search`.
fn dev_grep(args: &Value) -> ToolResult {
    let pattern = args["pattern"].as_str().unwrap_or_default();
    if pattern.is_empty() {
        return Err("pattern is required.".to_owned());
    }
    let case_sensitive = args["caseSensitive"].as_bool().unwrap_or(false);
    let source = if case_sensitive {
        pattern.to_owned()
    } else {
        format!("(?i){pattern}")
    };
    let regex = Regex::new(&source).map_err(|error| format!("Invalid pattern: {error}"))?;
    let root = local_path(args["path"].as_str().unwrap_or("."));
    let glob = args["glob"].as_str().map(str::to_owned);
    let context = args["contextLines"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"]
        .as_u64()
        .map_or(50, |value| value as usize)
        .clamp(1, MAX_DEV_GREP_MATCHES);
    let mut hits: Vec<String> = Vec::new();
    let mut searched = 0usize;
    let mut visit = |path: &Path| {
        if let Some(glob) = &glob {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if !glob_matches(glob, &name) {
                return true;
            }
        }
        let Ok(bytes) = std::fs::read(path) else {
            return true;
        };
        if bytes.iter().take(2048).any(|byte| *byte == 0) {
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
                let marker = if from + offset == index { ':' } else { '-' };
                hits.push(format!("{}:{number}{marker}{text}", path.display()));
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
        return Ok(format!("No matches for {pattern} under {}", root.display()));
    }
    Ok(format!(
        "{}\n\n[{} lines searched, {} matches]",
        hits.join("\n"),
        searched,
        hits.len()
    ))
}

/// `dev_find_files`.
fn dev_find(args: &Value) -> ToolResult {
    let pattern = args["namePattern"].as_str().unwrap_or_default();
    if pattern.is_empty() {
        return Err("namePattern is required.".to_owned());
    }
    let root = local_path(args["path"].as_str().unwrap_or("."));
    let limit = args["limit"]
        .as_u64()
        .map_or(50, |value| value as usize)
        .clamp(1, MAX_DEV_FIND_MATCHES);
    let mut found: Vec<String> = Vec::new();
    let mut visit = |path: &Path| {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        if glob_matches(pattern, &name) {
            found.push(path.display().to_string());
        }
        found.len() < limit
    };
    walk_files(&root, 0, &mut visit);
    if found.is_empty() {
        return Ok(format!("No files matching {pattern} under {}", root.display()));
    }
    Ok(found.join("\n"))
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
        let schemas = specs();
        assert_eq!(schemas.len(), 31);
        let names: Vec<&str> = schemas.iter().map(|schema| schema["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"run_command"));
        assert!(names.contains(&"run_sudo"));
        assert!(names.contains(&"dev_grep_search"));
        assert!(names.contains(&"terminal_send_keys"));
        assert!(names.contains(&"system_memory_report"));
        assert!(names.contains(&"rag_search"));
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
        assert_eq!(replace_once("a b a", "a", "c").unwrap_err(), "oldText appears 2 times; include more surrounding text so the change is unambiguous.");
        assert_eq!(replace_once("abc", "x", "y").unwrap_err(), "oldText was not found in the file.");
        assert_eq!(replace_once("abc", "b", "B").unwrap(), "aBc");
        assert!(replace_once("abc", "", "x").is_err());
    }

    #[test]
    fn globs_match_file_names() {
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(!glob_matches("*.rs", "main.rss"));
        assert!(glob_matches("lib?.txt", "lib1.txt"));
        assert!(!glob_matches("lib?.txt", "lib10.txt"));
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
        let json: Value =
            serde_json::from_str(&run_local(TOOL_CURRENT_TIME, &json!({}), 1_700_000_000_000).unwrap())
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
        run_local(TOOL_LOCAL_WRITE, &json!({ "path": path, "content": "hello" }), 0).unwrap();
        assert!(run_local(TOOL_LOCAL_WRITE, &json!({ "path": path, "content": "x" }), 0).is_err());
        run_local(
            TOOL_LOCAL_WRITE,
            &json!({ "path": path, "content": "hello again", "overwrite": true }),
            0,
        )
        .unwrap();
        assert_eq!(run_local(TOOL_LOCAL_READ, &json!({ "path": path }), 0).unwrap(), "hello again");

        // Editing reports the change.
        let diff = run_local(
            TOOL_DEV_EDIT,
            &json!({ "path": path, "oldText": "again", "newText": "once more" }),
            0,
        )
        .unwrap();
        assert!(diff.contains("+   0 hello once more"), "{diff}");

        // Viewing numbers the lines.
        let view = run_local(TOOL_DEV_VIEW, &json!({ "path": path }), 0).unwrap();
        assert!(view.contains("    1\thello once more"), "{view}");

        // Listing and grepping find it again.
        let list = run_local(TOOL_LOCAL_LIST, &json!({ "path": dir.display().to_string() }), 0).unwrap();
        assert!(list.contains("note.txt"), "{list}");
        let grep = run_local(
            TOOL_DEV_GREP,
            &json!({ "pattern": "once more", "path": dir.display().to_string() }),
            0,
        )
        .unwrap();
        assert!(grep.contains("note.txt"), "{grep}");
        let find = run_local(
            TOOL_DEV_FIND,
            &json!({ "namePattern": "*.txt", "path": dir.display().to_string() }),
            0,
        )
        .unwrap();
        assert!(find.contains("note.txt"), "{find}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_system_probes_are_shell_lines() {
        let command = remote_shell_command(TOOL_SYSTEM_PROCESSES, &json!({ "limit": 7 })).unwrap();
        assert!(command.contains("head -n 7"));
        assert!(command.contains("--sort=-pcpu"));
        let sort = remote_shell_command(TOOL_SYSTEM_PROCESSES, &json!({ "sort": "mem" })).unwrap();
        assert!(sort.contains("--sort=-pmem"));
        assert!(remote_shell_command(TOOL_SYSTEM_OVERVIEW, &json!({})).unwrap().contains("uname -a"));
        assert!(remote_shell_command(TOOL_SYSTEM_DISK, &json!({ "includeInodes": true }))
            .unwrap()
            .contains("df -i"));
        assert!(remote_shell_command(TOOL_SYSTEM_MEMORY, &json!({}))
            .unwrap()
            .contains("/proc/meminfo"));
        assert!(remote_shell_command(TOOL_RUN_COMMAND, &json!({})).is_none());
    }

    #[test]
    fn session_tools_are_marked_as_such() {
        assert!(needs_session(TOOL_RUN_COMMAND));
        assert!(needs_session(TOOL_SYSTEM_OVERVIEW));
        assert!(needs_session(TOOL_TERMINAL_READ));
        assert!(!needs_session(TOOL_WEB_FETCH));
        assert!(!needs_session(TOOL_LOCAL_READ));
        assert!(is_read_only(TOOL_REMOTE_LIST));
        assert!(!is_read_only(TOOL_REMOTE_WRITE));
        assert!(!is_read_only(TOOL_RUN_COMMAND));
    }
}
