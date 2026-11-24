//! The conversation and the tool loop.
//!
//! Port of the parts of `main.ts` that drive a turn: build the request, stream
//! the answer, then run whatever tools the model asked for — asking first when
//! the rules say so — and feed the results back until it answers in prose.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use base64::Engine as _;
use serde_json::Value;

use super::permissions::{Permission, Permissions};
use super::map::{EnvMap, SystemFacts};
use crate::plugin::{RemoteHost, RemoteHosts, RemoteState};
use super::protocol::{
    ApiMessage, ApiToolCall, Completion, DEFAULT_MAX_SUBAGENTS, ImagePart,
    MAX_SUBAGENT_RESULT_CHARS, TOOL_ASK_USER, TOOL_CHANGE_DIR, TOOL_CURRENT_TIME, TOOL_DEV_CREATE, TOOL_DEV_DIFF, TOOL_DEV_EDIT, TOOL_DEV_FIND,
    TOOL_DEV_GREP, TOOL_DEV_IMAGE, TOOL_DEV_LIST, TOOL_DEV_VIEW, TOOL_ENV_FIND, TOOL_ENV_OUTLINE,
    TOOL_ENV_OVERVIEW, TOOL_ENV_SCAN, TOOL_ENV_SEARCH, TOOL_ENV_SYSTEM, TOOL_GIT_BRANCHES,
    TOOL_GIT_CHECKOUT, TOOL_GIT_COMMIT, TOOL_GIT_DIFF, TOOL_GIT_LOG, TOOL_GIT_STATUS,
    TOOL_LOAD_SCOPE,
    TOOL_LOCAL_DELETE, TOOL_LOCAL_EDIT, TOOL_LOCAL_GREP, TOOL_LOCAL_LIST, TOOL_LOCAL_MOVE,
    TOOL_LOCAL_MULTI_EDIT, TOOL_LOCAL_PATCH, TOOL_LOCAL_READ, TOOL_LOCAL_REPLACE, TOOL_LOCAL_RUN,
    TOOL_LOCAL_WRITE, TOOL_RAG_SEARCH, TOOL_REMOTE_DELETE, TOOL_REMOTE_EDIT, TOOL_REMOTE_LIST,
    TOOL_REMOTE_MOVE, TOOL_REMOTE_PATCH, TOOL_REMOTE_READ, TOOL_REMOTE_STAT, TOOL_REMOTE_WRITE,
    TOOL_REMOTE_ZIP_EXTRACT, TOOL_REMOTE_ZIP_LIST, TOOL_REMOTE_ZIP_READ, TOOL_RUN_COMMAND,
    TOOL_HOSTS_LIST, TOOL_RUN_COMMAND_BACKGROUND, TOOL_RUN_SCRIPT, TOOL_RUN_SUDO,
    TOOL_SCRIPT_RESULT, TOOL_SPAWN_SUBAGENT,
    TOOL_SYSTEM_DISK, TOOL_SYSTEM_MEMORY, TOOL_SYSTEM_OVERVIEW, TOOL_SYSTEM_PROCESSES,
    TOOL_TERMINAL_KEYS, TOOL_TERMINAL_READ, TOOL_TERMINAL_WAIT, TOOL_TERMINAL_WRITE,
    TOOL_UNLOAD_SCOPE, TOOL_WAIT_SUBAGENT, TOOL_WEB_FETCH, TOOL_WEB_SEARCH,
};
use super::providers::{CompletionRequest, EmbedConfig, StreamEvent, complete, embeddings};
use super::rag;
use super::rules::{
    ApprovalScope, RuleDecision, approval_scopes, decide_command,
};
use super::scopes::{self, Loaded, Scope};
use super::script::{self, ScriptEvent, ScriptReply};
use super::tools::{self, ToolResult};
use super::web;
use crate::plugin::SessionAccess;

/// The session slot every dispatch takes.
///
/// Two lifetimes on purpose: the borrow of the slot must be allowed to end long
/// before the session itself does, or nothing else could touch the context.
type Session<'a, 'b> = &'a mut Option<&'b mut dyn SessionAccess>;

/// Longest command output handed to the model.
pub const MAX_OUTPUT_CHARS: usize = 64_000;
/// Prefix of the call ids a script's tool calls carry; they never reach the provider.
const SCRIPT_CALL_PREFIX: &str = "script-";
/// Background scripts one conversation may run at once.
const MAX_BACKGROUND_SCRIPTS: usize = 4;
/// Longest `script_result` waits for a background script.
const MAX_SCRIPT_WAIT_SECONDS: u64 = 300;
/// Hits `env_find` returns when the call does not say.
const ENV_FIND_HITS_DEFAULT: usize = 30;
/// Files `env_search` ranks when the call does not say.
const ENV_SEARCH_HITS_DEFAULT: usize = 10;
/// How much of the raw tail is kept so the markers are never cut off.
pub const MARKER_TAIL_CHARS: usize = 4_000;
/// Key the session caches the running command under.
pub const EXEC_KEY: &str = "ai.run_command";
/// Key the session caches a probe under (existence checks and system reports).
pub const PROBE_KEY: &str = "ai.probe";
/// Key the session caches a map scan the pane asked for under.
pub const MAP_KEY: &str = "ai.env_scan";
/// Prefix of the ids sub-agents are handed out.
pub const SUBAGENT_ID_PREFIX: &str = "sub";

/// Prefix of the names this pane gives the connections it opens to other hosts.
const HOST_HANDLE_PREFIX: &str = "agent:";
/// Longest sub-agent name taken from its prompt.
const SUBAGENT_TITLE_CHARS: usize = 48;
/// Said to a model that asks a sub-agent to spawn one of its own.
pub const NO_NESTED_SUBAGENTS: &str = "Sub-agents cannot start sub-agents; do the work yourself.";
/// Marker the sudo preamble prints when the password was refused.
pub const SUDO_AUTH_FAILED_MARKER: &str = "__WASSH_SUDO_FAILED__";
/// What the user is told when `sudo` refused the password they typed.
pub const SUDO_REFUSED: &str = "sudo refused the password.";
/// Said to the model when a command needed root it did not have.
pub const SUDO_HINT: &str =
    "[This command needs root: call run_sudo with it so the user can type their password.]";
/// Said to the model when a file search ran on a tree the map has never walked.
pub const NO_MAP_HINT: &str = "[The Graph holds no map of this tree yet: env_scan walks it \
once, and env_search and env_outline then answer questions about it without touching the \
host.]";
/// Lines a shell prints when `sudo` cannot ask for its password.
const SUDO_DEMAND_MARKERS: [&str; 4] = [
    "sudo: a password is required",
    "sudo: no tty present",
    "sudo: a terminal is required",
    "must be root",
];
/// Longest a `terminal_wait` blocks by default.
pub const TERMINAL_WAIT_DEFAULT_MS: u64 = 5_000;
/// Said when a reply arrives with neither text nor tool calls.
pub const EMPTY_ANSWER: &str = "The model returned an empty answer.";
/// Said when a capability is switched off or denied in the settings.
pub const NOT_PERMITTED: &str = "This action is not permitted.";
/// Said when the user turns an action down.
pub const DENIED_BY_USER: &str = "Action denied by user.";
/// Said when a Plan-mode chat is asked to change something.
pub const PLAN_READ_ONLY: &str = "This chat is in Plan mode, which only reads. Switch it to Agent \
mode to change anything.";
/// Said when a script-mode chat calls a tool directly instead of from a script.
pub const SCRIPT_MODE_ONLY: &str = "This chat is in script mode, so the tools are called from \
inside `run_script`: call run_script with a program that awaits `tools.<name>({ ... })`.";
/// Question shown when `ask_user` carried none.
const QUESTION_FALLBACK: &str = "The agent needs your input.";
/// Answer given to a tool call the user stopped before it ran.
pub const STOPPED_BY_USER: &str = "Stopped by the user before this ran.";
/// System prompt used when summarising a chat for a new one.
pub const SUMMARY_SYSTEM_PROMPT: &str = "Summarise the conversation above for a fresh agent session that must \
continue the work. Reply with terse bullet points, at most 150 words, covering: Goal (what the user wants), \
State (what has been done and what was learned, including hosts, paths and commands that matter), Next (the \
immediate next step). No preamble, no closing sentence.";

/// What went out with the user's message, shown as tags on its bubble.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UserMeta {
    /// Files attached to the message.
    pub attachments: usize,
    /// Whether the terminal screen came along.
    pub terminal: bool,
}

/// One line of the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Turn {
    /// What the user asked, and what came with it.
    User {
        /// The text they typed.
        text: String,
        /// Files and terminal output folded into what the model saw.
        meta: UserMeta,
    },
    /// What the model answered.
    Assistant {
        /// The text so far.
        text: String,
        /// Whether the user stopped it early.
        stopped: bool,
    },
    /// A tool the agent ran.
    Tool {
        /// Tool name.
        name: String,
        /// One-line summary of the call, e.g. `$ uptime`.
        label: String,
        /// What it returned.
        output: String,
        /// Whether it failed.
        failed: bool,
    },
    /// A call the rules refused.
    Denied(String),
    /// A question the agent asked, and the answer once given.
    Question {
        /// What it asked.
        text: String,
        /// The user's answer, when it came.
        answer: Option<String>,
    },
    /// A script the agent ran, with the calls it made.
    Script {
        /// The program it wrote.
        code: String,
        /// The calls it made, in order.
        calls: Vec<ScriptRow>,
        /// The result handed back to the model, once it came.
        result: Option<String>,
        /// Whether the script failed.
        failed: bool,
        /// What the script logged, shown to the reader only.
        console: String,
    },
    /// Something that went wrong.
    Error(String),
    /// A note from a slash command.
    Notice(String),
}

/// One tool call a script made, for its transcript card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptRow {
    /// Tool name.
    pub name: String,
    /// One-line summary of the call.
    pub label: String,
    /// What the tool returned.
    pub output: String,
    /// Whether it failed.
    pub failed: bool,
}

/// A script the agent is running.
struct ScriptRun {
    /// The call the model made.
    outer: ApiToolCall,
    /// Its transcript line.
    turn: usize,
    /// The call the script is waiting on, with the id it waits under.
    inner: Option<(ApiToolCall, u64)>,
    /// Every call the script has made so far.
    calls: Vec<ScriptRow>,
    /// When the call it is waiting on started.
    since: Option<std::time::Instant>,
    /// Where its events and answers travel.
    job: script::ScriptJob,
}

/// One script running beside the conversation.
pub struct BackgroundScript {
    /// Id the model uses: `s1`, `s2`…
    pub id: String,
    /// The script's own conversation state: its calls, rules and prompts.
    pub agent: Agent,
    /// What it produced, once it finished.
    result: Option<(String, bool)>,
    /// Whether the finish line was already written to the transcript.
    announced: bool,
}

/// A question the agent asks, with optional selectable answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentQuestion {
    /// What the user is asked.
    pub text: String,
    /// Selectable answers; the pane adds a final free-text choice.
    pub options: Vec<String>,
}

/// What the agent is doing right now.
pub enum Phase {
    /// Waiting for the user.
    Idle,
    /// A reply is streaming in.
    Streaming {
        /// Where the text goes.
        turn: usize,
        /// Events from the worker thread.
        events: Receiver<StreamEvent>,
    },
    /// A command needs the user's permission.
    Approving {
        /// The call being approved.
        call: ApiToolCall,
        /// The command it wants to run.
        command: String,
        /// "Always allow/deny" choices offered.
        scopes: Vec<ApprovalScope>,
    },
    /// A capability whose permission is "ask" waits for the user.
    Allowing {
        /// The call being approved.
        call: ApiToolCall,
        /// Its transcript line.
        turn: usize,
        /// What it wants to do, shown on the card.
        label: String,
    },
    /// A script asked to run longer than the default, and waits for the user.
    Extending {
        /// The `run_script` call waiting on the answer.
        call: ApiToolCall,
        /// Where its transcript line is.
        turn: usize,
        /// Seconds it wants to run for.
        seconds: u64,
        /// Whether it was asked to run beside the conversation.
        background: bool,
    },
    /// Another host is about to be opened, and the user is being asked about it.
    Connect {
        /// Saved host the call named.
        host: String,
        /// The call waiting on the answer.
        call: ApiToolCall,
        /// Where its transcript line is.
        turn: usize,
    },
    /// A connection to another host is opening; the call runs once it has.
    Remote {
        /// Saved host the call named.
        host: String,
        /// The call waiting on the connection.
        call: ApiToolCall,
        /// Where its transcript line is.
        turn: usize,
    },
    /// A tool is running.
    Running {
        /// The call being answered.
        call: ApiToolCall,
        /// Where its transcript line is.
        turn: usize,
        /// What the agent is waiting for.
        waiting: Waiting,
    },
    /// The agent asked the user a question.
    Asking {
        /// The call being answered.
        call: ApiToolCall,
        /// Questions the pane shows one at a time.
        questions: Vec<AgentQuestion>,
    },
    /// A command needs root, so the user is asked for their password.
    Sudo {
        /// The call being answered.
        call: ApiToolCall,
        /// The command to run as root.
        command: String,
        /// Why it needs root, when the model said.
        reason: Option<String>,
    },
}

/// What a pending tool waits on.
pub enum Waiting {
    /// A shell command on the session.
    Command {
        /// Cache key it answers under.
        key: String,
        /// What to do with the output.
        then: CommandThen,
        /// Saved host it ran on, when it is not this tab's own.
        host: Option<String>,
    },
    /// A sub-agent, until it answers.
    Subagent {
        /// Id `wait_subagent` was given.
        id: String,
    },
    /// A remote read.
    Read {
        /// File being read.
        path: String,
        /// Cap on the bytes.
        max: u64,
        /// What to do with the contents.
        then: ReadThen,
    },
    /// A remote listing.
    List {
        /// Directory being listed.
        path: String,
    },
    /// An upload in flight.
    Transfer {
        /// Where the bytes go.
        target: String,
        /// Text to report instead of the transfer line (the diff of an edit).
        note: Option<String>,
    },
    /// A worker thread (web, knowledge base).
    Worker {
        /// Its answer.
        events: Receiver<ToolResult>,
    },
    /// The terminal screen, until it changes.
    Terminal {
        /// The screen before the wait.
        before: String,
        /// When the wait gives up, in milliseconds since the epoch.
        until_ms: u64,
    },
    /// A script in its sandbox, until it returns or asks for another tool.
    Script,
    /// A background script, until it answers or the wait ends.
    BackgroundScript {
        /// Id the model asked for.
        id: String,
        /// When the wait gives up, in milliseconds since the epoch.
        until_ms: u64,
    },
}

/// What the pane shows while another host is being reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostCard {
    /// The user is being asked whether this chat may reach the host.
    Ask,
    /// The host needs a password the vault does not hold.
    Password,
    /// The connection is opening.
    Connecting,
}

/// The name this pane gives the connection it opens to a saved host.
fn host_handle(host: &str) -> String {
    format!("{HOST_HANDLE_PREFIX}{host}")
}

/// What follows a probe command.
#[derive(Clone, Debug)]
pub enum CommandThen {
    /// Return the output.
    Return,
    /// Read the output as a scan of `root` and store it as the environment map.
    ScanMap {
        /// Directory the scan walked.
        root: String,
    },
    /// Read the output as the host's ports and services.
    ScanSystem,
    /// Write a remote file once the probe says whether it exists.
    WriteRemote {
        /// Destination.
        path: String,
        /// Contents.
        content: String,
        /// Whether an existing file may be replaced.
        overwrite: bool,
    },
    /// Read the patch the probe printed, then apply it to `path`.
    ReadPatchFile {
        /// File the patch changes.
        path: String,
        /// Where the patch was read from, for the error message.
        patch_path: String,
    },
}

/// What follows a remote read.
#[derive(Clone, Debug)]
pub enum ReadThen {
    /// Return the contents.
    Return,
    /// Number the lines from `start` before returning them.
    View {
        /// First line shown, 1-based.
        start: usize,
        /// Most lines shown.
        lines: usize,
    },
    /// Replace text and write the file back.
    Edit {
        /// Text to find.
        old: String,
        /// Its replacement.
        new: String,
    },
    /// Diff the contents against the text the model expected.
    Diff {
        /// What the file should have said.
        new: String,
        /// Lines of context around each change.
        context: usize,
    },
    /// Apply a unified diff and write the file back.
    Patch {
        /// The diff, `@@` hunks included.
        patch: String,
    },
    /// Return the contents base64-encoded.
    Base64,
    /// Show the image to the model as a user message.
    Image,
}

/// Settings the tools read.
#[derive(Clone, Debug, Default)]
pub struct ToolSettings {
    /// Search engine for `web_search`.
    pub web_provider: Option<String>,
    /// Its key.
    pub web_key: Option<String>,
    /// Knowledge-base folders.
    pub rag_folders: Vec<String>,
    /// Embedding model, when semantic search is configured.
    pub embed: Option<EmbedConfig>,
}

/// Command cache keys, so agents running side by side never read each other's answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandKeys {
    /// Key `run_command` answers under.
    pub exec: String,
    /// Key probes answer under.
    pub probe: String,
}

impl Default for CommandKeys {
    fn default() -> Self {
        Self {
            exec: EXEC_KEY.to_owned(),
            probe: PROBE_KEY.to_owned(),
        }
    }
}

impl CommandKeys {
    /// Keys of a sub-agent, kept apart from its parent's.
    pub fn subagent(id: &str) -> Self {
        Self {
            exec: format!("{EXEC_KEY}#{id}"),
            probe: format!("{PROBE_KEY}#{id}"),
        }
    }
}

/// What a sub-agent is doing, for the chat list and the parent's caption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentState {
    /// Working.
    Running,
    /// Waiting for an answer the parent chat shows.
    NeedsAnswer,
    /// Finished.
    Done,
    /// Ended on an error.
    Failed,
    /// Stopped by the user.
    Stopped,
}

impl SubagentState {
    /// Short word shown beside the sub-agent's name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::NeedsAnswer => "needs answer",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    /// The stored spelling, for the plugin data.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::NeedsAnswer => "needs-answer",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

/// How a sub-agent's run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentEnd {
    /// It answered.
    Done,
    /// It ended on an error.
    Failed,
    /// The user stopped it.
    Stopped,
}

impl SubagentEnd {
    /// How the plugin data stores it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    /// Read a stored one back.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }
}

/// One conversation the parent delegated work to.
pub struct Subagent {
    /// Id `wait_subagent` takes.
    pub id: String,
    /// Short name shown with the chats.
    pub title: String,
    /// The task it was given.
    pub prompt: String,
    /// Its own conversation.
    pub agent: Agent,
    /// How it ended, once it has.
    pub end: Option<SubagentEnd>,
    /// Whether the parent already said it is waiting for an answer.
    waiting_announced: bool,
}

impl Subagent {
    /// What it is doing now.
    pub fn state(&self) -> SubagentState {
        match self.end {
            Some(SubagentEnd::Done) => SubagentState::Done,
            Some(SubagentEnd::Failed) => SubagentState::Failed,
            Some(SubagentEnd::Stopped) => SubagentState::Stopped,
            None if self.agent.needs_user() => SubagentState::NeedsAnswer,
            None => SubagentState::Running,
        }
    }

    /// Whether it is still working.
    pub fn running(&self) -> bool {
        self.end.is_none()
    }

    /// The last prose it wrote, capped for the transcript and the model.
    pub fn answer(&self) -> String {
        head_chars(self.last_text(), MAX_SUBAGENT_RESULT_CHARS)
    }

    /// The last prose it wrote in full, for a caller that reads it as data rather than as
    /// a report — a mapping pass answers with a JSON object.
    pub fn answer_full(&self) -> String {
        head_chars(self.last_text(), super::map::MAX_MAP_ANSWER_CHARS)
    }

    /// The last prose it wrote.
    fn last_text(&self) -> &str {
        self.agent
            .turns
            .iter()
            .rev()
            .find_map(|turn| match turn {
                Turn::Assistant { text, .. } if !text.trim().is_empty() => Some(text.trim()),
                _ => None,
            })
            .unwrap_or_default()
    }
}

/// Its line in the parent transcript, once `sub` starts waiting for the user.
///
/// Says it once per wait, so a sub-agent that asks twice is not announced twice.
pub fn announce_wait(sub: &mut Subagent) -> Option<Turn> {
    if !sub.agent.needs_user() {
        sub.waiting_announced = false;
        return None;
    }
    if std::mem::replace(&mut sub.waiting_announced, true) {
        return None;
    }
    let title = sub.title.clone();
    Some(Turn::Notice(format!(
        "Sub-agent \"{title}\" needs your answer."
    )))
}

/// Record how `sub` ended: what its parent shows, and what its model reads next time.
pub fn end_subagent(sub: &mut Subagent) -> (Turn, String) {
    let answer = sub.answer();
    let failed = sub.agent.error.is_some() && answer.is_empty();
    let error = sub.agent.error.clone();
    sub.end = Some(if failed {
        SubagentEnd::Failed
    } else {
        SubagentEnd::Done
    });
    let title = sub.title.clone();
    let (line, report) = if failed {
        let error = error.unwrap_or_else(|| "it stopped without an answer.".to_owned());
        (
            format!("Sub-agent \"{title}\" failed: {error}"),
            format!("[Sub-agent \"{title}\" failed]\n{error}"),
        )
    } else {
        (
            format!("Sub-agent \"{title}\" finished:\n{answer}"),
            format!("[Sub-agent \"{title}\" finished]\n{answer}"),
        )
    };
    (Turn::Notice(line), report)
}

/// A name for a sub-agent, from what it was asked to do.
fn subagent_title(prompt: &str) -> String {
    let first = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    head_chars(first, SUBAGENT_TITLE_CHARS)
}

/// Cut `text` to `max_chars`, marking the cut.
fn head_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max_chars).collect();
    cut.push('…');
    cut
}

/// A spawn the pane still has to turn into a provider request.
///
/// The agent knows what to delegate but not which provider, model or prompt the request
/// needs, so the pane builds it and hands the sub-agent back its first request.
#[derive(Clone, Debug)]
pub struct SpawnRequest {
    /// Call being answered.
    pub call: ApiToolCall,
    /// Transcript line of the call.
    pub turn: usize,
    /// Name the sub-agent gets.
    pub title: String,
    /// Task it is given.
    pub prompt: String,
}

/// One conversation's state.
#[derive(Default)]
pub struct Agent {
    /// The transcript.
    pub turns: Vec<Turn>,
    /// The conversation in the provider's shape.
    pub history: Vec<ApiMessage>,
    /// What the agent is doing.
    pub phase: Option<Phase>,
    /// Last failure, shown under the transcript.
    pub error: Option<String>,
    /// Directory commands run in.
    pub cwd: String,
    /// Rules shipped with the app, from the plugin settings.
    pub app_allow: Vec<String>,
    /// Patterns the app refuses, from the plugin settings.
    pub app_deny: Vec<String>,
    /// Rules for this host.
    pub host_allow: Vec<String>,
    /// Patterns this host refuses.
    pub host_deny: Vec<String>,
    /// Rules the user approved for this session.
    pub session_allow: Vec<String>,
    /// Rules the user refused for this session.
    pub session_deny: Vec<String>,
    /// Set once the user allowed every prompt for the rest of this session.
    pub allow_all: bool,
    /// Tool settings from the plugin data.
    pub tools: ToolSettings,
    /// What this host may do, read from the plugin settings.
    pub permissions: Permissions,
    /// Whether this chat only reads: Plan mode offers the read-only tools and refuses the
    /// rest even if the model asks for one.
    pub plan: bool,
    /// The scopes whose tools this conversation carries.
    pub scopes: Loaded,
    /// Characters the last request spent on the system prompt and the tool schemas.
    pub prompt_chars: usize,
    /// Set when the model owes an answer to a tool result.
    pub needs_reply: bool,
    /// Messages the user typed while a run was in flight, in order.
    ///
    /// They show up in the transcript straight away but only join the provider history
    /// once the run in front of them is done, so the model sees them after it finished
    /// the step it was on rather than before its own reply.
    injected: std::collections::VecDeque<(String, UserMeta, Vec<ImagePart>)>,
    /// Clock used by the time-based tools; the view sets it every frame.
    pub now_ms: u64,
    /// The last command, kept so a refused `sudo` can be retried.
    pub last_command: Option<String>,
    /// Tool calls still to run for the current reply.
    queue: VecDeque<ApiToolCall>,
    /// The conversation's size in characters, with the length and last-message size it was
    /// counted at; see [`Agent::history_chars`].
    history_count: Option<(usize, usize, usize)>,
    /// Summary of the part of the conversation that was compacted away.
    ///
    /// It travels in the system prompt rather than the history, so the transcript and the
    /// history keep one user turn per user message.
    pub compacted: Option<String>,
    /// The environment map of the host, which outlives the conversation.
    pub map: EnvMap,
    /// How the host is set up: its ports and services, which also outlive the conversation.
    pub system: SystemFacts,
    /// A map scan the pane asked for: the root, until its output arrives.
    pub map_scan: Option<String>,
    /// Set when a tool answered about a tree whose files are listed but not described, so
    /// the pane can start the mapping pass.
    deep_map_wanted: bool,
    /// Sub-agents this conversation started, in the order they were started.
    pub subagents: Vec<Subagent>,
    /// Counter behind the sub-agent ids.
    next_subagent: usize,
    /// How many sub-agents may run at once.
    pub max_subagents: usize,
    /// Whether this conversation is itself a sub-agent.
    pub subagent: bool,
    /// Command cache keys, kept apart from every other agent's.
    pub keys: CommandKeys,
    /// A spawn the pane still has to launch.
    spawn_request: Option<SpawnRequest>,
    /// Answers from sub-agents, held until the conversation is between turns so a tool
    /// call of its own is never separated from its result.
    pending_reports: Vec<String>,
    /// Pictures read by tools, held until every call of the turn is answered so the image
    /// message lands after the last tool result rather than in the middle of them.
    shown_images: Vec<(String, ImagePart)>,
    /// The script this conversation is running, if any.
    script: Option<ScriptRun>,
    /// Scripts running beside the conversation, in the order they were started.
    pub background: Vec<BackgroundScript>,
    /// Counter behind the background script ids.
    next_background: usize,
    /// Whether the tools are offered as a script API instead of one schema per tool.
    pub script_mode: bool,
    /// The app's connections to other hosts, handed in by the pane every frame.
    remote: Option<Rc<RefCell<dyn RemoteHosts>>>,
    /// Saved hosts as the app last listed them, for the model and the cards.
    pub hosts: Vec<RemoteHost>,
    /// Saved hosts this conversation may reach without asking.
    pub reach_allow: Vec<String>,
    /// Saved hosts this conversation may never reach.
    pub reach_deny: Vec<String>,
    /// Working directory of each other host, by host id.
    host_cwds: BTreeMap<String, String>,
    /// The host the call being dispatched runs on, when it is not this tab's own.
    active_host: Option<String>,
    /// Whether the app's connections are lent out for the call in flight.
    on_host: bool,
    /// Whether a queued call was left for the moment those connections come back.
    resume_after_host: bool,
}

impl Agent {
    /// A fresh conversation.
    pub fn new() -> Self {
        Self {
            cwd: "/".to_owned(),
            max_subagents: DEFAULT_MAX_SUBAGENTS,
            ..Self::default()
        }
    }

    /// Whether a reply or tool is in flight, or another host is being reached.
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Some(Phase::Streaming { .. } | Phase::Running { .. } | Phase::Remote { .. })
        )
    }

    /// Whether the pane is asking whether another host may be reached.
    pub fn connecting(&self) -> bool {
        matches!(self.phase, Some(Phase::Connect { .. }))
    }

    /// Whether the user has to answer an approval.
    pub fn approving(&self) -> bool {
        matches!(self.phase, Some(Phase::Approving { .. }))
    }

    /// Whether a capability's permission asks the user.
    pub fn allowing(&self) -> bool {
        matches!(self.phase, Some(Phase::Allowing { .. }))
    }

    /// Whether a script's request for more run time waits for the user.
    pub fn extending(&self) -> bool {
        matches!(self.phase, Some(Phase::Extending { .. }))
    }

    /// Whether the agent is waiting on the user.
    pub fn asking(&self) -> bool {
        matches!(self.phase, Some(Phase::Asking { .. }))
    }

    /// Whether the pane is asking for a sudo password.
    pub fn sudo_prompt(&self) -> Option<(&str, Option<&str>)> {
        match &self.phase {
            Some(Phase::Sudo {
                command, reason, ..
            }) => Some((command.as_str(), reason.as_deref())),
            _ => None,
        }
    }

    /// Whether the run is parked on the user rather than on a tool.
    pub fn needs_user(&self) -> bool {
        self.approving()
            || self.allowing()
            || self.extending()
            || self.asking()
            || self.sudo_prompt().is_some()
            || self.connecting()
    }

    /// Whether anything at all blocks a new message.
    pub fn blocked(&self) -> bool {
        self.busy()
            || self.approving()
            || self.allowing()
            || self.extending()
            || self.asking()
            || self.sudo_prompt().is_some()
            || self.connecting()
            || self.spawning()
    }

    /// Throw the conversation away.
    pub fn clear(&mut self) {
        self.turns.clear();
        self.history.clear();
        self.injected.clear();
        self.phase = None;
        self.error = None;
        self.queue.clear();
        self.needs_reply = false;
        self.compacted = None;
        self.scopes = Loaded::new();
        self.plan = false;
        self.script = None;
        self.spawn_request = None;
        self.pending_reports.clear();
        self.shown_images.clear();
        for sub in &mut self.subagents {
            if sub.running() {
                sub.agent.stop();
            }
        }
        self.subagents.clear();
        for bg in &mut self.background {
            bg.agent.stop();
        }
        self.background.clear();
        self.next_background = 0;
        self.next_subagent = 0;
    }

    /// Add the user's turn, if the conversation is idle.
    pub fn push_user(&mut self, text: &str, meta: UserMeta) -> bool {
        self.push_user_with_images(text, meta, Vec::new())
    }

    /// Add the user's turn with the images that went with it.
    pub fn push_user_with_images(
        &mut self,
        text: &str,
        meta: UserMeta,
        images: Vec<ImagePart>,
    ) -> bool {
        let text = text.trim();
        if text.is_empty() || self.blocked() {
            return false;
        }
        self.error = None;
        self.turns.push(Turn::User {
            text: text.to_owned(),
            meta,
        });
        self.history.push(ApiMessage::User {
            content: text.to_owned(),
            images,
        });
        true
    }

    /// Take a message the user typed while a run was in flight.
    ///
    /// It appears in the transcript at once — it is part of the conversation the agent is
    /// in the middle of — and is folded into the provider history by
    /// [`Agent::flush_injected`] once that run is done.
    pub fn inject_user(&mut self, text: &str, meta: UserMeta) -> bool {
        self.inject_user_with_images(text, meta, Vec::new())
    }

    /// Take a message typed mid-run, with the images that went with it.
    pub fn inject_user_with_images(
        &mut self,
        text: &str,
        meta: UserMeta,
        images: Vec<ImagePart>,
    ) -> bool {
        let text = text.trim();
        if text.is_empty() || !self.blocked() {
            return false;
        }
        self.error = None;
        self.turns.push(Turn::User {
            text: text.to_owned(),
            meta,
        });
        self.injected.push_back((text.to_owned(), meta, images));
        true
    }

    /// Fold messages injected mid-run into the provider history, once nothing is running.
    ///
    /// Returns true when the model owes a reply to what was added.
    pub fn flush_injected(&mut self) -> bool {
        if self.phase.is_some() || self.injected.is_empty() {
            return false;
        }
        for (text, _, images) in self.injected.drain(..) {
            self.history.push(ApiMessage::User {
                content: text,
                images,
            });
        }
        self.needs_reply = true;
        true
    }

    /// Drop everything from the user turn at `index` on, returning its text so the
    /// composer can take it back — the reference's rewind-on-click.
    pub fn rewind_to(&mut self, index: usize) -> Option<String> {
        let text = match self.turns.get(index) {
            Some(Turn::User { text, .. }) if !self.blocked() => text.clone(),
            _ => return None,
        };
        // Turns and provider messages line up one user turn per user message.
        let wanted = self.turns[..index]
            .iter()
            .filter(|turn| matches!(turn, Turn::User { .. }))
            .count();
        if let Some(cut) = self
            .history
            .iter()
            .enumerate()
            .filter(|(_, message)| matches!(message, ApiMessage::User { .. }))
            .nth(wanted)
            .map(|(position, _)| position)
        {
            self.history.truncate(cut);
        }
        self.turns.truncate(index);
        self.error = None;
        self.needs_reply = false;
        Some(text)
    }

    /// Start a reply for the conversation as it stands.
    pub fn start(&mut self, request: CompletionRequest) {
        self.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        let turn = self.turns.len() - 1;
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || complete(&request, &sender));
        self.phase = Some(Phase::Streaming { turn, events });
    }

    /// Take in whatever the worker thread or the session has produced.
    pub fn poll(&mut self, session: Session<'_, '_>) -> bool {
        self.flush_reports();
        let mut progressed = self.poll_own(session);
        progressed |= self.poll_subagents(session);
        progressed |= self.poll_background(session);
        progressed
    }

    /// Fold the sub-agent answers that arrived mid-run into the history, now that the
    /// conversation is between turns again.
    fn flush_reports(&mut self) {
        if self.phase.is_some() {
            return;
        }
        for report in self.pending_reports.drain(..) {
            self.history.push(ApiMessage::User {
            content: report,
            images: Vec::new(),
        });
        }
    }

    /// Poll every sub-agent, and record the ones that have ended.
    fn poll_subagents(&mut self, session: Session<'_, '_>) -> bool {
        self.sync_subagents();
        let mut progressed = false;
        for index in 0..self.subagents.len() {
            if !self.subagents[index].running() {
                continue;
            }
            if self.subagents[index].agent.poll(session) {
                progressed = true;
            }
            self.announce_subagent_wait(index);
            let settled = self.subagents[index].agent.phase.is_none()
                && !self.subagents[index].agent.needs_reply
                && !self.subagents[index].agent.spawning();
            if settled {
                self.finish_subagent(index);
                progressed = true;
            }
        }
        progressed
    }

    /// Keep the running sub-agents on this conversation's rules and permissions.
    ///
    /// Copying rather than sharing is what lets an "always allow" answered later reach
    /// every sub-agent: the pane answers on the parent, and the next frame carries it in.
    fn sync_subagents(&mut self) {
        let max_subagents = self.max_subagents;
        let remote = self.remote.clone();
        let hosts = self.hosts.clone();
        let reach_allow = self.reach_allow.clone();
        let reach_deny = self.reach_deny.clone();
        for sub in self.subagents.iter_mut().filter(|sub| sub.running()) {
            let agent = &mut sub.agent;
            agent.max_subagents = max_subagents;
            agent.now_ms = self.now_ms;
            if agent.allow_all != self.allow_all {
                agent.allow_all = self.allow_all;
            }
            if agent.permissions != self.permissions {
                agent.permissions = self.permissions.clone();
            }
            agent.remote = remote.clone();
            if agent.hosts != hosts {
                agent.hosts = hosts.clone();
            }
            if agent.reach_allow != reach_allow {
                agent.reach_allow = reach_allow.clone();
            }
            if agent.reach_deny != reach_deny {
                agent.reach_deny = reach_deny.clone();
            }
            if agent.app_allow != self.app_allow {
                agent.app_allow = self.app_allow.clone();
            }
            if agent.app_deny != self.app_deny {
                agent.app_deny = self.app_deny.clone();
            }
            if agent.host_allow != self.host_allow {
                agent.host_allow = self.host_allow.clone();
            }
            if agent.host_deny != self.host_deny {
                agent.host_deny = self.host_deny.clone();
            }
            if agent.session_allow != self.session_allow {
                agent.session_allow = self.session_allow.clone();
            }
            if agent.session_deny != self.session_deny {
                agent.session_deny = self.session_deny.clone();
            }
        }
    }

    /// Say once, in the parent transcript, that a sub-agent is waiting for an answer.
    fn announce_subagent_wait(&mut self, index: usize) {
        let Some(sub) = self.subagents.get_mut(index) else {
            return;
        };
        if let Some(turn) = announce_wait(sub) {
            self.turns.push(turn);
        }
    }

    /// Record how a sub-agent ended, in the transcript and in what the model will read.
    fn finish_subagent(&mut self, index: usize) {
        let Some(sub) = self.subagents.get_mut(index) else {
            return;
        };
        let (turn, report) = end_subagent(sub);
        self.turns.push(turn);
        self.pending_reports.push(report);
    }

    /// Poll the background scripts, and note the ones that finished.
    fn poll_background(&mut self, session: Session<'_, '_>) -> bool {
        if self.background.is_empty() {
            return false;
        }
        let permissions = self.permissions.clone();
        let remote = self.remote.clone();
        let hosts = self.hosts.clone();
        let reach_allow = self.reach_allow.clone();
        let reach_deny = self.reach_deny.clone();
        let app_allow = self.app_allow.clone();
        let app_deny = self.app_deny.clone();
        let host_allow = self.host_allow.clone();
        let host_deny = self.host_deny.clone();
        let session_allow = self.session_allow.clone();
        let session_deny = self.session_deny.clone();
        let allow_all = self.allow_all;
        let now = self.now_ms;
        let mut progressed = false;
        let mut finished = Vec::new();
        for bg in &mut self.background {
            let agent = &mut bg.agent;
            agent.now_ms = now;
            if agent.permissions != permissions {
                agent.permissions = permissions.clone();
            }
            agent.remote = remote.clone();
            if agent.hosts != hosts {
                agent.hosts = hosts.clone();
            }
            if agent.reach_allow != reach_allow {
                agent.reach_allow = reach_allow.clone();
            }
            if agent.reach_deny != reach_deny {
                agent.reach_deny = reach_deny.clone();
            }
            if agent.app_allow != app_allow {
                agent.app_allow = app_allow.clone();
            }
            if agent.app_deny != app_deny {
                agent.app_deny = app_deny.clone();
            }
            if agent.host_allow != host_allow {
                agent.host_allow = host_allow.clone();
            }
            if agent.host_deny != host_deny {
                agent.host_deny = host_deny.clone();
            }
            if agent.session_allow != session_allow {
                agent.session_allow = session_allow.clone();
            }
            if agent.session_deny != session_deny {
                agent.session_deny = session_deny.clone();
            }
            if agent.allow_all != allow_all {
                agent.allow_all = allow_all;
            }
            if bg.result.is_some() {
                continue;
            }
            progressed |= agent.poll(&mut *session);
            let call_id = format!("bg-{}", bg.id);
            if let Some(result) = tool_result(agent, &call_id) {
                bg.result = Some(result);
                progressed = true;
            }
            if bg.result.is_some() && !bg.announced {
                bg.announced = true;
                let failed = bg.result.as_ref().is_some_and(|(_, failed)| *failed);
                finished.push((bg.id.clone(), failed));
            }
        }
        for (id, failed) in finished {
            let line = if failed {
                format!("Background script {id} failed.")
            } else {
                format!("Background script {id} finished. Call script_result with \"{id}\" to collect it.")
            };
            self.turns.push(Turn::Notice(line));
        }
        progressed
    }

    /// Take in whatever this conversation's own worker thread or tool produced.
    fn poll_own(&mut self, session: Session<'_, '_>) -> bool {
        let mut progressed = false;
        match &self.phase {
            Some(Phase::Streaming { turn, .. }) => {
                let turn = *turn;
                let mut done: Option<Result<Completion, String>> = None;
                if let Some(Phase::Streaming { events, .. }) = &self.phase {
                    loop {
                        match events.try_recv() {
                            Ok(StreamEvent::Delta(text)) => {
                                if let Some(Turn::Assistant { text: body, .. }) =
                                    self.turns.get_mut(turn)
                                {
                                    body.push_str(&text);
                                }
                                progressed = true;
                            }
                            Ok(StreamEvent::Done(completion)) => {
                                done = Some(Ok(completion));
                                break;
                            }
                            Ok(StreamEvent::Failed(error)) => {
                                done = Some(Err(error));
                                break;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                done = Some(Err("the provider closed the connection".to_owned()));
                                break;
                            }
                        }
                    }
                }
                if let Some(result) = done {
                    self.phase = None;
                    self.finish_reply(turn, result, session);
                    progressed = true;
                }
            }
            Some(Phase::Running {
                waiting: Waiting::Script,
                ..
            }) => progressed = self.poll_script(session),
            Some(Phase::Running { call, .. }) => {
                let call = call.clone();
                let host = self.running_host(&call);
                self.with_host(host, session, |agent, session| {
                    progressed = if agent.script_call_timed_out(session) {
                        true
                    } else {
                        agent.poll_tool(session)
                    };
                });
            }
            Some(Phase::Remote { host, call, turn }) => {
                let (host, call, turn) = (host.clone(), call.clone(), *turn);
                match self.host_state(&host) {
                    Some(RemoteState::Open { .. }) => {
                        self.phase = None;
                        self.reach_host(host, call, turn, session);
                        progressed = true;
                    }
                    Some(RemoteState::Failed(reason)) => {
                        self.phase = None;
                        let label = call_label(&call);
                        self.settle(call, turn, label, reason, true, session);
                        progressed = true;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        progressed
    }

    /// Drive the running script: dispatch the calls it makes, or take its result.
    fn poll_script(&mut self, session: Session<'_, '_>) -> bool {
        let Some(run) = &self.script else {
            return false;
        };
        let mut events = Vec::new();
        while let Ok(event) = run.job.events.try_recv() {
            events.push(event);
        }
        let mut progressed = false;
        for event in events {
            progressed = true;
            match event {
                ScriptEvent::Call(call) => self.start_script_call(call, &mut *session),
                ScriptEvent::Finished { result, console } => {
                    self.finish_script(result, console, session);
                    break;
                }
            }
        }
        progressed
    }

    /// Run one call the script asked for, through the same gates a direct call takes.
    fn start_script_call(&mut self, call: script::ScriptCall, session: Session<'_, '_>) {
        let Some(run) = self.script.as_mut() else {
            return;
        };
        let inner = ApiToolCall {
            id: format!("{SCRIPT_CALL_PREFIX}{}", call.id),
            name: call.name,
            arguments: call.arguments,
        };
        run.calls.push(ScriptRow {
            name: inner.name.clone(),
            label: call_label(&inner),
            output: String::new(),
            failed: false,
        });
        run.inner = Some((inner.clone(), call.id));
        run.since = Some(std::time::Instant::now());
        let turn = run.turn;
        self.dispatch(inner, turn, session);
    }

    /// Fail the script's pending call when it has waited too long, so a hung command cannot
    /// hold the conversation forever.
    fn script_call_timed_out(&mut self, session: Session<'_, '_>) -> bool {
        let Some(((inner, _), since)) = self
            .script
            .as_ref()
            .and_then(|run| run.inner.clone().zip(run.since))
        else {
            return false;
        };
        if since.elapsed() < std::time::Duration::from_millis(script::MAX_CALL_MS) {
            return false;
        }
        // A command can be interrupted; the other waits are only dropped.
        if let Some(Phase::Running {
            waiting: Waiting::Command { key, .. },
            ..
        }) = &self.phase
            && let Some(access) = session.as_mut()
        {
            access.cancel(key);
        }
        if let Some(run) = self.script.as_mut() {
            run.since = None;
        }
        let seconds = script::MAX_CALL_MS / 1_000;
        self.answer_script(
            &inner,
            call_label(&inner),
            format!("The tool did not answer within {seconds} seconds."),
            true,
        );
        true
    }

    /// Take the script's result: it becomes the `run_script` tool result, and nothing the
    /// script did in between enters the provider history.
    fn finish_script(
        &mut self,
        result: Result<String, String>,
        console: String,
        session: Session<'_, '_>,
    ) {        let Some(run) = self.script.take() else {
            return;
        };
        self.phase = None;
        let (text, failed) = match result {
            Ok(payload) => (script::clamp_result(payload), false),
            Err(problem) => (format!("The script failed: {problem}"), true),
        };
        if let Some(Turn::Script {
            calls,
            result: slot,
            failed: flag,
            console: logged,
            ..
        }) = self.turns.get_mut(run.turn)
        {
            *calls = run.calls;
            *slot = Some(text.clone());
            *flag = failed;
            *logged = console;
        }
        self.history.push(ApiMessage::Tool {
            tool_call_id: run.outer.id.clone(),
            content: text,
        });
        self.advance(session);
    }

    /// Poll the running tool and move the turn on when it answers.
    fn poll_tool(&mut self, session: Session<'_, '_>) -> bool {
        enum Ready {
            Settle(String, String, bool),
            /// Settle the tool call, then show the image to the model.
            SettleImage {
                label: String,
                output: String,
                image: ImagePart,
            },
            WriteRemote(String, String),
            ReadRemote {
                path: String,
                max: u64,
                then: ReadThen,
            },
        }

        let Some(Phase::Running {
            call,
            turn,
            waiting,
        }) = &self.phase
        else {
            return false;
        };
        let (call, turn) = (call.clone(), *turn);
        let ready = match waiting {
            Waiting::Script => return false,
            Waiting::Command { key, then, host } => {
                let (key, then, host) = (key.clone(), then.clone(), host.clone());
                let (output, failure) = match session.as_ref() {
                    Some(access) => (
                        access.output(&key).cloned(),
                        access.failure(&key).map(str::to_owned),
                    ),
                    None => (None, Some("The session is not connected.".to_owned())),
                };
                if output.is_none() && failure.is_none() {
                    return false;
                }
                match then {
                    CommandThen::Return => {
                        let mut moved = None;
                        let (visible, exit_code, directory, truncated) = match (output, failure) {
                            (Some(output), _) => {
                                let raw = tail_of(&format!("{}{}", output.stdout, output.stderr));
                                let (visible, exit_code, directory) = unwrap_output(&raw);
                                (
                                    clamp_output(visible),
                                    exit_code,
                                    directory,
                                    raw.chars().count() > MAX_OUTPUT_CHARS,
                                )
                            }
                            (None, Some(failure)) => (failure, None, None, false),
                            (None, None) => (String::new(), None, None, false),
                        };
                        if let Some(directory) = &directory
                            && self.remember_dir(host.as_deref(), directory.clone())
                        {
                            moved = Some(directory.clone());
                        }
                        // The wrapper prints a marker when `sudo` refused the password; say so.
                        let refused_sudo = visible.contains(SUDO_AUTH_FAILED_MARKER);
                        let mut visible = if refused_sudo {
                            visible
                                .replace(SUDO_AUTH_FAILED_MARKER, SUDO_REFUSED)
                                .trim_end()
                                .to_owned()
                        } else {
                            visible
                        };
                        // A plain `run_command` cannot answer sudo's prompt, so point the
                        // model at the tool that asks the user for the password.
                        let plain_command = matches!(
                            self.turns.get(turn),
                            Some(Turn::Tool { name, .. }) if name == TOOL_RUN_COMMAND
                        );
                        if plain_command && needs_sudo_help(&visible) {
                            visible.push('\n');
                            visible.push_str(SUDO_HINT);
                        }
                        // A command that left the shell in another directory has moved the
                        // conversation's: say so where the model reads, not only in the next
                        // request's prompt.
                        if let Some(moved) = &moved {
                            if !visible.is_empty() {
                                visible.push('\n');
                            }
                            visible.push_str(&format!("Working directory is now {moved}."));
                        }
                        // A search the map could have answered is the moment to say it exists:
                        // the tree is walked once, and every later question costs no host call.
                        let searched_by_hand = matches!(
                            self.turns.get(turn),
                            Some(Turn::Tool { name, .. })
                                if name == TOOL_DEV_GREP || name == TOOL_DEV_FIND
                        );
                        if searched_by_hand && self.map.is_empty() {
                            visible.push('\n');
                            visible.push_str(NO_MAP_HINT);
                        }
                        Ready::Settle(
                            self.turn_label(turn, &call),
                            command_result_text(&visible, exit_code, truncated),
                            exit_code.is_some_and(|code| code != 0) || refused_sudo,
                        )
                    }
                    CommandThen::ScanSystem => {
                        let text = output
                            .as_ref()
                            .map(|output| format!("{}{}", output.stdout, output.stderr))
                            .unwrap_or_else(|| failure.unwrap_or_default());
                        self.system = SystemFacts::parse(self.now_ms, &text);
                        Ready::Settle(
                            "env_system".to_owned(),
                            self.system.summary(),
                            false,
                        )
                    }
                    CommandThen::ScanMap { root } => {
                        let text = output
                            .as_ref()
                            .map(|output| format!("{}{}", output.stdout, output.stderr))
                            .unwrap_or_else(|| failure.unwrap_or_default());
                        let scanned = EnvMap::parse(&root, self.now_ms, &text);
                        let found = scanned.len();
                        let truncated = scanned.truncated;
                        let mapped_root = scanned.root.clone();
                        self.map = scanned;
                        let deeper = self.note_shallow_map();
                        Ready::Settle(
                            format!("env_scan {mapped_root}"),
                            match found {
                                0 => format!(
                                    "Mapped {mapped_root}: no files. The walk read nothing there — \
                                     check the directory, and that the session's shell can list it."
                                ),
                                found => format!(
                                    "Mapped {mapped_root}: {found} files{}. Query it with env_find, \
                                     env_outline or env_overview.{deeper}",
                                    if truncated {
                                        " (capped, the tree is bigger)"
                                    } else {
                                        ""
                                    }
                                ),
                            },
                            found == 0,
                        )
                    }
                    CommandThen::WriteRemote {
                        path,
                        content,
                        overwrite,
                    } => {
                        let exists = output
                            .as_ref()
                            .is_some_and(|output| output.stdout.trim_end().ends_with("yes"));
                        if exists && !overwrite {
                            Ready::Settle(
                                format!("write {path}"),
                                format!("{path} already exists; pass overwrite to replace it."),
                                true,
                            )
                        } else {
                            Ready::WriteRemote(path, content)
                        }
                    }
                    CommandThen::ReadPatchFile { path, patch_path } => match output {
                        Some(output) => Ready::ReadRemote {
                            path,
                            max: tools::MAX_FILE_READ_CHARS as u64,
                            then: ReadThen::Patch {
                                patch: output.stdout.clone(),
                            },
                        },
                        None => Ready::Settle(
                            format!("patch {path}"),
                            format!(
                                "Cannot read {patch_path}: {}",
                                failure.unwrap_or_default()
                            ),
                            true,
                        ),
                    },
                }
            }
            Waiting::Read { path, max, then } => {
                let (path, max, then) = (path.clone(), *max, then.clone());
                let content = session.as_mut().and_then(|access| access.read(&path, max));
                let Some(content) = content else {
                    return false;
                };
                let text = String::from_utf8_lossy(&content.data).into_owned();
                match then {
                    ReadThen::Return => {
                        let mut body = text;
                        if content.truncated {
                            body.push_str("\n\n[Content truncated at the read limit]");
                        }
                        Ready::Settle(format!("read {path}"), body, false)
                    }
                    ReadThen::View { start, lines } => {
                        let numbered: Vec<String> = text
                            .lines()
                            .enumerate()
                            .skip(start - 1)
                            .take(lines)
                            .map(|(index, line)| format!("{:>5}\t{line}", index + 1))
                            .collect();
                        let mut body = format!("{path}\n{}", numbered.join("\n"));
                        if content.truncated {
                            body.push_str("\n\n[Content truncated at the read limit]");
                        }
                        Ready::Settle(format!("read {path}"), body, false)
                    }
                    ReadThen::Edit { old, new } => {
                        if content.truncated {
                            Ready::Settle(
                                format!("edit {path}"),
                                format!("{path} is larger than the edit limit."),
                                true,
                            )
                        } else {
                            match replace_and_report(&text, &old, &new) {
                                Ok((updated, diff)) => {
                                    Ready::WriteRemote(path, format!("{updated}\u{0}{diff}"))
                                }
                                Err(error) => Ready::Settle(format!("edit {path}"), error, true),
                            }
                        }
                    }
                    ReadThen::Diff { new, context } => {
                        let change = tools::diff_texts(&text, &new, context);
                        Ready::Settle(format!("diff {path}"), change, false)
                    }
                    ReadThen::Patch { patch } => {
                        if content.truncated {
                            Ready::Settle(
                                format!("edit {path}"),
                                format!("{path} is larger than the patch limit."),
                                true,
                            )
                        } else {
                            match tools::apply_unified_patch(&text, &patch) {
                                Ok(updated) if updated == text => Ready::Settle(
                                    format!("edit {path}"),
                                    "The patch changed nothing.".to_owned(),
                                    false,
                                ),
                                Ok(updated) => {
                                    let diff = tools::diff_texts(
                                        &text,
                                        &updated,
                                        tools::DIFF_CONTEXT_LINES_DEFAULT,
                                    );
                                    Ready::WriteRemote(path, format!("{updated}\u{0}{diff}"))
                                }
                                Err(error) => Ready::Settle(format!("edit {path}"), error, true),
                            }
                        }
                    }
                    ReadThen::Base64 => {
                        let mut body = base64::engine::general_purpose::STANDARD.encode(&content.data);
                        if content.truncated {
                            body.push_str(
                                "\n\n[Stopped at the base64 read limit; the file continues]",
                            );
                        }
                        Ready::Settle(format!("read {path}"), body, false)
                    }
                    ReadThen::Image => {
                        if content.truncated {
                            Ready::Settle(
                                format!("image {path}"),
                                format!(
                                    "{path} is larger than {} bytes.",
                                    super::protocol::MAX_IMAGE_BYTES
                                ),
                                true,
                            )
                        } else if let Some(media_type) = super::protocol::image_media_type(&content.data)
                        {
                            Ready::SettleImage {
                                label: format!("image {path}"),
                                output: format!("The image {path} is shown to you now."),
                                image: ImagePart {
                                    media_type: media_type.to_owned(),
                                    data: content.data.clone(),
                                },
                            }
                        } else {
                            Ready::Settle(
                                format!("image {path}"),
                                format!(
                                    "{path} is not a PNG, JPEG, GIF or WebP image."
                                ),
                                true,
                            )
                        }
                    }
                }
            }
            Waiting::List { path } => {
                let path = path.clone();
                let listed = session
                    .as_ref()
                    .and_then(|access| access.listing())
                    .filter(|listing| listing.path == path)
                    .cloned();
                let Some(listing) = listed else {
                    return false;
                };
                let mut lines = Vec::new();
                for entry in listing.entries.iter().take(tools::MAX_DIR_ENTRIES) {
                    let kind = if entry.is_dir { "dir " } else { "file" };
                    let size = if entry.is_dir {
                        String::new()
                    } else {
                        format!("{:>10}", entry.size)
                    };
                    lines.push(format!("{kind}{size}  {}", entry.name));
                }
                if lines.is_empty() {
                    lines.push("(empty directory)".to_owned());
                }
                Ready::Settle(format!("list {}", listing.path), lines.join("\n"), false)
            }
            Waiting::Transfer { target, note } => {
                let (target, note) = (target.clone(), note.clone());
                let busy = session
                    .as_ref()
                    .is_some_and(|access| access.is_transferring());
                if busy {
                    return false;
                }
                let status = session
                    .as_ref()
                    .and_then(|access| access.transfer_status().map(str::to_owned))
                    .unwrap_or_default();
                let lower = status.to_lowercase();
                let failed =
                    lower.contains("fail") || lower.contains("denied") || lower.contains("error");
                match note {
                    Some(note) => {
                        let body = if failed {
                            format!("{note}\n\n[upload failed: {status}]")
                        } else {
                            note
                        };
                        Ready::Settle(format!("edit {target}"), body, failed)
                    }
                    None => Ready::Settle(
                        format!("write {target}"),
                        if status.is_empty() {
                            format!("Wrote {target}.")
                        } else if failed {
                            status
                        } else {
                            format!("Wrote {target}. {status}")
                        },
                        failed,
                    ),
                }
            }
            Waiting::Worker { events } => match events.try_recv() {
                Ok(Ok(output)) => Ready::Settle(self.turn_label(turn, &call), output, false),
                Ok(Err(error)) => Ready::Settle(self.turn_label(turn, &call), error, true),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Ready::Settle(
                    self.label(turn),
                    "The tool stopped without an answer.".to_owned(),
                    true,
                ),
                Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            },
            Waiting::Subagent { id } => {
                let id = id.clone();
                let found = self
                    .subagents
                    .iter()
                    .find(|sub| sub.id == id)
                    .map(|sub| (sub.title.clone(), sub.answer(), sub.end));
                match found {
                    None => Ready::Settle(
                        format!("wait {id}"),
                        format!("There is no sub-agent '{id}' any more."),
                        true,
                    ),
                    Some((_, _, None)) => return false,
                    Some((title, answer, Some(end))) => {
                        let label = format!("wait {title}");
                        match end {
                            SubagentEnd::Done => Ready::Settle(
                                label,
                                format!("Sub-agent \"{title}\" finished:\n{answer}"),
                                false,
                            ),
                            SubagentEnd::Failed => Ready::Settle(
                                label,
                                format!(
                                    "Sub-agent \"{title}\" failed:\n{}",
                                    if answer.is_empty() {
                                        "it stopped without an answer."
                                    } else {
                                        answer.as_str()
                                    }
                                ),
                                true,
                            ),
                            SubagentEnd::Stopped => Ready::Settle(
                                label,
                                format!("Sub-agent \"{title}\" was stopped."),
                                true,
                            ),
                        }
                    }
                }
            }
            Waiting::BackgroundScript { id, until_ms } => {
                let (id, until_ms) = (id.clone(), *until_ms);
                if let Some((text, failed)) = self.collect_background(&id) {
                    Ready::Settle(format!("script {id}"), text, failed)
                } else if self.now_ms < until_ms {
                    return false;
                } else {
                    Ready::Settle(
                        format!("script {id}"),
                        format!("Script {id} is still running; collect it again later."),
                        false,
                    )
                }
            }
            Waiting::Terminal { before, until_ms } => {
                let (before, until_ms) = (before.clone(), *until_ms);
                let screen = session
                    .as_ref()
                    .map(|access| access.screen_text())
                    .unwrap_or_default();
                let changed = screen != before;
                if !changed && self.now_ms < until_ms {
                    return false;
                }
                let body = if changed {
                    format!("The terminal changed. Current screen:\n{screen}")
                } else {
                    format!("No change within the timeout. Current screen:\n{screen}")
                };
                Ready::Settle("terminal wait".to_owned(), body, false)
            }
        };

        match ready {
            Ready::Settle(label, output, failed) => {
                self.phase = None;
                self.settle(call, turn, label, output, failed, session);
            }
            Ready::ReadRemote { path, max, then } => {
                self.start_read(call, turn, path, max, then, session);
            }
            Ready::SettleImage {
                label,
                output,
                image,
            } => {
                // A script decides what the model sees; an image it read stays out of the
                // conversation until the model asks for it itself. Both reads happen before
                // settling, since answering the script clears the call it was waiting on.
                let scripted = self.waiting_on(&call);
                if !scripted {
                    self.shown_images.push((label.clone(), image));
                }
                self.settle(call, turn, label, output, false, session);
            }
            Ready::WriteRemote(target, payload) => {
                // An edit packs the diff after a NUL so the file keeps its bytes.
                let (content, diff) = match payload.split_once('\u{0}') {
                    Some((content, diff)) => (content.to_owned(), Some(diff.to_owned())),
                    None => (payload, None),
                };
                let Some(access) = session.as_mut() else {
                    self.phase = None;
                    self.settle(
                        call,
                        turn,
                        format!("write {target}"),
                        "The session is not connected.".to_owned(),
                        true,
                        session,
                    );
                    return true;
                };
                access.write_file(&target, content.into_bytes());
                // The upload finishes on a later frame; a diff is the answer.
                if let Some(Turn::Tool { output, .. }) = self.turns.get_mut(turn) {
                    *output = diff
                        .clone()
                        .unwrap_or_else(|| format!("Uploading to {target}…"));
                }
                self.phase = Some(Phase::Running {
                    call,
                    turn,
                    waiting: Waiting::Transfer { target, note: diff },
                });
            }
        }
        true
    }

    /// Whether a tree was mapped without descriptions since the last look, so the pane can
    /// start the mapping pass.
    /// Whether a tool answer asked for the map to be walked or described.
    pub fn deep_map_wanted(&self) -> bool {
        self.deep_map_wanted
    }

    /// Clear the request for a mapping pass once the pane has acted on it.
    pub fn clear_deep_map_wanted(&mut self) {
        self.deep_map_wanted = false;
    }

    /// Note that an answer came from a map that is not there or not described, and say what
    /// the pane starts for it.
    ///
    /// Returns the line the tool answer adds, so the model knows a scan and a mapping pass are
    /// coming and what to do meanwhile.
    fn note_map_needed(&mut self) -> String {
        if self.map.is_empty() {
            self.deep_map_wanted = true;
            return "\nNothing is mapped yet: a scan of the project and a mapping pass are being \
                    started. You will be told when the map is ready."
                .to_owned();
        }
        self.note_shallow_map()
    }

    /// Note that an answer came from a map whose files are listed but not described.
    ///
    /// Returns the line the tool answer adds, so the model knows a mapping pass is coming
    /// and what to do meanwhile.
    fn note_shallow_map(&mut self) -> String {
        if !self.map.needs_summaries() {
            return String::new();
        }
        self.deep_map_wanted = true;
        format!(
            "\n{} is listed but nothing in it has been read yet: a mapping pass is being \
             started to describe the files and what they connect to. You will be told when it \
             is ready.",
            self.map.root
        )
    }

    /// Start a scan the pane asked for, apart from any tool call.
    ///
    /// The map is host state, so the pane can rebuild it without a conversation turn; the
    /// answer lands through [`Agent::poll_map_scan`].
    pub fn start_map_scan(&mut self, root: &str, session: Session<'_, '_>) -> Result<(), String> {
        if self.map_scan.is_some() {
            return Err("A scan is already running.".to_owned());
        }
        let Some(access) = session.as_mut() else {
            return Err("The session is not connected.".to_owned());
        };
        let root = root.trim().to_owned();
        if root == "/" {
            return Err("Pick a directory other than the root to map.".to_owned());
        }
        access.exec(MAP_KEY, &EnvMap::scan_command(&root));
        self.map_scan = Some(root);
        Ok(())
    }

    /// Collect a pane scan that finished. Returns whether the map changed.
    pub fn poll_map_scan(&mut self, session: Session<'_, '_>) -> bool {
        let Some(root) = self.map_scan.clone() else {
            return false;
        };
        let (output, failure) = match session.as_ref() {
            Some(access) => (
                access.output(MAP_KEY).cloned(),
                access.failure(MAP_KEY).map(str::to_owned),
            ),
            None => (None, None),
        };
        if output.is_none() && failure.is_none() {
            return false;
        }
        self.map_scan = None;
        let text = match output {
            Some(output) => format!("{}{}", output.stdout, output.stderr),
            None => failure.unwrap_or_default(),
        };
        self.map = EnvMap::parse(&root, self.now_ms, &text);
        // A fresh listing describes nothing, so the pane follows it with a mapping pass.
        if self.map.needs_summaries() {
            self.deep_map_wanted = true;
        }
        true
    }

    /// A write, an edit or a delete leaves the map stale for that path alone.
    fn forget_map_path(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        self.map.forget(&relative_remote(self.cwd(), path));
        self.map.forget_suffix(path);
    }

    /// The label of a tool turn, for the transcript.
    fn label(&self, turn: usize) -> String {
        self.turns
            .get(turn)
            .and_then(|turn| match turn {
                Turn::Tool { label, .. } => Some(label.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The label of a call: its tool turn's when it has one, else the call's own.
    ///
    /// A script's calls have no tool turn of their own — they live on the script's card —
    /// so the call itself names them.
    fn turn_label(&self, turn: usize, call: &ApiToolCall) -> String {
        self.turns
            .get(turn)
            .and_then(|turn| match turn {
                Turn::Tool { label, .. } => Some(label.clone()),
                _ => None,
            })
            .unwrap_or_else(|| call_label(call))
    }

    /// Record a tool result and move on to the next call or the next reply.
    fn settle(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        label: String,
        output: String,
        failed: bool,
        session: Session<'_, '_>,
    ) {
        // A call a script made is not part of the provider history: its result goes back to
        // the sandbox, which decides what the model ever sees.
        if self.waiting_on(&call) {
            self.answer_script(&call, label, output, failed);
            return;
        }
        if let Some(Turn::Tool {
            label: turn_label,
            output: turn_output,
            failed: turn_failed,
            ..
        }) = self.turns.get_mut(turn)
        {
            *turn_label = label;
            *turn_output = output.clone();
            *turn_failed = failed;
        } else {
            self.turns.push(Turn::Tool {
                name: call.name.clone(),
                label,
                output: output.clone(),
                failed,
            });
        }
        self.history.push(ApiMessage::Tool {
            tool_call_id: call.id.clone(),
            content: output,
        });
        self.advance(session);
    }

    /// Whether the script is waiting on this call.
    fn waiting_on(&self, call: &ApiToolCall) -> bool {
        self.script.as_ref().is_some_and(|run| {
            run.inner
                .as_ref()
                .is_some_and(|(inner, _)| inner.id == call.id)
        })
    }

    /// Hand a finished tool back to the script that asked for it and resume the script.
    fn answer_script(&mut self, call: &ApiToolCall, label: String, output: String, failed: bool) {
        let Some(run) = self.script.as_mut() else {
            return;
        };
        let Some((_, id)) = run.inner.take() else {
            return;
        };
        run.since = None;
        if let Some(row) = run.calls.last_mut()
            && row.name == call.name
        {
            row.label = label;
            row.failed = failed;
            row.output = output.clone();
        }
        // A successful call resolves to the tool's JSON result; a failed one rejects with
        // the message, so `try { ... } catch` reads the way JavaScript expects.
        let text = if failed {
            output
        } else {
            serde_json::json!({ "ok": true, "text": output }).to_string()
        };
        run.job.reply(ScriptReply {
            id,
            ok: !failed,
            text,
        });
        let outer = run.outer.clone();
        let turn = run.turn;
        self.phase = Some(Phase::Running {
            call: outer,
            turn,
            waiting: Waiting::Script,
        });
    }

    /// Run the next queued call, or ask the model again.
    fn advance(&mut self, session: Session<'_, '_>) {
        // While a call holds the app's connections, the call behind it waits: the one that
        // borrowed them starts it again as soon as it lets go.
        if self.on_host {
            self.resume_after_host = true;
            return;
        }
        if let Some(next) = self.queue.pop_front() {
            self.begin_tool(next, session);
        } else {
            // A picture a tool read joins the conversation only once every call of the turn
            // is answered: the provider wants an assistant turn's calls answered before
            // anything else, and a picture in between would leave them unanswered.
            for (content, image) in std::mem::take(&mut self.shown_images) {
                self.history.push(ApiMessage::User {
                    content,
                    images: vec![image],
                });
            }
            self.needs_reply = true;
        }
    }

    /// Handle a finished reply: either the turn is over or tools must run.
    fn finish_reply(
        &mut self,
        turn: usize,
        result: Result<Completion, String>,
        session: Session<'_, '_>,
    ) {
        match result {
            Err(error) => {
                self.error = Some(error.clone());
                self.drop_empty_reply(turn);
                self.turns.push(Turn::Error(error));
            }
            Ok(completion) => {
                if completion.text.is_empty() && completion.tool_calls.is_empty() {
                    // Silence here looks like the pane ignored the prompt.
                    self.error = Some(EMPTY_ANSWER.to_owned());
                    self.drop_empty_reply(turn);
                    self.turns.push(Turn::Error(EMPTY_ANSWER.to_owned()));
                    return;
                }
                if let Some(Turn::Assistant { text, .. }) = self.turns.get_mut(turn)
                    && text.is_empty()
                {
                    text.clone_from(&completion.text);
                }
                let text = self
                    .turns
                    .get(turn)
                    .and_then(|turn| match turn {
                        Turn::Assistant { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                // A reply that only asked for tools has no words of its own to show.
                self.drop_empty_reply(turn);
                self.history.push(ApiMessage::Assistant {
                    content: text,
                    reasoning_content: completion.reasoning_content,
                    tool_calls: completion.tool_calls.clone(),
                });
                self.queue = completion.tool_calls.into_iter().collect();
                if let Some(next) = self.queue.pop_front() {
                    self.begin_tool(next, session);
                }
            }
        }
    }

    /// Drop the reply turn when nothing was written into it, so the transcript shows no
    /// empty bubble in front of the tool cards or the error that follows.
    fn drop_empty_reply(&mut self, turn: usize) {
        if matches!(
            self.turns.get(turn),
            Some(Turn::Assistant { text, .. }) if text.trim().is_empty()
        ) {
            self.turns.remove(turn);
        }
    }

    /// Start one tool call, from any entry point.
    fn begin_tool(&mut self, call: ApiToolCall, session: Session<'_, '_>) {
        self.turns.push(Turn::Tool {
            name: call.name.clone(),
            label: call_label(&call),
            output: String::new(),
            failed: false,
        });
        let turn = self.turns.len() - 1;
        self.dispatch(call, turn, session);
    }

    /// Run one call, or park the agent on whatever it needs.
    fn dispatch(&mut self, call: ApiToolCall, turn: usize, session: Session<'_, '_>) {
        // Nothing to approve in Plan mode: a call that changes something is refused before
        // the user is asked about a tool the chat will not run.
        if self.plan_blocks(&call.name) {
            let label = call_label(&call);
            self.settle(call, turn, label, PLAN_READ_ONLY.to_owned(), true, session);
            return;
        }
        match self.permission(&call.name) {
            Permission::Allow => {}
            Permission::Deny => {
                let label = call_label(&call);
                self.settle(
                    call,
                    turn,
                    label,
                    NOT_PERMITTED.to_owned(),
                    false,
                    session,
                );
                return;
            }
            Permission::Ask => {
                self.phase = Some(Phase::Allowing {
                    label: call_label(&call),
                    call,
                    turn,
                });
                return;
            }
        }
        self.run_tool(call, turn, session);
    }

    /// Answer a capability's permission prompt. "Always" answers are written
    /// to the settings by the pane before calling this.
    pub fn answer_permission(&mut self, allow: bool, session: Session<'_, '_>) {
        let Some(Phase::Allowing { call, turn, label }) = self.phase.take() else {
            return;
        };
        if allow {
            self.run_tool(call, turn, session);
        } else {
            self.settle(call, turn, label, DENIED_BY_USER.to_owned(), false, session);
        }
    }

    /// Allow every prompt for the rest of this session, then run the tool that asked.
    ///
    /// Ask decisions are allowed for the rest of the session; Deny decisions remain final.
    pub fn allow_everything(&mut self, session: Session<'_, '_>) {
        self.allow_all = true;
        self.answer_permission(true, session);
    }

    /// Answer a script's request for more run time. A yes starts it with that budget.
    pub fn answer_extension(&mut self, allow: bool, session: Session<'_, '_>) {
        let Some(Phase::Extending {
            call,
            turn,
            seconds,
            background,
        }) = self.phase.take()
        else {
            return;
        };
        if allow {
            let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
            if background {
                let code = arguments["code"].as_str().unwrap_or_default().to_owned();
                let label = call_label(&call);
                match self.begin_background_script(&code, seconds, session) {
                    Ok(id) => self.settle(
                        call,
                        turn,
                        label,
                        format!(
                            "Started background script {id}. Collect it with script_result and \
                             \"{id}\", and keep working in the meantime."
                        ),
                        false,
                        session,
                    ),
                    Err(problem) => self.settle(call, turn, label, problem, true, session),
                }
            } else {
                self.start_task_script(call, turn, &arguments, session);
            }
        } else {
            let limit = script::MAX_EXEC_MS / 1_000;
            let label = call_label(&call);
            self.settle(
                call,
                turn,
                label,
                format!(
                    "The user refused a {seconds}-second script; it may run for {limit} seconds \
at most."
                ),
                true,
                session,
            );
        }
    }

    /// The decision for `tool`, with this session's blanket answer applied.
    fn permission(&self, tool: &str) -> Permission {
        if self.allow_all && self.permissions.decision(tool) == Permission::Ask {
            Permission::Allow
        } else {
            self.permissions.decision(tool)
        }
    }

    /// Whether Plan mode forbids a call.
    fn plan_blocks(&self, tool: &str) -> bool {
        self.plan && !tools::plan_safe(tool)
    }

    /// Whether a tool should be offered to the model for this session.
    ///
    /// A tool exists, belongs to a scope this conversation loaded, the host may use it, and
    /// — in Plan mode — it only reads. That one answer decides both what the request carries
    /// and what the prompt names.
    pub fn offers_tool(&self, tool: &str) -> bool {
        (tools::is_scope_tool(tool) || self.scopes.holds(tool))
            && self.permission(tool) != Permission::Deny
            && (!self.plan || tools::plan_safe(tool))
    }

    /// Bring a scope's tools into the conversation.
    ///
    /// The answer is what the model reads as the tool result: which scope it got, what it
    /// holds, and what else it may load next. A scope it cannot see yet is refused here —
    /// loading is how the graph is walked, not a way around it.
    pub fn load_scope(&mut self, id: &str) -> Result<String, String> {
        let Some(scope) = scopes::find(id.trim()) else {
            return Err(self.scope_problem(id));
        };
        if self.scopes.contains(scope.id) {
            return Ok(format!("{} is loaded already.", scope.id));
        }
        if !self.reachable(scope) {
            return Err(self.scope_problem(id));
        }
        self.scopes.load(scope);
        Ok(format!(
            "Loaded {} ({} tools): {}.",
            scope.id,
            self.scope_tool_count(scope),
            self.scope_names(scope)
        ))
    }

    /// Take a loaded scope, and everything it nests, back out.
    pub fn unload_scope(&mut self, id: &str) -> Result<String, String> {
        if id == scopes::ROOT {
            return Err(format!(
                "{} holds the commands this conversation runs on and stays loaded.",
                scopes::ROOT
            ));
        }
        if !self.scopes.contains(id.trim()) {
            return Err(self.scope_problem(id));
        }
        let removed = self.scopes.unload(id.trim());
        Ok(format!("Unloaded {}.", removed.join(", ")))
    }

    /// Whether the graph lets this conversation load `scope` next.
    fn reachable(&self, scope: &'static Scope) -> bool {
        scopes::visible(&self.scopes, &self.permissions)
            .iter()
            .any(|candidate| candidate.id == scope.id)
    }

    /// What to tell the model about a scope it asked for and cannot have.
    fn scope_problem(&self, id: &str) -> String {
        let visible = scopes::visible(&self.scopes, &self.permissions);
        let names: Vec<&str> = visible.iter().map(|scope| scope.id).collect();
        if names.is_empty() {
            return format!("'{}' is not available here, and nothing else can be loaded.", id.trim());
        }
        format!(
            "'{}' is not available here. Loadable now: {}.",
            id.trim(),
            names.join(", ")
        )
    }

    /// The tools a scope holds, as the model names them.
    fn scope_names(&self, scope: &'static Scope) -> String {
        let mut names = scope.tools.to_vec();
        if let Some(pane) = scope.pane {
            names.extend(crate::plugin::tools_of(pane).iter().map(|tool| tool.name));
        }
        names.join(", ")
    }

    /// How many tools a scope holds.
    fn scope_tool_count(&self, scope: &'static Scope) -> usize {
        scope.tools.len()
            + scope
                .pane
                .map_or(0, |pane| crate::plugin::tools_of(pane).len())
    }

    /// Session-wide allow answers Ask decisions but leaves command Deny rules intact.
    fn command_decision(&self, command: &str) -> RuleDecision {
        let decision = decide_command(
            command,
            Some(&self.host_allow),
            Some(&self.host_deny),
            Some(&self.app_allow),
            Some(&self.app_deny),
            &self.session_allow,
            &self.session_deny,
        );
        if self.allow_all && decision == RuleDecision::Ask {
            RuleDecision::Allow
        } else {
            decision
        }
    }

    /// Run one call the permissions already allowed, on the host it names.
    ///
    /// A call naming another saved host is confirmed with the user once — the answer can be
    /// remembered per host — and the connection is opened before the call runs there.
    fn run_tool(&mut self, call: ApiToolCall, turn: usize, session: Session<'_, '_>) {
        let host = match self.call_host(&call) {
            Ok(host) => host,
            Err(problem) => {
                let label = call_label(&call);
                self.settle(call, turn, label, problem, true, session);
                return;
            }
        };
        let Some(host) = host else {
            self.with_host(None, session, |agent, session| agent.perform(call, turn, session));
            return;
        };
        match self.host_permission(&host) {
            Permission::Allow => self.reach_host(host, call, turn, session),
            Permission::Ask => {
                self.phase = Some(Phase::Connect { host, call, turn });
            }
            Permission::Deny => {
                let label = call_label(&call);
                let output = format!(
                    "The settings do not let this chat connect to {}.",
                    self.host_name(&host)
                );
                self.settle(call, turn, label, output, true, session);
            }
        }
    }

    /// The host a call runs on, for one that was already let through.
    fn running_host(&self, call: &ApiToolCall) -> Option<String> {
        self.call_host(call).ok().flatten()
    }

    /// The saved host the call names in its `host` argument, if it names one.
    fn call_host(&self, call: &ApiToolCall) -> Result<Option<String>, String> {
        let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
        let asked = arguments["host"]
            .as_str()
            .map(str::trim)
            .unwrap_or_default();
        if asked.is_empty() {
            return Ok(None);
        }
        if !tools::takes_host(&call.name) {
            return Err(format!(
                "{} runs on this tab's host only. Pass `host` to {}.",
                call.name,
                tools::HOST_TOOLS.join(", ")
            ));
        }
        match self.saved_host(asked) {
            Some(id) => Ok(Some(id)),
            None => Err(format!(
                "There is no saved host `{asked}`. Call {TOOL_HOSTS_LIST} to see the ones there are."
            )),
        }
    }

    /// The id of the saved host `asked` names, by id or by display name.
    fn saved_host(&self, asked: &str) -> Option<String> {
        self.hosts
            .iter()
            .find(|host| host.id == asked || host.name.eq_ignore_ascii_case(asked))
            .map(|host| host.id.clone())
    }

    /// Run one call, assuming whatever it needs is already open.
    fn perform(&mut self, call: ApiToolCall, turn: usize, session: Session<'_, '_>) {
        let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
        let name = call.name.clone();
        if !tools::is_tool(&name) {
            self.settle(
                call,
                turn,
                name.clone(),
                format!("The tool {name} is not available."),
                true,
                session,
            );
            return;
        }
        // A tool the conversation never loaded is not in the request, so a model that
        // names one is recalling it from earlier: say which scope brings it back.
        if !tools::is_scope_tool(&name) && !self.scopes.holds(&name) {
            let scope = tools::scope_for(&name).unwrap_or(scopes::ROOT);
            let label = call_label(&call);
            self.settle(
                call,
                turn,
                label,
                format!("{name} is not loaded in this conversation. Call load_scope with \"{scope}\" first."),
                true,
                session,
            );
            return;
        }
        // Script mode offers one tool: everything else is called from inside the program.
        // The calls a script makes are let through — they are what the sandbox asked for.
        if self.script_mode
            && name != TOOL_RUN_SCRIPT
            && name != TOOL_SCRIPT_RESULT
            && !self.waiting_on(&call)
        {
            let label = call_label(&call);
            self.settle(
                call,
                turn,
                label,
                SCRIPT_MODE_ONLY.to_owned(),
                true,
                session,
            );
            return;
        }
        // Plan mode offers read-only tools, so a call that changes something can only come
        // from an earlier turn or a mode switch made under a prompt: refuse it rather than
        // trust the request to have been enough.
        if self.plan_blocks(&name) {
            let label = call_label(&call);
            self.settle(call, turn, label, PLAN_READ_ONLY.to_owned(), true, session);
            return;
        }

        // A script that wants to run past the default asks the user first; a smaller
        // budget needs no answer.
        if name == TOOL_RUN_SCRIPT {
            let seconds = tools::script_seconds(&arguments);
            if seconds > script::MAX_EXEC_MS / 1_000 {
                self.phase = Some(Phase::Extending {
                    call,
                    turn,
                    seconds,
                    background: arguments["background"].as_bool().unwrap_or(false),
                });
                return;
            }
        }

        // A write, an edit or a delete leaves the map stale for that one path.
        if is_writing_tool(&name)
            && self.active_host.is_none()
            && let Some(path) = arguments["path"].as_str()
        {
            self.forget_map_path(path);
        }

        // Everything that needs the session checks once, up front.
        // `run_command` asks for approval first, so it keeps its own check.
        let needs_session_now = tools::needs_session(&name)
            && !matches!(name.as_str(), TOOL_ASK_USER | TOOL_RUN_COMMAND | TOOL_RUN_COMMAND_BACKGROUND);
        if needs_session_now && session.is_none() {
            let label = call_label(&call);
            self.settle(
                call,
                turn,
                label,
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        }

        match name.as_str() {
            TOOL_RUN_SCRIPT => {
                if arguments["background"].as_bool().unwrap_or(false) {
                    let code = arguments["code"].as_str().unwrap_or_default().to_owned();
                    let seconds = tools::script_seconds(&arguments);
                    let label = call_label(&call);
                    match self.begin_background_script(&code, seconds, session) {
                        Ok(id) => self.settle(
                            call,
                            turn,
                            label,
                            format!(
                                "Started background script {id}. Collect it with script_result and \
                                 \"{id}\", and keep working in the meantime."
                            ),
                            false,
                            session,
                        ),
                        Err(problem) => self.settle(call, turn, label, problem, true, session),
                    }
                } else {
                    self.start_task_script(call, turn, &arguments, session);
                }
            }
            TOOL_SCRIPT_RESULT => {
                let id = arguments["id"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                let label = call_label(&call);
                if !self.background.iter().any(|bg| bg.id == id) {
                    let running: Vec<&str> = self
                        .background
                        .iter()
                        .filter(|bg| bg.result.is_none())
                        .map(|bg| bg.id.as_str())
                        .collect();
                    let message = if running.is_empty() {
                        "There is no background script; none are running.".to_owned()
                    } else {
                        format!(
                            "There is no background script '{id}'. Running: {}.",
                            running.join(", ")
                        )
                    };
                    self.settle(call, turn, label, message, true, session);
                    return;
                }
                if let Some((text, failed)) = self.collect_background(&id) {
                    self.settle(call, turn, label, text, failed, session);
                } else {
                    let wait = arguments["waitSeconds"]
                        .as_u64()
                        .unwrap_or(0)
                        .min(MAX_SCRIPT_WAIT_SECONDS);
                    if wait == 0 {
                        self.settle(
                            call,
                            turn,
                            label,
                            format!("Script {id} is still running."),
                            false,
                            session,
                        );
                    } else {
                        self.phase = Some(Phase::Running {
                            call,
                            turn,
                            waiting: Waiting::BackgroundScript {
                                id,
                                until_ms: self.now_ms + wait * 1_000,
                            },
                        });
                    }
                }
            }
            TOOL_LOAD_SCOPE => {
                let asked = arguments["scope"].as_str().unwrap_or_default().to_owned();
                let (output, failed) = match self.load_scope(&asked) {
                    Ok(message) => (message, false),
                    Err(problem) => (problem, true),
                };
                let label = call_label(&call);
                self.settle(call, turn, label, output, failed, session);
            }
            TOOL_UNLOAD_SCOPE => {
                let asked = arguments["scope"].as_str().unwrap_or_default().to_owned();
                let (output, failed) = match self.unload_scope(&asked) {
                    Ok(message) => (message, false),
                    Err(problem) => (problem, true),
                };
                let label = call_label(&call);
                self.settle(call, turn, label, output, failed, session);
            }
            TOOL_ASK_USER => {
                let questions = asked_questions(&arguments);
                self.phase = Some(Phase::Asking { call, questions });
            }
            TOOL_CHANGE_DIR => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                // The wrapper reports the directory it ended in, so a path that is not there
                // fails the call and leaves the working directory where it was.
                let command = format!("cd {} && pwd", shell_quote(&path));
                self.exec_command(call, command, None, turn, session);
            }
            TOOL_HOSTS_LIST => {
                let output = self.host_listing();
                self.settle(call, turn, TOOL_HOSTS_LIST.to_owned(), output, false, session);
            }
            TOOL_RUN_COMMAND => {
                let Some(command) = arguments["command"]
                    .as_str()
                    .map(str::trim)
                    .filter(|command| !command.is_empty())
                    .map(str::to_owned)
                else {
                    self.settle(
                        call,
                        turn,
                        "run_command".to_owned(),
                        "The command argument was missing or empty.".to_owned(),
                        true,
                        session,
                    );
                    return;
                };
                let decision = self.command_decision(&command);
                let sudo_stdin = arguments["sudoStdin"]
                    .as_str()
                    .filter(|password| !password.is_empty())
                    .map(str::to_owned);
                match decision {
                    RuleDecision::Allow => {
                        self.exec_command(call, command, sudo_stdin, turn, session)
                    }
                    RuleDecision::Deny => {
                        let reason =
                            format!("Running `{command}` is not allowed by the permission rules.");
                        self.settle(call, turn, format!("$ {command}"), reason, true, session);
                    }
                    RuleDecision::Ask => {
                        self.phase = Some(Phase::Approving {
                            scopes: approval_scopes(&command),
                            call,
                            command,
                        });
                    }
                }
            }
            TOOL_RUN_COMMAND_BACKGROUND => {
                let Some(command) = arguments["command"]
                    .as_str()
                    .map(str::trim)
                    .filter(|command| !command.is_empty())
                    .map(str::to_owned)
                else {
                    self.settle(
                        call,
                        turn,
                        TOOL_RUN_COMMAND_BACKGROUND.to_owned(),
                        "The command argument was missing or empty.".to_owned(),
                        true,
                        session,
                    );
                    return;
                };
                match self.command_decision(&command) {
                    RuleDecision::Allow => {
                        self.exec_background_command(call, command, turn, session)
                    }
                    RuleDecision::Deny => {
                        let reason =
                            format!("Running `{command}` is not allowed by the permission rules.");
                        self.settle(call, turn, format!("$ {command}"), reason, true, session);
                    }
                    RuleDecision::Ask => {
                        self.phase = Some(Phase::Approving {
                            scopes: approval_scopes(&command),
                            call,
                            command,
                        });
                    }
                }
            }
            TOOL_RUN_SUDO => {
                let Some(command) = arguments["command"]
                    .as_str()
                    .map(str::trim)
                    .filter(|command| !command.is_empty())
                    .map(str::to_owned)
                else {
                    self.settle(
                        call,
                        turn,
                        "run_sudo".to_owned(),
                        "The command argument was missing or empty.".to_owned(),
                        true,
                        session,
                    );
                    return;
                };
                // The password prompt is the approval: it names the command and only the
                // user can answer it. Rules that refuse a command still refuse it here.
                let decision = self.command_decision(&command);
                if decision == RuleDecision::Deny {
                    let reason =
                        format!("Running `{command}` is not allowed by the permission rules.");
                    let label = call_label(&call);
                    self.settle(call, turn, label, reason, true, session);
                    return;
                }
                let reason = arguments["reason"]
                    .as_str()
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
                    .map(str::to_owned);
                self.phase = Some(Phase::Sudo {
                    call,
                    command,
                    reason,
                });
            }
            _ => {
                if let Some(command) = tools::remote_shell_command(&name, &arguments) {
                    self.run_probe(call, turn, CommandThen::Return, command, session);
                    return;
                }
                if let Some(tool) = tools::plugin_tool(&name) {
                    self.run_plugin_tool(call, turn, tool, &arguments, session);
                    return;
                }
                self.dispatch_remote(call, turn, &name, &arguments, session);
            }
        }
    }

    /// Start a script in its sandbox. It reports back through [`Waiting::Script`], and the
    /// calls it makes are dispatched like any other tool's.
    fn start_task_script(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        arguments: &Value,
        session: Session<'_, '_>,
    ) {
        if self.script.is_some() {
            let label = call_label(&call);
            self.settle(
                call,
                turn,
                label,
                "A script cannot start another script; call the tool itself.".to_owned(),
                true,
                session,
            );
            return;
        }
        let Some(code) = arguments["code"]
            .as_str()
            .map(str::trim)
            .filter(|code| !code.is_empty())
            .map(str::to_owned)
        else {
            let label = call_label(&call);
            self.settle(
                call,
                turn,
                label,
                "The code argument was missing or empty.".to_owned(),
                true,
                session,
            );
            return;
        };
        if let Some(slot) = self.turns.get_mut(turn) {
            *slot = Turn::Script {
                code: code.clone(),
                calls: Vec::new(),
                result: None,
                failed: false,
                console: String::new(),
            };
        }
        self.script = Some(ScriptRun {
            outer: call.clone(),
            turn,
            inner: None,
            calls: Vec::new(),
            since: None,
            job: script::spawn_with_budget(&code, tools::script_seconds(arguments) * 1_000),
        });
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Script,
        });
    }

    /// Run a tool a pane offered.
    ///
    /// The call becomes a command on the session's host, so it is approved the way a typed
    /// command would be: a host that allows it runs it, a rule that forbids it refuses, and
    /// anything else asks the user first.
    fn run_plugin_tool(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        tool: &'static crate::plugin::PluginTool,
        arguments: &Value,
        session: Session<'_, '_>,
    ) {
        let command = match (tool.command)(arguments) {
            Ok(command) => command,
            Err(reason) => {
                let label = call_label(&call);
                self.settle(call, turn, label, reason, true, session);
                return;
            }
        };
        match self.command_decision(&command) {
            RuleDecision::Allow => self.exec_command(call, command, None, turn, session),
            RuleDecision::Deny => {
                let reason = format!("Running `{command}` is not allowed by the permission rules.");
                self.settle(call, turn, format!("$ {command}"), reason, true, session);
            }
            RuleDecision::Ask => {
                self.phase = Some(Phase::Approving {
                    scopes: approval_scopes(&command),
                    call,
                    command,
                });
            }
        }
    }

    /// The tools that need the session or a worker thread.
    fn dispatch_remote(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        name: &str,
        arguments: &Value,
        session: Session<'_, '_>,
    ) {
        match name {
            TOOL_REMOTE_READ => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                if arguments["encoding"].as_str() == Some("base64") {
                    self.start_read(call, turn, path, tools::MAX_BASE64_BYTES, ReadThen::Base64, session);
                    return;
                }
                let max = arguments["maxChars"]
                    .as_u64()
                    .unwrap_or(32_000)
                    .clamp(1_000, tools::MAX_FILE_READ_CHARS as u64);
                self.start_read(call, turn, path, max, ReadThen::Return, session);
            }
            TOOL_REMOTE_EDIT | TOOL_DEV_EDIT => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let old = arguments["oldText"].as_str().unwrap_or_default().to_owned();
                let new = arguments["newText"].as_str().unwrap_or_default().to_owned();
                if old.is_empty() {
                    self.settle(
                        call,
                        turn,
                        format!("edit {path}"),
                        "oldText must not be empty.".to_owned(),
                        true,
                        session,
                    );
                    return;
                }
                self.start_read(
                    call,
                    turn,
                    path,
                    tools::MAX_FILE_READ_CHARS as u64,
                    ReadThen::Edit { old, new },
                    session,
                );
            }
            TOOL_DEV_VIEW => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let start = arguments["startLine"].as_u64().unwrap_or(1).max(1) as usize;
                let max_lines = arguments["maxLines"]
                    .as_u64()
                    .map_or(tools::MAX_DEV_VIEW_LINES_DEFAULT, |value| value as usize);
                let lines = arguments["endLine"].as_u64().map_or(max_lines, |end| {
                    (end as usize).saturating_sub(start) + 1
                });
                self.start_read(
                    call,
                    turn,
                    path,
                    tools::MAX_FILE_READ_CHARS as u64,
                    ReadThen::View { start, lines },
                    session,
                );
            }
            TOOL_DEV_IMAGE => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                // One byte over the cap is enough to know the file is too big.
                self.start_read(
                    call,
                    turn,
                    path,
                    super::protocol::MAX_IMAGE_BYTES as u64 + 1,
                    ReadThen::Image,
                    session,
                );
            }
            TOOL_DEV_DIFF => {
                let context = arguments["contextLines"]
                    .as_u64()
                    .map_or(tools::DIFF_CONTEXT_LINES_DEFAULT, |value| value as usize);
                let new = arguments["newText"]
                    .as_str()
                    .or_else(|| arguments["patch"].as_str())
                    .unwrap_or_default()
                    .to_owned();
                if new.is_empty() {
                    let label = call_label(&call);
                    self.settle(
                        call,
                        turn,
                        label,
                        "Pass newText or patch to diff against.".to_owned(),
                        true,
                        session,
                    );
                    return;
                }
                match arguments["oldText"].as_str() {
                    // Both sides came with the call, so the file is not needed.
                    Some(old) => {
                        let body = tools::diff_texts(old, &new, context);
                        let label = call_label(&call);
                        self.settle(call, turn, label, body, false, session);
                    }
                    None => {
                        let path = relative_remote(
                            self.cwd(),
                            arguments["path"].as_str().unwrap_or_default(),
                        );
                        self.start_read(
                            call,
                            turn,
                            path,
                            tools::MAX_FILE_READ_CHARS as u64,
                            ReadThen::Diff { new, context },
                            session,
                        );
                    }
                }
            }
            TOOL_DEV_GREP => {
                let root =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let command = tools::dev_grep_command(arguments, &root);
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_DEV_FIND => {
                let root =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let command = tools::dev_find_command(arguments, &root);
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_REMOTE_WRITE | TOOL_DEV_CREATE => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let content = arguments["content"].as_str().unwrap_or_default().to_owned();
                let overwrite = arguments["overwrite"].as_bool().unwrap_or(false);
                let probe = format!("test -e {} && echo yes || echo no", shell_quote(&path));
                self.run_probe(
                    call,
                    turn,
                    CommandThen::WriteRemote {
                        path,
                        content,
                        overwrite,
                    },
                    probe,
                    session,
                );
            }
            TOOL_REMOTE_LIST | TOOL_DEV_LIST => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                // Only the developer listing walks sub-directories, and it does that
                // with the host's own `find`.
                if arguments["recursive"].as_bool().unwrap_or(false) {
                    let command = tools::dev_tree_command(arguments, &path);
                    self.run_probe(call, turn, CommandThen::Return, command, session);
                    return;
                }
                match session.as_mut() {
                    Some(access) => access.request_listing(&path),
                    None => {
                        self.settle(
                            call,
                            turn,
                            format!("list {path}"),
                            "The session is not connected.".to_owned(),
                            true,
                            session,
                        );
                        return;
                    }
                }
                self.phase = Some(Phase::Running {
                    call,
                    turn,
                    waiting: Waiting::List { path },
                });
            }
            TOOL_REMOTE_DELETE => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let command = format!("rm -rf -- {} && echo removed", shell_quote(&path));
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_REMOTE_STAT => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let command = tools::stat_command(&path);
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_REMOTE_MOVE => {
                let from =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let to = relative_remote(
                    self.cwd(),
                    arguments["destination"].as_str().unwrap_or_default(),
                );
                let overwrite = arguments["overwrite"].as_bool().unwrap_or(false);
                let command = tools::move_command(&from, &to, overwrite);
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_REMOTE_PATCH => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let patch = arguments["patch"].as_str().unwrap_or_default().trim().to_owned();
                let patch_file = arguments["patchFile"].as_str().unwrap_or_default().trim();
                if !patch.is_empty() {
                    self.start_read(
                        call,
                        turn,
                        path,
                        tools::MAX_FILE_READ_CHARS as u64,
                        ReadThen::Patch { patch },
                        session,
                    );
                } else if !patch_file.is_empty() {
                    let patch_path = relative_remote(self.cwd(), patch_file);
                    let command = format!("cat {}", shell_quote(&patch_path));
                    self.run_probe(
                        call,
                        turn,
                        CommandThen::ReadPatchFile { path, patch_path },
                        command,
                        session,
                    );
                } else {
                    let label = call_label(&call);
                    self.settle(
                        call,
                        turn,
                        label,
                        "Pass patch or patchFile.".to_owned(),
                        true,
                        session,
                    );
                }
            }
            TOOL_REMOTE_ZIP_LIST => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let command = tools::zip_list_command(&path);
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_REMOTE_ZIP_READ => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let entry = arguments["entry"].as_str().unwrap_or_default();
                let command = tools::zip_read_command(&path, entry);
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_REMOTE_ZIP_EXTRACT => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let destination = relative_remote(
                    self.cwd(),
                    arguments["destination"].as_str().unwrap_or_default(),
                );
                let overwrite = arguments["overwrite"].as_bool().unwrap_or(false);
                let command = tools::zip_extract_command(&path, &destination, overwrite);
                self.run_probe(call, turn, CommandThen::Return, command, session);
            }
            TOOL_GIT_STATUS
            | TOOL_GIT_DIFF
            | TOOL_GIT_LOG
            | TOOL_GIT_BRANCHES
            | TOOL_GIT_COMMIT
            | TOOL_GIT_CHECKOUT => {
                let dir =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                match tools::git_command(name, arguments, &dir) {
                    Some(command) => {
                        self.run_probe(call, turn, CommandThen::Return, command, session)
                    }
                    None => {
                        let label = call_label(&call);
                        self.settle(
                            call,
                            turn,
                            label,
                            format!("{name} is not a git tool."),
                            true,
                            session,
                        );
                    }
                }
            }
            TOOL_TERMINAL_READ => {
                let screen = session
                    .as_ref()
                    .map(|access| access.screen_text())
                    .unwrap_or_default();
                let lines: Vec<&str> = screen.lines().collect();
                let keep = arguments["maxLines"]
                    .as_u64()
                    .map_or(lines.len(), |value| value as usize)
                    .clamp(1, 5_000);
                let from = lines.len().saturating_sub(keep);
                let body = if lines.is_empty() {
                    "The terminal is empty.".to_owned()
                } else {
                    lines[from..].join("\n")
                };
                self.settle(call, turn, "terminal_read".to_owned(), body, false, session);
            }
            TOOL_TERMINAL_WRITE | TOOL_TERMINAL_KEYS => {
                let text = if name == TOOL_TERMINAL_KEYS {
                    match tools::keys_sequence(arguments["keys"].as_str().unwrap_or_default()) {
                        Ok(sequence) => sequence,
                        Err(error) => {
                            let label = call_label(&call);
                            self.settle(call, turn, label, error, true, session);
                            return;
                        }
                    }
                } else {
                    arguments["text"].as_str().unwrap_or_default().to_owned()
                };
                let enter = arguments["pressEnter"].as_bool().unwrap_or(false);
                let label = call_label(&call);
                let Some(access) = session.as_mut() else {
                    self.settle(
                        call,
                        turn,
                        label,
                        "The session is not connected.".to_owned(),
                        true,
                        session,
                    );
                    return;
                };
                access.type_text(&text);
                if enter {
                    access.type_text("\r");
                }
                let note = if name == TOOL_TERMINAL_KEYS {
                    format!("Sent {}.", arguments["keys"].as_str().unwrap_or_default())
                } else {
                    format!(
                        "Typed {} characters{}.",
                        text.chars().count(),
                        if enter { " and Enter" } else { "" }
                    )
                };
                self.settle(call, turn, label, note, false, session);
            }
            TOOL_TERMINAL_WAIT => {
                let timeout = arguments["timeoutMs"]
                    .as_u64()
                    .unwrap_or(TERMINAL_WAIT_DEFAULT_MS)
                    .clamp(100, tools::MAX_TERMINAL_WAIT_MS);
                let before = session
                    .as_ref()
                    .map(|access| access.screen_text())
                    .unwrap_or_default();
                self.phase = Some(Phase::Running {
                    call,
                    turn,
                    waiting: Waiting::Terminal {
                        before,
                        until_ms: self.now_ms + timeout,
                    },
                });
            }
            TOOL_LOCAL_RUN => {
                let arguments = arguments.clone();
                self.spawn_worker(call, turn, move || tools::run_local_command(&arguments));
            }
            TOOL_WEB_FETCH => {
                let url = arguments["url"].as_str().unwrap_or_default().to_owned();
                let max = arguments["maxChars"].as_u64();
                self.spawn_worker(call, turn, move || web::web_fetch(&url, max));
            }
            TOOL_WEB_SEARCH => {
                let query = arguments["query"].as_str().unwrap_or_default().to_owned();
                let provider = arguments["provider"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| self.tools.web_provider.clone());
                let key = arguments["apiKey"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| self.tools.web_key.clone());
                let limit = arguments["limit"].as_u64();
                self.spawn_worker(call, turn, move || {
                    web::web_search(&query, provider.as_deref(), key.as_deref(), limit)
                });
            }
            TOOL_RAG_SEARCH => {
                let query = arguments["query"].as_str().unwrap_or_default().to_owned();
                let limit = arguments["limit"]
                    .as_u64()
                    .map_or(5, |value| value as usize);
                let folders = self.tools.rag_folders.clone();
                let embed = self.tools.embed.clone();
                self.spawn_worker(call, turn, move || {
                    let index = rag::cached(&folders, embed.as_ref())?;
                    let query_vector = match &embed {
                        Some(config) => match embeddings(config, std::slice::from_ref(&query)) {
                            Ok(mut vectors) if !vectors.is_empty() => Some(vectors.remove(0)),
                            Ok(_) => None,
                            Err(error) => return Err(error),
                        },
                        None => None,
                    };
                    let hits = rag::search(&index, &query, query_vector.as_deref(), limit);
                    Ok(rag::format_hits(&query, &hits))
                });
            }
            TOOL_ENV_SCAN => {
                // No path means "map this project": the host finds the root itself, so a
                // session opened deep inside a tree maps the tree and not that directory.
                let root = arguments["path"]
                    .as_str()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(|path| relative_remote(self.cwd(), path))
                    .or_else(|| (!self.map.root.is_empty()).then(|| self.map.root.clone()))
                    .unwrap_or_default();
                // The scan is the one map tool that touches the host: one walk, no reads.
                let command = EnvMap::scan_command(&root);
                self.run_probe(call, turn, CommandThen::ScanMap { root }, command, session);
            }
            TOOL_ENV_FIND => {
                let query = arguments["query"].as_str().unwrap_or_default();
                let limit = arguments["limit"]
                    .as_u64()
                    .map_or(ENV_FIND_HITS_DEFAULT, |value| value as usize);
                let hits = self.map.find(query, limit);
                let (output, failed) = if hits.is_empty() {
                    (
                        format!(
                            "Nothing in the environment map matches \"{query}\". Scan a \
                             directory with env_scan, or search the files themselves."
                        ),
                        false,
                    )
                } else {
                    (hits.join("\n"), false)
                };
                let output = format!("{output}{}", self.note_map_needed());
                let label = call_label(&call);
                self.settle(call, turn, label, output, failed, session);
            }
            TOOL_ENV_OUTLINE => {
                let path =
                    relative_remote(self.cwd(), arguments["path"].as_str().unwrap_or_default());
                let (output, failed) = match self.map.outline(&path) {
                    Some(outline) => (outline, false),
                    None => (
                        format!(
                            "{path} is not in the environment map. Scan its directory with \
                             env_scan, or read it with dev_view_file."
                        ),
                        true,
                    ),
                };
                let output = format!("{output}{}", self.note_map_needed());
                let label = call_label(&call);
                self.settle(call, turn, label, output, failed, session);
            }
            TOOL_ENV_OVERVIEW => {
                let output = format!("{}{}", self.map.overview(), self.note_map_needed());
                let failed = self.map.is_empty();
                let label = call_label(&call);
                self.settle(call, turn, label, output, failed, session);
            }
            TOOL_ENV_SEARCH => {
                let query = arguments["query"].as_str().unwrap_or_default();
                let limit = arguments["limit"]
                    .as_u64()
                    .map_or(ENV_SEARCH_HITS_DEFAULT, |value| value as usize);
                let hits = self.map.search(query, limit);
                let (output, failed) = if hits.is_empty() {
                    (
                        format!(
                            "Nothing in the environment map answers \"{query}\". Try \
                             env_find, or env_scan the directory if it is not mapped."
                        ),
                        false,
                    )
                } else {
                    (hits.join("\n"), false)
                };
                let output = format!("{output}{}", self.note_map_needed());
                let label = call_label(&call);
                self.settle(call, turn, label, output, failed, session);
            }
            TOOL_ENV_SYSTEM => {
                // One scan per conversation is enough: it is host state, and env_system
                // returns what it holds rather than re-reading the host every turn.
                if self.system.is_empty() {
                    let command = SystemFacts::scan_command();
                    self.run_probe(call, turn, CommandThen::ScanSystem, command, session);
                } else {
                    let output = self.system.summary();
                    let label = call_label(&call);
                    self.settle(call, turn, label, output, false, session);
                }
            }
            TOOL_SPAWN_SUBAGENT => {
                let label = call_label(&call);
                if self.subagent {
                    self.settle(
                        call,
                        turn,
                        label,
                        NO_NESTED_SUBAGENTS.to_owned(),
                        true,
                        session,
                    );
                    return;
                }
                let prompt = arguments["prompt"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if prompt.is_empty() {
                    self.settle(
                        call,
                        turn,
                        label,
                        "spawn_subagent needs a prompt.".to_owned(),
                        true,
                        session,
                    );
                    return;
                }
                let running = self.subagents.iter().filter(|sub| sub.running()).count();
                if running >= self.max_subagents {
                    self.settle(
                        call,
                        turn,
                        label,
                        format!(
                            "{running} sub-agents are running, which is the limit. Wait for \
                             one with wait_subagent before starting another."
                        ),
                        true,
                        session,
                    );
                    return;
                }
                let title = arguments["title"]
                    .as_str()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| subagent_title(&prompt));
                self.spawn_request = Some(SpawnRequest {
                    call,
                    turn,
                    title,
                    prompt,
                });
            }
            TOOL_WAIT_SUBAGENT => {
                let id = arguments["id"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                let label = call_label(&call);
                if !self.subagents.iter().any(|sub| sub.id == id) {
                    self.settle(
                        call,
                        turn,
                        label,
                        format!("There is no sub-agent '{id}'."),
                        true,
                        session,
                    );
                    return;
                }
                self.phase = Some(Phase::Running {
                    call,
                    turn,
                    waiting: Waiting::Subagent { id },
                });
            }
            _ => {
                let label = call_label(&call);
                match tools::run_local(name, arguments, self.now_ms) {
                    Ok(output) => self.settle(call, turn, label, output, false, session),
                    Err(error) => self.settle(call, turn, label, error, true, session),
                }
            }
        }
    }

    /// Whether a spawn is waiting for the pane to build its first request.
    pub fn spawning(&self) -> bool {
        self.spawn_request.is_some()
    }

    /// Take the spawn the pane has to launch.
    pub fn take_spawn_request(&mut self) -> Option<SpawnRequest> {
        self.spawn_request.take()
    }

    /// A child conversation carrying this one's directory, scopes and rules.
    fn child_agent(&self) -> Agent {
        let mut agent = Agent::new();
        agent.cwd = self.cwd.clone();
        agent.scopes = self.scopes.clone();
        agent.map = self.map.clone();
        agent.system = self.system.clone();
        agent.permissions = self.permissions.clone();
        agent.remote = self.remote.clone();
        agent.hosts = self.hosts.clone();
        agent.reach_allow = self.reach_allow.clone();
        agent.reach_deny = self.reach_deny.clone();
        agent.app_allow = self.app_allow.clone();
        agent.app_deny = self.app_deny.clone();
        agent.host_allow = self.host_allow.clone();
        agent.host_deny = self.host_deny.clone();
        agent.session_allow = self.session_allow.clone();
        agent.session_deny = self.session_deny.clone();
        agent.allow_all = self.allow_all;
        agent.plan = self.plan;
        agent.script_mode = self.script_mode;
        agent.tools = self.tools.clone();
        agent.max_subagents = self.max_subagents;
        agent
    }

    /// Create the sub-agent `request` describes, returning its index in [`Agent::subagents`].
    ///
    /// It starts with this conversation's working directory, scopes, permissions and rules;
    /// the rules are re-read every frame, so later answers here reach it too.
    pub fn begin_subagent(&mut self, title: &str, prompt: &str) -> usize {
        self.next_subagent += 1;
        let id = format!("{SUBAGENT_ID_PREFIX}-{}", self.next_subagent);
        let mut agent = self.child_agent();
        agent.subagent = true;
        agent.keys = CommandKeys::subagent(&id);
        agent.push_user(prompt, UserMeta::default());
        self.subagents.push(Subagent {
            id: id.clone(),
            title: title.to_owned(),
            prompt: prompt.to_owned(),
            agent,
            end: None,
            waiting_announced: false,
        });
        self.subagents.len() - 1
    }

    /// Hand a launched sub-agent its first request, answering the call that spawned it.
    pub fn launch_subagent(
        &mut self,
        index: usize,
        request: CompletionRequest,
        call: ApiToolCall,
        turn: usize,
        session: &mut Option<&mut dyn SessionAccess>,
    ) {
        let Some(sub) = self.subagents.get_mut(index) else {
            return;
        };
        let id = sub.id.clone();
        let title = sub.title.clone();
        sub.agent.start(request);
        let label = call_label(&call);
        self.settle(
            call,
            turn,
            label,
            format!(
                "Started sub-agent \"{title}\" ({id}). Its work is listed with the chats; \
                 call wait_subagent with \"{id}\" to collect its answer."
            ),
            false,
            session,
        );
    }

    /// Answer a spawn whose request could not be built.
    pub fn fail_spawn(
        &mut self,
        request: SpawnRequest,
        reason: String,
        session: &mut Option<&mut dyn SessionAccess>,
    ) {
        let label = call_label(&request.call);
        self.settle(request.call, request.turn, label, reason, true, session);
    }

    /// How many sub-agents are still working.
    pub fn subagents_running(&self) -> usize {
        self.subagents.iter().filter(|sub| sub.running()).count()
    }

    /// The first sub-agent waiting for the user.
    pub fn waiting_subagent(&self) -> Option<usize> {
        self.subagents
            .iter()
            .position(|sub| sub.state() == SubagentState::NeedsAnswer)
    }

    /// Take back the sub-agents a stored chat kept, continuing their numbering.
    pub fn adopt_subagents(&mut self, subagents: Vec<Subagent>) {
        self.next_subagent = subagents
            .iter()
            .filter_map(|sub| sub.id.rsplit('-').next()?.parse::<usize>().ok())
            .max()
            .unwrap_or(0);
        self.subagents = subagents;
    }

    /// Fold the answers a sub-agent was given into this conversation's rules.
    pub fn adopt_subagent_rules(&mut self, index: usize) {
        let Some(sub) = self.subagents.get(index) else {
            return;
        };
        let (allow_all, allow, deny) = (
            sub.agent.allow_all,
            sub.agent.session_allow.clone(),
            sub.agent.session_deny.clone(),
        );
        self.adopt_rules(allow_all, allow, deny);
    }

    /// Take the blanket and session answers into this conversation.
    fn adopt_rules(&mut self, allow_all: bool, allow: Vec<String>, deny: Vec<String>) {
        self.allow_all |= allow_all;
        if self.session_allow != allow {
            self.session_allow = allow;
        }
        if self.session_deny != deny {
            self.session_deny = deny;
        }
    }

    /// Stop every sub-agent still working, answering with how many were stopped.
    pub fn stop_subagents(&mut self) -> usize {
        let mut stopped = 0;
        for sub in &mut self.subagents {
            if !sub.running() {
                continue;
            }
            sub.agent.stop();
            sub.end = Some(SubagentEnd::Stopped);
            stopped += 1;
        }
        if stopped > 0 {
            self.turns
                .push(Turn::Notice(format!("Stopped {stopped} sub-agent(s).")));
        }
        stopped
    }

    /// Start a script that runs beside the conversation, and answer with its id.
    fn begin_background_script(
        &mut self,
        code: &str,
        seconds: u64,
        session: Session<'_, '_>,
    ) -> Result<String, String> {
        if self.subagent {
            return Err(NO_NESTED_SUBAGENTS.to_owned());
        }
        if self.background.len() >= MAX_BACKGROUND_SCRIPTS {
            return Err(format!(
                "{MAX_BACKGROUND_SCRIPTS} background scripts are already running. Collect one \
                 with script_result before starting another."
            ));
        }
        let code = code.trim();
        if code.is_empty() {
            return Err("The code argument was missing or empty.".to_owned());
        }
        self.next_background += 1;
        let id = format!("s{}", self.next_background);
        let mut agent = self.child_agent();
        // A script gets no model of its own and starts no sub-agents of its own.
        agent.subagent = true;
        agent.script_mode = false;
        agent.keys = CommandKeys::subagent(&format!("bg-{id}"));
        let call = ApiToolCall {
            id: format!("bg-{id}"),
            name: TOOL_RUN_SCRIPT.to_owned(),
            arguments: serde_json::json!({ "code": code, "timeoutSeconds": seconds }).to_string(),
        };
        let arguments = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
        agent.start_task_script(call, 0, &arguments, session);
        self.background.push(BackgroundScript {
            id: id.clone(),
            agent,
            result: None,
            announced: false,
        });
        Ok(id)
    }

    /// Take a finished background script's result, dropping it so its slot frees up.
    fn collect_background(&mut self, id: &str) -> Option<(String, bool)> {
        let index = self.background.iter().position(|bg| bg.id == id)?;
        let result = self.background[index].result.clone()?;
        self.background.remove(index);
        Some(result)
    }

    /// The first background script waiting for the user.
    pub fn waiting_background(&self) -> Option<usize> {
        self.background
            .iter()
            .position(|bg| bg.result.is_none() && bg.agent.needs_user())
    }

    /// Stop a background script whose prompt is on screen.
    pub fn stop_background(&mut self, index: usize) {
        let Some(bg) = self.background.get_mut(index) else {
            return;
        };
        bg.agent.stop();
        if bg.result.is_none() {
            bg.result = Some(("The script was stopped by the user.".to_owned(), true));
            bg.announced = true;
        }
    }

    /// Fold the answers a background script was given into this conversation's rules.
    pub fn adopt_background_rules(&mut self, index: usize) {
        let Some(bg) = self.background.get(index) else {
            return;
        };
        let (allow_all, allow, deny) = (
            bg.agent.allow_all,
            bg.agent.session_allow.clone(),
            bg.agent.session_deny.clone(),
        );
        self.adopt_rules(allow_all, allow, deny);
    }

    /// The directory the call being dispatched resolves paths against.
    ///
    /// A call aimed at another host works in that host's own directory, which is wherever
    /// the last command there left the shell.
    fn cwd(&self) -> &str {
        match &self.active_host {
            Some(host) => self.host_cwds.get(host).map_or("", String::as_str),
            None => &self.cwd,
        }
    }

    /// Remember where a command left the shell, on the host it ran on.
    ///
    /// Returns whether it moved, which is what tells the model its directory changed.
    fn remember_dir(&mut self, host: Option<&str>, directory: String) -> bool {
        let current = match host {
            Some(host) => self.host_cwds.get(host).map_or("", String::as_str),
            None => self.cwd.as_str(),
        };
        if directory.is_empty() || directory == current {
            return false;
        }
        match host {
            Some(host) => {
                self.host_cwds.insert(host.to_owned(), directory);
            }
            None => self.cwd = directory,
        }
        true
    }

    /// Take the app's connections to other hosts, as the pane hands them in every frame.
    pub fn set_remote(&mut self, remote: Option<Rc<RefCell<dyn RemoteHosts>>>) {
        self.hosts = remote
            .as_ref()
            .map(|remote| remote.borrow().hosts().to_vec())
            .unwrap_or_default();
        self.remote = remote;
    }

    /// What the pane should show for a connection on its way, and which host it is for.
    pub fn host_card(&self) -> Option<(HostCard, &RemoteHost)> {
        let (host, card) = match &self.phase {
            Some(Phase::Connect { host, .. }) => (host, HostCard::Ask),
            Some(Phase::Remote { host, .. }) => {
                let card = match self.host_state(host)? {
                    RemoteState::NeedsPassword => HostCard::Password,
                    RemoteState::Failed(_) => return None,
                    _ => HostCard::Connecting,
                };
                (host, card)
            }
            _ => return None,
        };
        self.hosts
            .iter()
            .find(|saved| &saved.id == host)
            .map(|saved| (card, saved))
    }

    /// Where the connection to `host` stands, as the app reports it.
    fn host_state(&self, host: &str) -> Option<RemoteState> {
        let remote = self.remote.as_ref()?;
        Some(remote.borrow().state(&host_handle(host)))
    }

    /// Hand the app the password the user typed for a host that has none in the vault.
    pub fn supply_host_password(&mut self, password: &str) {
        let Some(Phase::Remote { host, .. }) = &self.phase else {
            return;
        };
        let handle = host_handle(host);
        let Some(remote) = self.remote.clone() else {
            return;
        };
        remote.borrow_mut().supply_password(&handle, password);
    }

    /// Answer the card that asks whether this chat may reach another host.
    pub fn answer_connect(&mut self, allow: bool, session: Session<'_, '_>) {
        let Some(Phase::Connect { host, call, turn }) = self.phase.take() else {
            return;
        };
        if allow {
            self.reach_host(host, call, turn, session);
            return;
        }
        let label = call_label(&call);
        let output = format!(
            "The user did not allow this chat to connect to {}.",
            self.host_name(&host)
        );
        self.settle(call, turn, label, output, true, session);
    }

    /// The decision for reaching `host`, with this session's blanket answer applied.
    ///
    /// Every saved host is asked about unless the user answered about it, and a denial is
    /// final: a connection is the one thing a session-wide "allow" cannot open by itself.
    fn host_permission(&self, host: &str) -> Permission {
        if self.reach_deny.iter().any(|saved| saved == host) {
            return Permission::Deny;
        }
        if self.reach_allow.iter().any(|saved| saved == host) || self.allow_all {
            return Permission::Allow;
        }
        Permission::Ask
    }

    /// How a saved host reads in a message.
    fn host_name(&self, host: &str) -> String {
        self.hosts
            .iter()
            .find(|saved| saved.id == host)
            .map(|saved| saved.name.clone())
            .unwrap_or_else(|| host.to_owned())
    }

    /// Run a call on a saved host, opening the connection first when it is not open.
    fn reach_host(
        &mut self,
        host: String,
        call: ApiToolCall,
        turn: usize,
        session: Session<'_, '_>,
    ) {
        let Some(remote) = self.remote.clone() else {
            let label = call_label(&call);
            let output = format!(
                "This pane has no connection to {}, so the call did not run.",
                self.host_name(&host)
            );
            self.settle(call, turn, label, output, true, session);
            return;
        };
        let handle = host_handle(&host);
        let state = remote.borrow().state(&handle);
        match state {
            RemoteState::Open { .. } => {
                // Waiting for a connection is not the call's own time to answer, so a
                // script's clock starts when the call actually runs.
                if self.waiting_on(&call)
                    && let Some(run) = self.script.as_mut()
                {
                    run.since = Some(std::time::Instant::now());
                }
                self.with_host(Some(host), session, |agent, session| {
                    agent.perform(call, turn, session)
                });
            }
            RemoteState::Failed(reason) => {
                let label = call_label(&call);
                self.settle(call, turn, label, reason, true, session);
            }
            RemoteState::Idle => {
                remote.borrow_mut().connect(&handle, &host);
                self.phase = Some(Phase::Remote { host, call, turn });
            }
            RemoteState::Connecting | RemoteState::NeedsPassword => {
                self.phase = Some(Phase::Remote { host, call, turn });
            }
        }
    }

    /// Run `job` with the session of the host a call names, or with this tab's own.
    ///
    /// The app's connections are lent out for the length of the call, so a call queued
    /// behind it starts once they are back.
    fn with_host(
        &mut self,
        host: Option<String>,
        session: Session<'_, '_>,
        job: impl FnOnce(&mut Self, Session<'_, '_>),
    ) {
        let Some(host) = host else {
            self.active_host = None;
            job(self, session);
            return;
        };
        let Some(remote) = self.remote.clone() else {
            self.active_host = Some(host);
            job(self, session);
            return;
        };
        self.active_host = Some(host.clone());
        let held = std::mem::replace(&mut self.on_host, true);
        {
            let mut borrowed = remote.borrow_mut();
            let mut side = borrowed.session(&host_handle(&host));
            job(self, &mut side);
        }
        self.on_host = held;
        if std::mem::take(&mut self.resume_after_host) {
            self.advance(session);
        }
    }

    /// Every saved host and how to work on one, as the model reads it.
    fn host_listing(&self) -> String {
        if self.hosts.is_empty() {
            return "No other hosts are saved, so everything runs on this tab's host.".to_owned();
        }
        let mut lines = vec!["Saved hosts:".to_owned()];
        for host in &self.hosts {
            let mut line = format!("- {} — {} ({})", host.id, host.name, host.address);
            if !host.tags.is_empty() {
                line.push_str(&format!(", tags: {}", host.tags.join(", ")));
            }
            match self.host_permission(&host.id) {
                Permission::Deny => line.push_str(", not allowed from here"),
                Permission::Allow => line.push_str(", allowed"),
                Permission::Ask => {}
            }
            if let Some(problem) = &host.problem {
                line.push_str(&format!(", cannot be opened: {problem}"));
            }
            lines.push(line);
        }
        lines.push(
            "Pass a host's id as `host` to run_command, set_working_directory,              remote_fs_read_file, remote_fs_write_file, remote_fs_list_dir or remote_fs_stat              to work there; the first call to a host asks the user to allow the connection."
                .to_owned(),
        );
        lines.join("\n")
    }

    fn exec_command(
        &mut self,
        call: ApiToolCall,
        command: String,
        sudo_stdin: Option<String>,
        turn: usize,
        session: Session<'_, '_>,
    ) {
        // A command that calls `sudo` cannot answer the password prompt itself, so it
        // waits for the user instead of running blind.
        if sudo_stdin.is_none() && command_needs_sudo(&command) {
            self.phase = Some(Phase::Sudo {
                call,
                command,
                reason: None,
            });
            return;
        }
        let Some(access) = session.as_mut() else {
            self.settle(
                call,
                turn,
                format!("$ {command}"),
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        };
        match &sudo_stdin {
            Some(password) => {
                let wrapped = wrap_command_with_sudo(self.cwd(), &command);
                access.exec_input(&self.keys.exec, &wrapped, password);
            }
            None => {
                let wrapped = wrap_command(self.cwd(), &command);
                access.exec(&self.keys.exec, &wrapped);
            }
        }
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Command {
                key: self.keys.exec.clone(),
                then: CommandThen::Return,
                host: self.active_host.clone(),
            },
        });
    }

    fn exec_background_command(
        &mut self,
        call: ApiToolCall,
        command: String,
        turn: usize,
        session: Session<'_, '_>,
    ) {
        let Some(access) = session.as_mut() else {
            self.settle(
                call,
                turn,
                format!("$ {command}"),
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        };
        let launched = format!(
            "(nohup sh -c {} >/dev/null 2>&1 </dev/null & echo \"Background PID: $!\")",
            shell_quote(&command)
        );
        let wrapped = wrap_command(self.cwd(), &launched);
        access.exec(&self.keys.exec, &wrapped);
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Command {
                key: self.keys.exec.clone(),
                then: CommandThen::Return,
                host: self.active_host.clone(),
            },
        });
    }

    /// Run a probe command on the session.
    fn run_probe(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        then: CommandThen,
        command: String,
        session: Session<'_, '_>,
    ) {
        let label = call_label(&call);
        let Some(access) = session.as_mut() else {
            self.settle(
                call,
                turn,
                label,
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        };
        access.exec(&self.keys.probe, &command);
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Command {
                key: self.keys.probe.clone(),
                then,
                host: self.active_host.clone(),
            },
        });
    }

    /// Start a remote read.
    fn start_read(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        path: String,
        max: u64,
        then: ReadThen,
        session: Session<'_, '_>,
    ) {
        let Some(access) = session.as_mut() else {
            self.settle(
                call,
                turn,
                format!("read {path}"),
                "The session is not connected.".to_owned(),
                true,
                session,
            );
            return;
        };
        access.read(&path, max);
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Read { path, max, then },
        });
    }

    /// Run `job` on a worker thread and wait for its answer.
    fn spawn_worker(
        &mut self,
        call: ApiToolCall,
        turn: usize,
        job: impl FnOnce() -> ToolResult + Send + 'static,
    ) {
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(job());
        });
        self.phase = Some(Phase::Running {
            call,
            turn,
            waiting: Waiting::Worker { events },
        });
    }

    /// Answer a pending approval.
    pub fn approve(&mut self, allow: bool, remember: Option<usize>, session: Session<'_, '_>) {
        let Some(Phase::Approving {
            call,
            command,
            scopes,
        }) = self.phase.take()
        else {
            return;
        };
        let turn = self.turn_for(&call);
        if allow {
            if let Some(index) = remember
                && let Some(scope) = scopes.get(index)
            {
                self.session_allow.push(scope.pattern.clone());
            }
            let host = self.running_host(&call);
            self.with_host(host, session, |agent, session| {
                agent.exec_command(call, command, None, turn, session)
            });
        } else {
            if let Some(index) = remember
                && let Some(scope) = scopes.get(index)
            {
                self.session_deny.push(scope.pattern.clone());
            }
            self.settle(
                call,
                turn,
                format!("$ {command}"),
                format!("The user refused to run `{command}`."),
                true,
                session,
            );
        }
    }

    /// Answer the sudo prompt: run the command with the password on stdin. An empty
    /// password tries passwordless `sudo`, like the reference.
    pub fn submit_sudo(&mut self, password: &str, session: Session<'_, '_>) {
        let Some(Phase::Sudo { call, command, .. }) = self.phase.take() else {
            return;
        };
        let turn = self.turn_for(&call);
        let password = password.trim().to_owned();
        let host = self.running_host(&call);
        self.with_host(host, session, |agent, session| {
            agent.exec_command(call, command, Some(password), turn, session)
        });
    }

    /// Refuse the sudo prompt; the model is told the command did not run.
    pub fn cancel_sudo(&mut self, session: Session<'_, '_>) {
        let Some(Phase::Sudo { call, command, .. }) = self.phase.take() else {
            return;
        };
        let turn = self.turn_for(&call);
        self.settle(
            call,
            turn,
            format!("$ sudo {command}"),
            "The user did not give a sudo password, so the command did not run.".to_owned(),
            true,
            session,
        );
    }

    /// Allow future Ask decisions for the rest of this session, then run this command.
    pub fn approve_everything(&mut self, session: Session<'_, '_>) {
        let Some(Phase::Approving { call, command, .. }) = self.phase.take() else {
            return;
        };
        self.allow_all = true;
        let turn = self.turn_for(&call);
        let host = self.running_host(&call);
        self.with_host(host, session, |agent, session| {
            agent.exec_command(call, command, None, turn, session)
        });
    }

    /// The transcript line belonging to a call with no answer yet.
    fn turn_for(&self, call: &ApiToolCall) -> usize {
        let wanted = call_label(call);
        self.turns
            .iter()
            .rposition(|turn| {
                matches!(turn, Turn::Tool { label, output, .. } if *label == wanted && output.is_empty())
            })
            .unwrap_or_else(|| self.turns.len().saturating_sub(1))
    }

    /// Answer the agent's questions, in the order they were asked.
    ///
    /// Every question needs an answer; anything less leaves the agent waiting.
    pub fn answer(&mut self, answers: &[String], session: Session<'_, '_>) {
        let Some(Phase::Asking { call, questions }) = self.phase.take() else {
            return;
        };
        let answers: Vec<String> = answers
            .iter()
            .map(|answer| answer.trim().to_owned())
            .collect();
        if answers.len() != questions.len() || answers.iter().any(String::is_empty) {
            self.phase = Some(Phase::Asking { call, questions });
            return;
        }
        let turn = self.turn_for(&call);
        for (question, answer) in questions.iter().zip(&answers) {
            self.turns.push(Turn::Question {
                text: question.text.clone(),
                answer: Some(answer.clone()),
            });
        }
        let result = questions
            .iter()
            .zip(&answers)
            .map(|(question, answer)| format!("{}: {}", question.text, answer))
            .collect::<Vec<_>>()
            .join("\n");
        self.settle(call, turn, "ask_user".to_owned(), result, false, session);
    }

    /// Stop whatever the run is doing.
    ///
    /// A tool that was in flight is answered with [`STOPPED_BY_USER`], so the history stays
    /// one the provider accepts: an assistant turn that asked for a tool must be followed by
    /// that tool's result, or the next request fails with a 400.
    pub fn stop(&mut self) {
        // A picture the stopped turn was going to show is no longer owed.
        self.shown_images.clear();
        // A script's own calls never enter the history, so the run is answered as one call.
        if let Some(run) = self.script.take() {
            run.job.stop();
            self.queue.clear();
            self.phase = None;
            if let Some(Turn::Script { failed, .. }) = self.turns.get_mut(run.turn) {
                *failed = true;
            }
            self.abandon_tool(run.outer, run.turn);
            return;
        }
        if let Some(spawn) = self.spawn_request.take() {
            self.abandon_tool(spawn.call, spawn.turn);
        }
        let Some(phase) = self.phase.take() else {
            return;
        };
        // Calls the model queued but that never ran are dropped with the run.
        self.queue.clear();
        match phase {
            Phase::Streaming { turn, .. } => {
                if let Some(Turn::Assistant { stopped, .. }) = self.turns.get_mut(turn) {
                    *stopped = true;
                }
            }
            Phase::Running { call, turn, .. }
            | Phase::Allowing { call, turn, .. }
            | Phase::Extending { call, turn, .. }
            | Phase::Connect { call, turn, .. }
            | Phase::Remote { call, turn, .. } => self.abandon_tool(call, turn),
            Phase::Approving { call, .. } | Phase::Asking { call, .. } | Phase::Sudo { call, .. } => {
                let turn = self.turn_for(&call);
                self.abandon_tool(call, turn);
            }
            Phase::Idle => {}
        }
    }

    /// The tool turn whose command is still running, if any.
    pub fn running_command(&self) -> Option<usize> {
        match &self.phase {
            Some(Phase::Running {
                turn,
                waiting: Waiting::Command { .. },
                ..
            }) => Some(*turn),
            _ => None,
        }
    }

    /// Stop the command the run is waiting on, so it does not keep going on the host.
    ///
    /// Its answer then reads as a failure, and the model is told the command was stopped
    /// instead of waiting for output that will never come.
    pub fn stop_command(&mut self, session: Session<'_, '_>) {
        let Some(Phase::Running {
            waiting: Waiting::Command { key, host, .. },
            ..
        }) = &self.phase
        else {
            return;
        };
        let (key, host) = (key.clone(), host.clone());
        self.with_host(host, session, |_, session| {
            if let Some(access) = session.as_mut() {
                access.cancel(&key);
            }
        });
    }

    /// Answer a tool call that will never run, so the model sees why it stopped.
    fn abandon_tool(&mut self, call: ApiToolCall, turn: usize) {
        if let Some(Turn::Tool { output, .. }) = self.turns.get_mut(turn) {
            *output = STOPPED_BY_USER.to_owned();
        }
        self.history.push(ApiMessage::Tool {
            tool_call_id: call.id,
            content: STOPPED_BY_USER.to_owned(),
        });
    }

    /// Take the "model owes an answer" flag.
    pub fn take_needs_reply(&mut self) -> bool {
        std::mem::take(&mut self.needs_reply)
    }

    /// Whether a run was cut off before the model answered — the reference's
    /// "the previous run was interrupted" state.
    pub fn can_resume(&self) -> bool {
        !self.blocked()
            && matches!(
                self.history.last(),
                Some(ApiMessage::User { .. } | ApiMessage::Tool { .. })
            )
    }

    /// Make the history one every provider accepts.
    ///
    /// An assistant turn may only name tool calls that are answered: a call left hanging —
    /// the user stopped the run, the session died, the model asked and nothing ran — makes
    /// the whole conversation a 400 until it is gone. The assistant's text stays, so
    /// nothing the model already said is lost; a turn with nothing left in it goes.
    pub fn repair_history(&mut self) {
        let answered: std::collections::HashSet<String> = self
            .history
            .iter()
            .filter_map(|message| match message {
                ApiMessage::Tool { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            })
            .collect();
        self.history.retain_mut(|message| match message {
            ApiMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                tool_calls.retain(|call| answered.contains(&call.id));
                !content.trim().is_empty() || !tool_calls.is_empty()
            }
            _ => true,
        });
    }

    /// The transcript as plain text, for summarising.
    pub fn transcript_text(&self) -> String {
        let mut out = String::new();
        for turn in &self.turns {
            match turn {
                Turn::User { text, .. } => out.push_str(&format!("User: {text}\n")),
                Turn::Assistant { text, .. } => out.push_str(&format!("Assistant: {text}\n")),
                Turn::Tool { label, output, .. } => {
                    let body: String = output.chars().take(2_000).collect();
                    out.push_str(&format!("Tool {label}: {body}\n"));
                }
                Turn::Script {
                    code,
                    calls,
                    result,
                    ..
                } => {
                    out.push_str(&format!("Script ({} calls):\n{code}\n", calls.len()));
                    if let Some(result) = result {
                        let body: String = result.chars().take(2_000).collect();
                        out.push_str(&format!("Result: {body}\n"));
                    }
                }
                Turn::Denied(reason) => out.push_str(&format!("Denied: {reason}\n")),
                Turn::Question { text, answer } => out.push_str(&format!(
                    "Question: {text}\nAnswer: {}\n",
                    answer.as_deref().unwrap_or("(no answer)")
                )),
                Turn::Error(text) => out.push_str(&format!("Error: {text}\n")),
                Turn::Notice(text) => out.push_str(&format!("Note: {text}\n")),
            }
        }
        out
    }

    /// Rough size of the provider history in characters, for the context budget.
    ///
    /// Counting it means walking the whole conversation, and the budget is checked on every
    /// frame, so the total is kept with the history length and last-message size it was
    /// counted at: a run appends messages or grows the last one, and nothing edits an
    /// earlier message in place.
    pub fn history_chars(&mut self) -> usize {
        let length = self.history.len();
        let last = self.history.last().map_or(0, message_chars);
        if let Some((counted_length, counted_last, chars)) = self.history_count
            && counted_length == length
            && counted_last == last
        {
            return chars;
        }
        let chars = self.history.iter().map(message_chars).sum();
        self.history_count = Some((length, last, chars));
        chars
    }

    /// Where the conversation splits when it is compacted: the message the tail starts at.
    ///
    /// The tail begins with a user message, so no tool result is left without the call it
    /// answers, and the last `keep` messages are never folded.
    pub fn compact_split(&self, keep: usize) -> Option<usize> {
        let last = self.history.len().checked_sub(keep)?;
        (1..=last)
            .rev()
            .find(|index| matches!(self.history[*index], ApiMessage::User { .. }))
    }

    /// The part of the conversation folding takes out, as text for the summariser.
    pub fn folded_text(&self, cut: usize) -> String {
        let mut out = String::new();
        for message in &self.history[..cut] {
            match message {
                ApiMessage::User { content, .. } => out.push_str(&format!("User: {content}\n")),
                ApiMessage::Assistant {
                    content,
                    tool_calls,
                    ..
                } => {
                    out.push_str(&format!("Assistant: {content}\n"));
                    for call in tool_calls {
                        out.push_str(&format!(
                            "Assistant called {}: {}\n",
                            call.name, call.arguments
                        ));
                    }
                }
                ApiMessage::Tool { content, .. } => {
                    out.push_str(&format!("Tool result: {content}\n"));
                }
            }
        }
        out
    }

    /// Fold the conversation up to `cut` into `summary`, returning the turns dropped.
    ///
    /// The transcript loses the turns folded with it, so the two stay one user turn per
    /// user message; the summary itself travels in the system prompt.
    pub fn fold(&mut self, cut: usize, summary: &str) -> usize {
        let users = self.history[..cut]
            .iter()
            .filter(|message| matches!(message, ApiMessage::User { .. }))
            .count();
        let turns = self
            .turns
            .iter()
            .enumerate()
            .filter(|(_, turn)| matches!(turn, Turn::User { .. }))
            .nth(users)
            .map_or(self.turns.len(), |(index, _)| index);
        self.history.drain(..cut);
        self.turns.drain(..turns);
        self.compacted = Some(summary.to_owned());
        turns
    }
}

/// Characters one provider message carries, tool calls included.
fn message_chars(message: &ApiMessage) -> usize {
    match message {
        ApiMessage::User { content, images } => {
            content.len() + images.iter().map(|image| image.data.len() / 4).sum::<usize>()
        }
        ApiMessage::Assistant {
            content,
            tool_calls,
            reasoning_content,
        } => {
            content.len()
                + reasoning_content.as_ref().map_or(0, String::len)
                + tool_calls
                    .iter()
                    .map(|call| call.name.len() + call.arguments.len())
                    .sum::<usize>()
        }
        ApiMessage::Tool { content, .. } => content.len(),
    }
}

/// The questions an `ask_user` call asks.
///
/// The documented shape is `{ "questions": [{ "question", "options" }] }`, but a script
/// makes the bare forms the natural ones: `tools.ask_user([{ question, options }])`,
/// `tools.ask_user({ question })` or `tools.ask_user("…")`. Every shape a model writes is
/// read here, so a real question never degrades to a card with no text and no options.
fn asked_questions(arguments: &Value) -> Vec<AgentQuestion> {
    let listed = &arguments["questions"];
    let items: Vec<&Value> = match arguments {
        Value::Array(items) => items.iter().collect(),
        Value::String(_) => vec![arguments],
        Value::Object(map) if map.contains_key("question") => vec![arguments],
        Value::Object(_) => match listed {
            Value::Array(items) => items.iter().collect(),
            Value::String(_) => vec![listed],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let questions: Vec<AgentQuestion> = items
        .into_iter()
        .filter_map(|item| {
            if let Some(text) = item.as_str().and_then(question_text) {
                return Some(AgentQuestion {
                    text,
                    options: Vec::new(),
                });
            }
            let text = item["question"].as_str().and_then(question_text)?;
            let options = item["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(question_text)
                .collect();
            Some(AgentQuestion { text, options })
        })
        .collect();
    if !questions.is_empty() {
        return questions;
    }
    let text = arguments["text"]
        .as_str()
        .and_then(question_text)
        .or_else(|| arguments.as_str().and_then(question_text))
        .unwrap_or_else(|| QUESTION_FALLBACK.to_owned());
    vec![AgentQuestion {
        text,
        options: Vec::new(),
    }]
}

/// A trimmed, non-empty string.
fn question_text(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The label shown for a tool call.
pub fn call_label(call: &ApiToolCall) -> String {
    let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
    match call.name.as_str() {
        TOOL_RUN_COMMAND => format!("$ {}", arguments["command"].as_str().unwrap_or_default()),
        TOOL_CHANGE_DIR => format!(
            "cd {}",
            arguments["path"].as_str().unwrap_or_default().trim()
        ),
        TOOL_RUN_COMMAND_BACKGROUND => format!(
            "$ {} (background)",
            arguments["command"].as_str().unwrap_or_default()
        ),
        TOOL_RUN_SUDO => format!(
            "$ sudo {}",
            arguments["command"].as_str().unwrap_or_default()
        ),
        TOOL_SPAWN_SUBAGENT => format!(
            "spawn {}",
            arguments["title"]
                .as_str()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| subagent_title(
                    arguments["prompt"].as_str().unwrap_or_default()
                ))
        ),
        TOOL_WAIT_SUBAGENT => {
            format!("wait for {}", arguments["id"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_READ | TOOL_LOCAL_READ | TOOL_DEV_VIEW => {
            format!("read {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_LIST | TOOL_LOCAL_LIST | TOOL_DEV_LIST => {
            format!("list {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_WRITE | TOOL_LOCAL_WRITE | TOOL_DEV_CREATE => {
            format!("write {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_EDIT | TOOL_LOCAL_EDIT | TOOL_DEV_EDIT => {
            format!("edit {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_PATCH | TOOL_LOCAL_PATCH | TOOL_LOCAL_MULTI_EDIT => {
            format!("patch {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_REMOTE_STAT => format!("stat {}", arguments["path"].as_str().unwrap_or_default()),
        TOOL_REMOTE_MOVE => format!(
            "move {} → {}",
            arguments["path"].as_str().unwrap_or_default(),
            arguments["destination"].as_str().unwrap_or_default()
        ),
        TOOL_REMOTE_ZIP_LIST => format!(
            "zip list {}",
            arguments["path"].as_str().unwrap_or_default()
        ),
        TOOL_REMOTE_ZIP_READ => format!(
            "zip read {}",
            arguments["entry"].as_str().unwrap_or_default()
        ),
        TOOL_REMOTE_ZIP_EXTRACT => format!(
            "zip extract {}",
            arguments["path"].as_str().unwrap_or_default()
        ),
        TOOL_GIT_STATUS => "git status".to_owned(),
        TOOL_GIT_DIFF => "git diff".to_owned(),
        TOOL_GIT_LOG => "git log".to_owned(),
        TOOL_GIT_BRANCHES => "git branches".to_owned(),
        TOOL_GIT_COMMIT => format!(
            "git commit {}",
            arguments["message"].as_str().unwrap_or_default()
        ),
        TOOL_GIT_CHECKOUT => format!(
            "git checkout {}",
            arguments["branch"].as_str().unwrap_or_default()
        ),
        TOOL_REMOTE_DELETE | TOOL_LOCAL_DELETE => {
            format!("delete {}", arguments["path"].as_str().unwrap_or_default())
        }
        TOOL_DEV_GREP | TOOL_LOCAL_GREP => {
            format!("grep {}", arguments["pattern"].as_str().unwrap_or_default())
        }
        TOOL_DEV_FIND => format!(
            "find {}",
            arguments["namePattern"].as_str().unwrap_or_default()
        ),
        TOOL_DEV_DIFF => format!("diff {}", arguments["path"].as_str().unwrap_or_default()),
        TOOL_SCRIPT_RESULT => format!(
            "script {}",
            arguments["id"].as_str().unwrap_or_default()
        ),
        TOOL_DEV_IMAGE => format!(
            "image {}",
            arguments["path"].as_str().unwrap_or_default()
        ),
        TOOL_LOCAL_RUN => format!(
            "local $ {}",
            arguments["command"].as_str().unwrap_or_default()
        ),
        TOOL_LOCAL_MOVE => format!(
            "move {} → {}",
            arguments["path"].as_str().unwrap_or_default(),
            arguments["destination"].as_str().unwrap_or_default()
        ),
        TOOL_LOCAL_REPLACE => format!(
            "replace {}",
            arguments["pattern"].as_str().unwrap_or_default()
        ),
        TOOL_WEB_FETCH => format!("fetch {}", arguments["url"].as_str().unwrap_or_default()),
        TOOL_WEB_SEARCH => format!("search {}", arguments["query"].as_str().unwrap_or_default()),
        TOOL_RAG_SEARCH => format!("rag {}", arguments["query"].as_str().unwrap_or_default()),
        TOOL_CURRENT_TIME => "time".to_owned(),
        TOOL_TERMINAL_READ => "terminal read".to_owned(),
        TOOL_TERMINAL_WRITE => format!(
            "terminal type {}",
            arguments["text"].as_str().unwrap_or_default()
        ),
        TOOL_TERMINAL_KEYS => format!(
            "terminal keys {}",
            arguments["keys"].as_str().unwrap_or_default()
        ),
        TOOL_TERMINAL_WAIT => "terminal wait".to_owned(),
        TOOL_SYSTEM_OVERVIEW => "system overview".to_owned(),
        TOOL_SYSTEM_PROCESSES => "processes".to_owned(),
        TOOL_SYSTEM_DISK => "disk usage".to_owned(),
        TOOL_SYSTEM_MEMORY => "memory report".to_owned(),
        TOOL_ASK_USER => "question".to_owned(),
        TOOL_LOAD_SCOPE => format!("load {}", arguments["scope"].as_str().unwrap_or_default()),
        TOOL_UNLOAD_SCOPE => format!("unload {}", arguments["scope"].as_str().unwrap_or_default()),
        other => other.to_owned(),
    }
}

/// Quote a path for `sh`.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Join `name` onto `cwd` unless it is already absolute.
pub fn relative_remote(cwd: &str, name: &str) -> String {
    let name = name.trim();
    if name.starts_with('/') {
        name.to_owned()
    } else if name.is_empty() {
        cwd.to_owned()
    } else {
        tools::join_path(cwd, name)
    }
}

/// The tool result a background script's run left in its own history.
fn tool_result(agent: &Agent, call_id: &str) -> Option<(String, bool)> {
    agent.history.iter().find_map(|message| match message {
        ApiMessage::Tool {
            tool_call_id,
            content,
        } if tool_call_id == call_id => {
            Some((content.clone(), content.starts_with("The script failed")))
        }
        _ => None,
    })
}

/// Whether a tool changes a file, which makes the map stale for that path.
fn is_writing_tool(name: &str) -> bool {
    matches!(
        name,
        TOOL_DEV_EDIT
            | TOOL_DEV_CREATE
            | TOOL_REMOTE_WRITE
            | TOOL_REMOTE_EDIT
            | TOOL_REMOTE_DELETE
            | TOOL_REMOTE_MOVE
            | TOOL_REMOTE_PATCH
            | TOOL_REMOTE_ZIP_EXTRACT
            | TOOL_GIT_COMMIT
            | TOOL_GIT_CHECKOUT
            | TOOL_LOCAL_WRITE
            | TOOL_LOCAL_EDIT
            | TOOL_LOCAL_DELETE
    )
}

/// Wrap a command so its exit status and working directory come back too.
pub fn wrap_command(cwd: &str, command: &str) -> String {
    let status = "__WASSH_STATUS";
    let pwd = "__WASSH_PWD";
    // A host the conversation has not moved in yet has no directory to go to.
    let enter = (!cwd.is_empty()).then(|| format!("cd {} >/dev/null 2>&1 || true", shell_quote(cwd)));
    [
        enter.unwrap_or_default(),
        command.to_owned(),
        "__wassh_status=$?".to_owned(),
        format!(r#"printf '\n{status}:%s__\n' "$__wassh_status""#),
        format!(r#"printf '{pwd}:%s__\n' "$(pwd 2>/dev/null)""#),
    ]
    .join("\n")
}

/// Whether a command's output says it needed root.
fn needs_sudo_help(output: &str) -> bool {
    let lower = output.to_lowercase();
    SUDO_DEMAND_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Whether a command line runs `sudo` at all.
///
/// Splits on whitespace and shell separators, then compares the program's name, so
/// `sudo` and `/usr/bin/sudo` count while `pseudo` does not.
fn command_needs_sudo(command: &str) -> bool {
    command
        .split(|c: char| c.is_whitespace() || c == ';' || c == '|' || c == '&')
        .any(|word| word.rsplit('/').next() == Some("sudo"))
}

/// Wrap a command that needs `sudo`, reading the password from stdin.
///
/// Port of the reference's sudo preamble: the password is written to the channel
/// and read with `IFS= read -r`, so it never reaches the command line. The command
/// itself is run as root unless it already calls `sudo`.
pub fn wrap_command_with_sudo(cwd: &str, command: &str) -> String {
    let status = "__WASSH_STATUS";
    let pwd = "__WASSH_PWD";
    let line = if command_needs_sudo(command) {
        command.to_owned()
    } else {
        format!("sudo -n {command}")
    };
    [
        format!("cd {} >/dev/null 2>&1 || true", shell_quote(cwd)),
        "IFS= read -r __wassh_sudo_pw || true".to_owned(),
        "if [ -n \"${__wassh_sudo_pw}\" ]; then".to_owned(),
        "  printf '%s\\n' \"${__wassh_sudo_pw}\" | sudo -S -p '' -v >/dev/null 2>&1".to_owned(),
        "  __wassh_sudo_auth=$?".to_owned(),
        "else".to_owned(),
        "  sudo -n -v >/dev/null 2>&1".to_owned(),
        "  __wassh_sudo_auth=$?".to_owned(),
        "fi".to_owned(),
        "unset __wassh_sudo_pw".to_owned(),
        "if [ \"${__wassh_sudo_auth}\" -ne 0 ]; then".to_owned(),
        format!("  printf '%s\\n' '{SUDO_AUTH_FAILED_MARKER}'"),
        "  __wassh_status=1".to_owned(),
        "else".to_owned(),
        format!("  {line}"),
        "  __wassh_status=$?".to_owned(),
        "fi".to_owned(),
        format!(r#"printf '\n{status}:%s__\n' "$__wassh_status""#),
        format!(r#"printf '{pwd}:%s__\n' "$(pwd 2>/dev/null)""#),
    ]
    .join("\n")
}

/// Undo [`wrap_command`]: the visible output, exit status and directory.
pub fn unwrap_output(raw: &str) -> (String, Option<i32>, Option<String>) {
    let mut exit_code = None;
    let mut directory = None;
    let mut visible = String::with_capacity(raw.len());
    for line in raw.split('\n') {
        let trimmed = line.trim_end_matches('\r');
        if let Some(rest) = trimmed.strip_prefix("__WASSH_STATUS:") {
            exit_code = rest.trim_end_matches("__").trim().parse().ok();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("__WASSH_PWD:") {
            directory = Some(rest.trim_end_matches("__").to_owned());
            continue;
        }
        visible.push_str(trimmed);
        visible.push('\n');
    }
    while visible.ends_with('\n') {
        visible.pop();
    }
    (visible, exit_code, directory)
}

/// Keep only the tail a command can still have its markers in.
fn tail_of(raw: &str) -> String {
    let keep = MAX_OUTPUT_CHARS + MARKER_TAIL_CHARS;
    let count = raw.chars().count();
    if count > keep {
        raw.chars().skip(count - keep).collect()
    } else {
        raw.to_owned()
    }
}

/// Cut the visible output to what the model may read.
fn clamp_output(text: String) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text;
    }
    text.chars().take(MAX_OUTPUT_CHARS).collect()
}

/// Apply an edit and describe it.
fn replace_and_report(text: &str, old: &str, new: &str) -> Result<(String, String), String> {
    let updated = tools::replace_once(text, old, new)?;
    Ok((updated.clone(), tools::diff_texts(text, &updated, 3)))
}

/// What the model is told about one command.
pub fn command_result_text(output: &str, exit_code: Option<i32>, truncated: bool) -> String {
    let mut text = String::new();
    if output.is_empty() {
        text.push_str("(no output)\n");
    } else {
        text.push_str(output);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    text.push_str(&format!(
        "[exit code: {}]",
        exit_code.map_or_else(|| "unknown".to_owned(), |code| code.to_string())
    ));
    if truncated {
        text.push_str("\n[output truncated]");
    }
    text
}

/// The transcript as stored JSON.
pub fn turns_to_json(turns: &[Turn]) -> Vec<Value> {
    turns
        .iter()
        .map(|turn| match turn {
            Turn::User { text, meta } => serde_json::json!({
                "role": "user",
                "text": text,
                "attachments": meta.attachments,
                "terminal": meta.terminal
            }),
            Turn::Assistant { text, stopped } => {
                serde_json::json!({ "role": "assistant", "text": text, "stopped": stopped })
            }
            Turn::Tool {
                name,
                label,
                output,
                failed,
            } => serde_json::json!({
                "role": "tool",
                "name": name,
                "label": label,
                "output": output,
                "failed": failed
            }),
            Turn::Denied(reason) => serde_json::json!({ "role": "denied", "text": reason }),
            Turn::Script {
                code,
                calls,
                result,
                failed,
                console,
            } => serde_json::json!({
                "role": "script",
                "code": code,
                "calls": calls
                    .iter()
                    .map(|row| serde_json::json!({
                        "name": row.name,
                        "label": row.label,
                        "output": row.output,
                        "failed": row.failed
                    }))
                    .collect::<Vec<_>>(),
                "result": result,
                "failed": failed,
                "console": console
            }),
            Turn::Question { text, answer } => {
                serde_json::json!({ "role": "question", "text": text, "answer": answer })
            }
            Turn::Error(text) => serde_json::json!({ "role": "error", "text": text }),
            Turn::Notice(text) => serde_json::json!({ "role": "notice", "text": text }),
        })
        .collect()
}

/// Read a stored transcript back.
pub fn turns_from_json(items: &[Value]) -> Vec<Turn> {
    items
        .iter()
        .filter_map(|item| {
            let text = || item["text"].as_str().unwrap_or_default().to_owned();
            match item["role"].as_str()? {
                "user" => Some(Turn::User {
                    text: text(),
                    meta: UserMeta {
                        attachments: item["attachments"].as_u64().unwrap_or(0) as usize,
                        terminal: item["terminal"].as_bool().unwrap_or(false),
                    },
                }),
                "assistant" => {
                    let text = text();
                    // A reply that only asked for tools was stored as an empty turn.
                    if text.trim().is_empty() {
                        return None;
                    }
                    Some(Turn::Assistant {
                        text,
                        stopped: item["stopped"].as_bool().unwrap_or(false),
                    })
                }
                "tool" => Some(Turn::Tool {
                    name: item["name"].as_str().unwrap_or_default().to_owned(),
                    label: item["label"].as_str().unwrap_or_default().to_owned(),
                    output: item["output"].as_str().unwrap_or_default().to_owned(),
                    failed: item["failed"].as_bool().unwrap_or(false),
                }),
                "denied" => Some(Turn::Denied(text())),
                "script" => Some(Turn::Script {
                    code: item["code"].as_str().unwrap_or_default().to_owned(),
                    calls: item["calls"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|row| ScriptRow {
                            name: row["name"].as_str().unwrap_or_default().to_owned(),
                            label: row["label"].as_str().unwrap_or_default().to_owned(),
                            output: row["output"].as_str().unwrap_or_default().to_owned(),
                            failed: row["failed"].as_bool().unwrap_or(false),
                        })
                        .collect(),
                    result: item["result"].as_str().map(str::to_owned),
                    failed: item["failed"].as_bool().unwrap_or(false),
                    console: item["console"].as_str().unwrap_or_default().to_owned(),
                }),
                "question" => Some(Turn::Question {
                    text: text(),
                    answer: item["answer"].as_str().map(str::to_owned),
                }),
                "error" => Some(Turn::Error(text())),
                "notice" => Some(Turn::Notice(text())),
                _ => None,
            }
        })
        .collect()
}

/// The sub-agents of one chat, as stored JSON.
pub fn subagents_to_json(subagents: &[Subagent]) -> Vec<Value> {
    subagents
        .iter()
        .map(|sub| {
            serde_json::json!({
                "id": sub.id,
                "title": sub.title,
                "prompt": sub.prompt,
                "end": sub.end.map(SubagentEnd::as_str),
                "turns": turns_to_json(&sub.agent.turns),
                "history": history_to_json(&sub.agent.history),
                "cwd": sub.agent.cwd,
                "compacted": sub.agent.compacted,
                "scopes": sub.agent.scopes.to_json()
            })
        })
        .collect()
}

/// Read stored sub-agents back.
///
/// One that was still working when the data was written is restored as stopped: its run
/// cannot be picked up again, but its transcript stays readable.
pub fn subagents_from_json(value: Option<&Value>) -> Vec<Subagent> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row["id"].as_str().filter(|id| !id.is_empty())?;
            let mut agent = Agent::new();
            agent.subagent = true;
            agent.keys = CommandKeys::subagent(id);
            agent.cwd = row["cwd"].as_str().unwrap_or("/").to_owned();
            agent.compacted = row["compacted"].as_str().map(str::to_owned);
            agent.scopes = Loaded::from_json(row.get("scopes"));
            if let Some(turns) = row["turns"].as_array() {
                agent.turns = turns_from_json(turns);
            }
            if let Some(history) = row["history"].as_array() {
                agent.history = history_from_json(history);
            }
            let end = row["end"]
                .as_str()
                .and_then(SubagentEnd::parse)
                .or(Some(SubagentEnd::Stopped));
            Some(Subagent {
                id: id.to_owned(),
                title: row["title"].as_str().unwrap_or_default().to_owned(),
                prompt: row["prompt"].as_str().unwrap_or_default().to_owned(),
                agent,
                end,
                waiting_announced: false,
            })
        })
        .collect()
}

/// The provider history as stored JSON.
pub fn history_to_json(messages: &[ApiMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| match message {
            ApiMessage::User { content, images } => {
                let mut stored = serde_json::json!({ "role": "user", "content": content });
                if !images.is_empty() {
                    stored["images"] = Value::Array(
                        images
                            .iter()
                            .map(|image| {
                                serde_json::json!({
                                    "mediaType": image.media_type,
                                    "data": base64::engine::general_purpose::STANDARD.encode(&image.data)
                                })
                            })
                            .collect(),
                    );
                }
                stored
            }
            ApiMessage::Assistant {
                content,
                reasoning_content,
                tool_calls,
            } => {
                let mut stored = serde_json::json!({
                    "role": "assistant",
                    "content": content,
                    "toolCalls": tool_calls
                    .iter()
                    .map(|call| serde_json::json!({
                        "id": call.id,
                        "name": call.name,
                        "arguments": call.arguments
                    }))
                    .collect::<Vec<_>>()
                });
                if let Some(reasoning_content) = reasoning_content {
                    stored["reasoningContent"] = serde_json::json!(reasoning_content);
                }
                stored
            }
            ApiMessage::Tool {
                tool_call_id,
                content,
            } => serde_json::json!({
                "role": "tool",
                "toolCallId": tool_call_id,
                "content": content
            }),
        })
        .collect()
}

/// Read a stored provider history back.
pub fn history_from_json(items: &[Value]) -> Vec<ApiMessage> {
    items
        .iter()
        .filter_map(|item| match item["role"].as_str()? {
            "user" => Some(ApiMessage::User {
                content: item["content"].as_str().unwrap_or_default().to_owned(),
                images: item["images"]
                    .as_array()
                    .map(|images| {
                        images
                            .iter()
                            .filter_map(|image| {
                                Some(ImagePart {
                                    media_type: image["mediaType"].as_str()?.to_owned(),
                                    data: base64::engine::general_purpose::STANDARD
                                        .decode(image["data"].as_str()?)
                                        .ok()?,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
            "assistant" => Some(ApiMessage::Assistant {
                content: item["content"].as_str().unwrap_or_default().to_owned(),
                reasoning_content: item["reasoningContent"].as_str().map(str::to_owned),
                tool_calls: item["toolCalls"]
                    .as_array()
                    .map(|calls| {
                        calls
                            .iter()
                            .map(|call| ApiToolCall {
                                id: call["id"].as_str().unwrap_or_default().to_owned(),
                                name: call["name"].as_str().unwrap_or_default().to_owned(),
                                arguments: call["arguments"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
            "tool" => Some(ApiMessage::Tool {
                tool_call_id: item["toolCallId"].as_str().unwrap_or_default().to_owned(),
                content: item["content"].as_str().unwrap_or_default().to_owned(),
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Polls a running script may take before the test calls it stuck.
    const SCRIPT_POLL_STEPS: usize = 10_000;

    #[test]
    fn a_wrapped_command_reports_status_and_directory() {
        let wrapped = wrap_command("/srv/app", "ls -la");
        assert!(wrapped.starts_with("cd '/srv/app' >/dev/null 2>&1 || true\nls -la\n"));
        assert!(wrapped.contains("__WASSH_STATUS:%s__"));
        assert!(wrapped.contains("__WASSH_PWD:%s__"));
    }

    #[test]
    fn reasoning_content_survives_history_storage_and_older_chats_still_load() {
        let history = vec![ApiMessage::Assistant {
            content: String::new(),
            reasoning_content: Some("thinking".to_owned()),
            tool_calls: Vec::new(),
        }];
        let stored = history_to_json(&history);
        assert_eq!(stored[0]["reasoningContent"], "thinking");
        assert_eq!(history_from_json(&stored), history);

        let older = history_from_json(&[serde_json::json!({
            "role": "assistant",
            "content": "legacy",
            "toolCalls": []
        })]);
        assert!(matches!(
            older.first(),
            Some(ApiMessage::Assistant {
                reasoning_content: None,
                ..
            })
        ));
    }

    #[test]
    fn a_message_injected_mid_run_joins_the_conversation_after_it() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        assert!(agent.push_user("first", UserMeta::default()));
        // A reply is streaming in.
        agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        let (sender, events) = std::sync::mpsc::channel();
        agent.phase = Some(Phase::Streaming { turn: 1, events });

        // Enter during the run adds to the transcript at once …
        assert!(agent.inject_user("and check /etc too", UserMeta::default()));
        assert!(matches!(
            agent.turns.last(),
            Some(Turn::User { text, .. }) if text == "and check /etc too"
        ));
        // … but the provider history waits, so the reply under way comes first.
        assert_eq!(agent.history.len(), 1, "history: {:?}", agent.history);
        assert!(!agent.flush_injected(), "the run is still going");

        // The run ends: the injected message is folded in behind it and owes a reply.
        sender
            .send(StreamEvent::Done(Completion {
                text: "done".to_owned(),
                ..Completion::default()
            }))
            .expect("the reply lands");
        agent.poll(&mut session);
        assert!(agent.phase.is_none());
        assert!(agent.flush_injected());
        assert!(
            matches!(
                (agent.history.get(1), agent.history.get(2)),
                (
                    Some(ApiMessage::Assistant { .. }),
                    Some(ApiMessage::User { content, .. })
                ) if content == "and check /etc too"
            ),
            "history: {:?}",
            agent.history
        );
        assert!(agent.needs_reply, "the model owes an answer");
        assert!(!agent.flush_injected(), "nothing is left to fold in");
    }

    #[test]
    fn a_message_typed_into_an_idle_conversation_is_not_an_injection() {
        let mut agent = Agent::new();
        assert!(!agent.inject_user("hello", UserMeta::default()));
        assert!(agent.turns.is_empty(), "nothing was taken");
    }

    #[test]
    fn unwrapping_keeps_the_output_and_reads_the_markers() {
        let raw = "total 4\ndrwxr-xr-x 2 root root\n__WASSH_STATUS:0__\n__WASSH_PWD:/srv/app__\n";
        let (output, code, directory) = unwrap_output(raw);
        assert_eq!(output, "total 4\ndrwxr-xr-x 2 root root");
        assert_eq!(code, Some(0));
        assert_eq!(directory.as_deref(), Some("/srv/app"));
    }

    #[test]
    fn the_tool_result_carries_the_exit_code() {
        assert_eq!(
            command_result_text("ok", Some(0), false),
            "ok\n[exit code: 0]"
        );
        assert_eq!(
            command_result_text("", Some(2), false),
            "(no output)\n[exit code: 2]"
        );
        assert!(command_result_text("big", None, true).contains("[output truncated]"));
    }

    #[test]
    fn shell_quoting_survives_a_quote_in_the_path() {
        assert_eq!(shell_quote("/srv/it's here"), r"'/srv/it'\''s here'");
    }

    #[test]
    fn remote_paths_resolve_against_the_working_directory() {
        assert_eq!(relative_remote("/srv/app", "/etc/passwd"), "/etc/passwd");
        assert_eq!(
            relative_remote("/srv/app", "notes.txt"),
            "/srv/app/notes.txt"
        );
        assert_eq!(relative_remote("/srv/app", ""), "/srv/app");
        assert_eq!(relative_remote("/", "etc"), "/etc");
    }

    #[test]
    fn tool_labels_read_like_the_reference() {
        let call = |name: &str, arguments: &str| ApiToolCall {
            id: "c1".to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        };
        assert_eq!(
            call_label(&call(TOOL_RUN_COMMAND, r#"{"command":"uptime"}"#)),
            "$ uptime"
        );
        assert_eq!(
            call_label(&call(TOOL_REMOTE_READ, r#"{"path":"/etc/hosts"}"#)),
            "read /etc/hosts"
        );
        assert_eq!(call_label(&call(TOOL_CURRENT_TIME, "{}")), "time");
        assert_eq!(
            call_label(&call(TOOL_WEB_SEARCH, r#"{"query":"rust"}"#)),
            "search rust"
        );
    }

    #[test]
    fn a_reply_whose_tools_never_came_back_is_made_answerable_before_resuming() {
        let call = |id: &str| ApiToolCall {
            id: id.to_owned(),
            name: TOOL_RUN_COMMAND.to_owned(),
            arguments: r#"{"command":"uptime"}"#.to_owned(),
        };
        let mut agent = Agent::new();
        agent.history = vec![
            ApiMessage::User {
                content: "go".to_owned(),
                images: Vec::new(),
            },
            ApiMessage::Assistant {
                content: String::new(),
                reasoning_content: None,
                tool_calls: vec![call("a"), call("b")],
            },
            ApiMessage::Tool {
                tool_call_id: "a".to_owned(),
                content: "up 3 days".to_owned(),
            },
        ];
        assert!(agent.can_resume(), "the model owes an answer to this run");
        agent.repair_history();
        // The answered call and its result stay; the one that never came back goes.
        assert_eq!(agent.history.len(), 3, "{:?}", agent.history);
        match &agent.history[1] {
            ApiMessage::Assistant { tool_calls, .. } => {
                assert_eq!(tool_calls.len(), 1, "{tool_calls:?}");
                assert_eq!(tool_calls[0].id, "a");
            }
            other => panic!("expected the assistant turn, got {other:?}"),
        }

        // An assistant turn with nothing but the unanswered call disappears entirely.
        agent.history.push(ApiMessage::Assistant {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![call("c")],
        });
        agent.repair_history();
        assert_eq!(agent.history.len(), 3, "{:?}", agent.history);

        // Text the model already wrote survives.
        agent.history.push(ApiMessage::Assistant {
            content: "Let me check the logs.".to_owned(),
            reasoning_content: None,
            tool_calls: vec![call("d")],
        });
        agent.repair_history();
        assert_eq!(agent.history.len(), 4, "{:?}", agent.history);
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Assistant { tool_calls, .. }) if tool_calls.is_empty()
        ));
    }

    #[test]
    fn stopping_a_tool_run_answers_the_call_so_the_next_request_is_valid() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        assert!(agent.push_user("deploy", UserMeta::default()));
        // The model asked for a tool; it is running when the user stops the run.
        let running = ApiToolCall {
            id: "c1".to_owned(),
            name: TOOL_TERMINAL_WAIT.to_owned(),
            arguments: r#"{"timeoutMs":60000}"#.to_owned(),
        };
        agent.turns.push(Turn::Assistant {
            text: "Waiting.".to_owned(),
            stopped: false,
        });
        agent.history.push(ApiMessage::Assistant {
            content: "Waiting.".to_owned(),
            reasoning_content: None,
            tool_calls: vec![running.clone()],
        });
        agent.turns.push(Turn::Tool {
            name: running.name.clone(),
            label: call_label(&running),
            output: String::new(),
            failed: false,
        });
        agent.phase = Some(Phase::Running {
            call: running,
            // The user, the reply and the tool call are the transcript so far.
            turn: 2,
            waiting: Waiting::Terminal {
                before: String::new(),
                until_ms: u64::MAX,
            },
        });

        agent.stop();
        assert!(agent.phase.is_none(), "the run is over");
        assert!(agent.can_resume(), "the model owes an answer");
        match agent.history.last() {
            Some(ApiMessage::Tool {
                tool_call_id,
                content,
            }) => {
                assert_eq!(tool_call_id, "c1");
                assert_eq!(content, STOPPED_BY_USER);
            }
            other => panic!("the pending call was left unanswered: {other:?}"),
        }
        match agent.turns.last() {
            Some(Turn::Tool { output, .. }) => assert_eq!(output, STOPPED_BY_USER),
            other => panic!("the transcript does not show the stop: {other:?}"),
        }
        // Stopping leaves nothing that would start another request on its own.
        assert!(!agent.take_needs_reply());
        let mut session = session.take();
        agent.poll(&mut session);
        let _ = &mut session;
    }

    #[test]
    fn rewinding_takes_the_conversation_back_to_the_clicked_message() {
        let mut agent = Agent::new();
        assert!(agent.push_user("first", UserMeta::default()));
        agent.turns.push(Turn::Assistant {
            text: "one".to_owned(),
            stopped: false,
        });
        assert!(agent.push_user("second", UserMeta::default()));
        agent.turns.push(Turn::Assistant {
            text: "two".to_owned(),
            stopped: false,
        });

        assert_eq!(agent.rewind_to(2).as_deref(), Some("second"));
        assert_eq!(
            agent
                .turns
                .iter()
                .filter(|turn| matches!(turn, Turn::User { .. }))
                .count(),
            1,
            "the later exchange is gone: {:?}",
            agent.turns
        );
        assert_eq!(
            agent
                .history
                .iter()
                .filter(|message| matches!(message, ApiMessage::User { .. }))
                .count(),
            1,
            "the provider history is cut to the same point: {:?}",
            agent.history
        );
        assert!(
            agent.rewind_to(0).is_some(),
            "the first message can be rewound to too"
        );
        assert!(agent.turns.is_empty() && agent.history.is_empty());
        assert_eq!(
            agent.rewind_to(0),
            None,
            "there is nothing left to rewind to"
        );
    }

    #[test]
    fn the_transcript_round_trips_through_json() {
        let turns = vec![
            Turn::User {
                text: "hello".to_owned(),
                meta: UserMeta {
                    attachments: 2,
                    terminal: true,
                },
            },
            Turn::Assistant {
                text: "hi".to_owned(),
                stopped: false,
            },
            Turn::Tool {
                name: TOOL_RUN_COMMAND.to_owned(),
                label: "$ uptime".to_owned(),
                output: "up 3 days".to_owned(),
                failed: false,
            },
            Turn::Question {
                text: "which file?".to_owned(),
                answer: Some("a.txt".to_owned()),
            },
            Turn::Notice("summary saved".to_owned()),
        ];
        let json = turns_to_json(&turns);
        assert_eq!(turns_from_json(&json), turns);
    }

    #[test]
    fn edits_are_applied_and_described() {
        let (updated, diff) = replace_and_report("a\nb\n", "b", "B").unwrap();
        assert_eq!(updated, "a\nB\n");
        assert!(diff.contains("+   1 B"), "{diff}");
        assert!(replace_and_report("a\n", "zz", "y").is_err());
    }

    #[test]
    fn an_unknown_tool_is_refused_without_touching_the_session() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: "no_such_tool".to_owned(),
                arguments: "{}".to_owned(),
            },
            &mut session,
        );
        assert!(matches!(&agent.turns[0], Turn::Tool { failed: true, .. }));
        assert!(agent.needs_reply);
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content.contains("not available")
        ));
    }

    #[test]
    fn a_local_tool_answers_immediately() {
        let mut agent = Agent::new();
        agent.now_ms = 1_700_000_000_000;
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_CURRENT_TIME.to_owned(),
                arguments: "{}".to_owned(),
            },
            &mut session,
        );
        match &agent.turns[0] {
            Turn::Tool { output, failed, .. } => {
                assert!(!failed);
                assert!(output.contains("1700000000000"), "{output}");
            }
            other => panic!("expected a tool turn, got {other:?}"),
        }
        assert!(agent.needs_reply);
    }

    /// Answer every tool a running script asks for until the run is over.
    fn drive_script(agent: &mut Agent, fake: &mut FakeSession) {
        let mut session: Option<&mut dyn SessionAccess> = Some(fake);
        for _ in 0..SCRIPT_POLL_STEPS {
            if !agent.blocked() {
                return;
            }
            if !agent.poll(&mut session) {
                // The sandbox runs on its own thread: give it the processor rather than
                // spending the poll budget before it has started.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        panic!("the script never finished");
    }

    /// Poll until every background script has a result.
    fn drive_background(agent: &mut Agent, fake: &mut FakeSession) {
        let mut session: Option<&mut dyn SessionAccess> = Some(fake);
        for _ in 0..SCRIPT_POLL_STEPS {
            if agent.background.iter().all(|bg| bg.result.is_some()) {
                return;
            }
            if !agent.poll(&mut session) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        panic!("the background script never finished");
    }

    /// Start a script the way the model does.
    fn start_script(agent: &mut Agent, fake: &mut FakeSession, code: &str) {
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut *fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_SCRIPT.to_owned(),
                    arguments: serde_json::json!({ "code": code }).to_string(),
                },
                &mut session,
            );
        }
        drive_script(agent, fake);
    }

    #[test]
    fn a_script_runs_its_calls_and_answers_with_one_result() {
        let mut agent = Agent::new();
        agent.now_ms = 1_700_000_000_000;
        let mut fake = FakeSession::default();
        start_script(
            &mut agent,
            &mut fake,
            "const a = await tools.get_current_time({});\n\
             const b = await tools.get_current_time({});\n\
             return { stamps: [a.text, b.text] };",
        );
        let Some(Turn::Script { calls, result, .. }) = agent.turns.last() else {
            panic!("expected a script turn, got {:?}", agent.turns.last());
        };
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|row| !row.failed && !row.output.is_empty()));
        let result = result.clone().expect("the script returned");
        assert!(result.contains("1700000000000"), "{result}");
        // The script's own calls stay out of the provider history: the model reads one result.
        let answers = agent
            .history
            .iter()
            .filter(|message| matches!(message, ApiMessage::Tool { .. }))
            .count();
        assert_eq!(answers, 1, "{:?}", agent.history);
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_script_that_throws_answers_with_its_error() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        start_script(&mut agent, &mut fake, "throw new Error('nope');");
        let Some(Turn::Script { failed, result, .. }) = agent.turns.last() else {
            panic!("expected a script turn, got {:?}", agent.turns.last());
        };
        assert!(failed);
        let result = result.clone().expect("the failure is recorded");
        assert!(result.contains("nope"), "{result}");
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content.contains("nope")
        ));
    }

    #[test]
    fn a_script_call_that_hangs_times_out() {
        let mut agent = Agent::new();
        agent
            .scopes
            .load(scopes::find("host-files").expect("the file scope"));
        let mut fake = FakeSession::default();
        // Start the script without driving it, so the call can be left unanswered.
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_SCRIPT.to_owned(),
                    arguments: serde_json::json!({
                        "code": "await tools.dev_grep_search({ pattern: 'x', path: '/' });"
                    })
                    .to_string(),
                },
                &mut session,
            );
        }
        // Poll until the sandbox hands the call over; the session never answers it.
        for _ in 0..SCRIPT_POLL_STEPS {
            if matches!(agent.phase, Some(Phase::Running { waiting: Waiting::Command { .. }, .. }))
            {
                break;
            }
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.poll(&mut session);
            std::thread::yield_now();
        }
        let command_key = agent.keys.probe.clone();
        assert!(
            matches!(agent.phase, Some(Phase::Running { waiting: Waiting::Command { .. }, .. })),
            "the call is waiting on its command"
        );
        // Pretend the wait started before the limit.
        let run = agent.script.as_mut().expect("the script is running");
        run.since = Some(
            std::time::Instant::now()
                - std::time::Duration::from_millis(script::MAX_CALL_MS + 1),
        );
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            assert!(agent.poll(&mut session), "the timeout looked at the call");
        }
        assert!(
            fake.cancelled.iter().any(|key| *key == command_key),
            "the hung command is stopped: {:?}",
            fake.cancelled
        );
        assert!(
            agent
                .script
                .as_ref()
                .is_some_and(|run| run.inner.is_none()),
            "the call was answered to the script"
        );
        assert!(
            matches!(agent.phase, Some(Phase::Running { waiting: Waiting::Script, .. })),
            "the sandbox holds the turn again"
        );
        drive_script(&mut agent, &mut fake);
        let Some(Turn::Script { result, failed, .. }) = agent.turns.last() else {
            panic!("expected a script turn, got {:?}", agent.turns.last());
        };
        assert!(failed, "{result:?}");
        let result = result.clone().expect("the failure is recorded");
        assert!(result.contains("did not answer within"), "{result}");
    }

    #[test]
    fn a_script_asking_for_more_time_waits_for_the_user() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_SCRIPT.to_owned(),
                    arguments: r#"{"code":"return 1;","timeoutSeconds":120}"#.to_owned(),
                },
                &mut session,
            );
        }
        assert!(
            matches!(agent.phase, Some(Phase::Extending { seconds: 120, .. })),
            "the user is asked before the script runs"
        );
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.answer_extension(true, &mut session);
        }
        assert!(matches!(
            agent.phase,
            Some(Phase::Running {
                waiting: Waiting::Script,
                ..
            })
        ));
        assert!(agent.script.is_some(), "the granted script started");
        agent.stop();
    }

    #[test]
    fn a_refused_longer_script_timeout_is_reported() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_SCRIPT.to_owned(),
                    arguments: r#"{"code":"return 1;","timeoutSeconds":120}"#.to_owned(),
                },
                &mut session,
            );
        }
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.answer_extension(false, &mut session);
        }
        assert!(!agent.extending(), "the card is gone");
        match agent.history.last() {
            Some(ApiMessage::Tool { content, .. }) => {
                assert!(content.contains("refused"), "{content}");
                assert!(content.contains("120"), "{content}");
            }
            other => panic!("expected the refusal in the history, got {other:?}"),
        }
    }

    #[test]
    fn a_background_script_runs_beside_the_chat() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        start_background(&mut agent, &mut fake, "return 7;");
        assert!(agent.phase.is_none(), "the conversation is free");
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. })
                if content.contains("Started background script s1")
        ));
        drive_background(&mut agent, &mut fake);
        let Some((text, failed)) = &agent.background[0].result else {
            panic!("the script has a result");
        };
        assert!(!failed);
        assert_eq!(text.as_str(), "7");
        assert!(matches!(
            agent.turns.last(),
            Some(Turn::Notice(line)) if line.contains("Background script s1 finished")
        ));
    }

    #[test]
    fn script_result_collects_a_finished_background_script() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        start_background(&mut agent, &mut fake, "return 7;");
        drive_background(&mut agent, &mut fake);
        collect(&mut agent, &mut fake, r#"{"id":"s1"}"#);
        assert_eq!(agent.history.last(), Some(&tool_answer("7")));
        assert!(agent.background.is_empty(), "the collected script is gone");
    }

    #[test]
    fn a_collected_background_script_frees_its_slot() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        for _ in 0..MAX_BACKGROUND_SCRIPTS {
            start_background(&mut agent, &mut fake, "return 1;");
            drive_background(&mut agent, &mut fake);
        }
        assert_eq!(agent.background.len(), MAX_BACKGROUND_SCRIPTS);
        collect(&mut agent, &mut fake, r#"{"id":"s1"}"#);
        assert_eq!(agent.background.len(), MAX_BACKGROUND_SCRIPTS - 1);
        start_background(&mut agent, &mut fake, "return 1;");
        assert_eq!(agent.background.last().map(|bg| bg.id.as_str()), Some("s5"));
        drive_background(&mut agent, &mut fake);
    }

    #[test]
    fn script_result_says_the_script_is_still_running() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        start_background(&mut agent, &mut fake, "return 7;");
        collect(&mut agent, &mut fake, r#"{"id":"s1"}"#);
        match agent.history.last() {
            Some(ApiMessage::Tool { content, .. }) => {
                assert!(content.contains("still running"), "{content}");
            }
            other => panic!("expected the answer in the history, got {other:?}"),
        }
    }

    #[test]
    fn script_result_with_a_wait_parks_the_call() {
        let mut agent = Agent::new();
        agent.now_ms = 1_700_000_000_000;
        let mut fake = FakeSession::default();
        start_background(&mut agent, &mut fake, "return 7;");
        collect(&mut agent, &mut fake, r#"{"id":"s1","waitSeconds":5}"#);
        assert!(matches!(
            agent.phase,
            Some(Phase::Running {
                waiting: Waiting::BackgroundScript { .. },
                ..
            })
        ));
        // The script lands inside the wait, which answers the call without timing out.
        drive_background(&mut agent, &mut fake);
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.poll(&mut session);
        assert!(agent.phase.is_none(), "the wait is over");
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content == "7"
        ));
    }

    #[test]
    fn script_result_names_nothing_when_no_script_matches() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        collect(&mut agent, &mut fake, r#"{"id":"s9"}"#);
        match agent.history.last() {
            Some(ApiMessage::Tool { content, .. }) => {
                assert!(content.contains("no background script"), "{content}");
            }
            other => panic!("expected the answer in the history, got {other:?}"),
        }
    }

    #[test]
    fn a_background_script_asking_for_more_time_waits_for_the_user() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_SCRIPT.to_owned(),
                    arguments: r#"{"code":"return 1;","timeoutSeconds":120,"background":true}"#
                        .to_owned(),
                },
                &mut session,
            );
        }
        assert!(matches!(
            agent.phase,
            Some(Phase::Extending {
                seconds: 120,
                background: true,
                ..
            })
        ));
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.answer_extension(true, &mut session);
        }
        assert!(agent.phase.is_none(), "the conversation is free");
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. })
                if content.contains("Started background script s1")
        ));
        agent.stop_background(0);
    }

    /// Start a background script the way the model does.
    fn start_background(agent: &mut Agent, fake: &mut FakeSession, code: &str) {
        let mut session: Option<&mut dyn SessionAccess> = Some(fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_SCRIPT.to_owned(),
                arguments: serde_json::json!({ "code": code, "background": true }).to_string(),
            },
            &mut session,
        );
    }

    /// Ask for a background script's result the way the model does.
    fn collect(agent: &mut Agent, fake: &mut FakeSession, arguments: &str) {
        let mut session: Option<&mut dyn SessionAccess> = Some(fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c2".to_owned(),
                name: TOOL_SCRIPT_RESULT.to_owned(),
                arguments: arguments.to_owned(),
            },
            &mut session,
        );
    }

    /// The provider history entry a tool answers with.
    fn tool_answer(content: &str) -> ApiMessage {
        ApiMessage::Tool {
            tool_call_id: "c2".to_owned(),
            content: content.to_owned(),
        }
    }

    #[test]
    fn a_script_within_the_default_needs_no_approval() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        start_script(
            &mut agent,
            &mut fake,
            "const r = await tools.get_current_time({}); return r.text;",
        );
        assert!(matches!(
            agent.turns.last(),
            Some(Turn::Script { result, failed: false, .. }) if result.is_some()
        ));
    }

    #[test]
    fn a_script_cannot_start_another_script() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        start_script(
            &mut agent,
            &mut fake,
            "await tools.run_script({ code: 'return 1;' });",
        );
        let Some(Turn::Script { calls, failed, .. }) = agent.turns.last() else {
            panic!("expected a script turn, got {:?}", agent.turns.last());
        };
        assert!(failed, "the rejection ends the run");
        assert_eq!(calls.len(), 1);
        assert!(calls[0].failed);
        assert!(
            calls[0].output.contains("cannot start another script"),
            "{}",
            calls[0].output
        );
    }

    #[test]
    fn plan_mode_offers_run_script_and_refuses_what_it_would_change() {
        let mut agent = Agent::new();
        agent.plan = true;
        assert!(agent.offers_tool(TOOL_RUN_SCRIPT));
        when_offered(&mut agent, TOOL_REMOTE_WRITE);
        let mut fake = FakeSession::default();
        start_script(
            &mut agent,
            &mut fake,
            "await tools.remote_fs_write_file({ path: '/tmp/x', content: 'hi' });",
        );
        let Some(Turn::Script { calls, failed, .. }) = agent.turns.last() else {
            panic!("expected a script turn, got {:?}", agent.turns.last());
        };
        assert!(failed);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].output.contains(PLAN_READ_ONLY), "{}", calls[0].output);
    }

    #[test]
    fn script_mode_refuses_a_tool_call_that_does_not_come_from_a_script() {
        let mut agent = Agent::new();
        agent.script_mode = true;
        agent.now_ms = 1_700_000_000_000;
        let mut fake = FakeSession::default();
        let output = run_tool(&mut agent, &mut fake, TOOL_CURRENT_TIME, "{}");
        assert!(output.contains("script mode"), "{output}");
        assert!(matches!(
            agent.turns.last(),
            Some(Turn::Tool { failed: true, .. })
        ));

        // The same tool runs when the script asks for it.
        let mut scripted = Agent::new();
        scripted.script_mode = true;
        let mut fake = FakeSession::default();
        start_script(
            &mut scripted,
            &mut fake,
            "const now = await tools.get_current_time({});\nreturn now.text.length;",
        );
        assert!(matches!(
            scripted.turns.last(),
            Some(Turn::Script { failed: false, .. })
        ));
    }

    #[test]
    fn stopping_a_script_answers_the_one_call_and_leaves_the_history_valid() {
        let mut agent = Agent::new();
        let call = ApiToolCall {
            id: "c1".to_owned(),
            name: TOOL_RUN_SCRIPT.to_owned(),
            arguments: serde_json::json!({ "code": "await tools.get_current_time({});" })
                .to_string(),
        };
        agent.history.push(ApiMessage::Assistant {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![call.clone()],
        });
        let mut fake = FakeSession::default();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(call, &mut session);
        }
        agent.stop();
        assert!(!agent.blocked());
        assert!(matches!(
            agent.turns.last(),
            Some(Turn::Script { failed: true, .. })
        ));
        // The history stays one the provider accepts: the call is answered once.
        assert_eq!(agent.history.len(), 2, "{:?}", agent.history);
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content == STOPPED_BY_USER
        ));
    }

    #[test]
    fn ask_user_is_read_from_every_shape_a_model_writes() {
        let ask = |arguments: &str| {
            let mut agent = Agent::new();
            let mut session: Option<&mut dyn SessionAccess> = None;
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_ASK_USER.to_owned(),
                    arguments: arguments.to_owned(),
                },
                &mut session,
            );
            match &agent.phase {
                Some(Phase::Asking { questions, .. }) => questions.clone(),
                _ => panic!("expected a question card"),
            }
        };

        // A bare array, which is what a script writes as `tools.ask_user([...])`.
        let questions = ask(
            r#"[{"question":"Which host?","options":["dev","prod"]},{"question":"Then what?"}]"#,
        );
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].text, "Which host?");
        assert_eq!(questions[0].options, ["dev", "prod"]);
        assert_eq!(questions[1].text, "Then what?");
        assert_eq!(ask(r#"{"question":"Which file?"}"#)[0].text, "Which file?");
        assert_eq!(ask(r#""Which host?""#)[0].text, "Which host?");
        // The documented shape and the old single text keep working.
        assert_eq!(ask(r#"{"questions":[{"question":"Why?"}]}"#)[0].text, "Why?");
        assert_eq!(ask(r#"{"text":"Really?"}"#)[0].text, "Really?");
        // Nothing usable still asks something the user can answer.
        assert_eq!(ask("{}")[0].text, QUESTION_FALLBACK);
    }

    #[test]
    fn ask_user_waits_for_every_question_before_continuing() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_ASK_USER.to_owned(),
                arguments: r#"{"questions":[{"question":"Which language?","options":["Rust","Go"]},{"question":"Which file?"}]}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.asking());

        // A partial set, or a blank answer, cannot continue.
        agent.answer(&["Rust".to_owned()], &mut session);
        assert!(agent.asking());
        assert!(!agent.needs_reply, "one answer of two cannot continue");
        agent.answer(&["Rust".to_owned(), "  ".to_owned()], &mut session);
        assert!(agent.asking());
        assert!(
            !agent
                .turns
                .iter()
                .any(|turn| matches!(turn, Turn::Question { .. })),
            "nothing was answered, so nothing goes in the transcript"
        );

        agent.answer(&["Rust".to_owned(), "main.rs".to_owned()], &mut session);
        assert!(!agent.asking());
        assert!(agent.needs_reply);
        assert!(agent.turns.iter().any(|turn| matches!(
            turn,
            Turn::Question { answer: Some(answer), .. } if answer == "Rust"
        )));
        assert!(agent.turns.iter().any(|turn| matches!(
            turn,
            Turn::Question { answer: Some(answer), .. } if answer == "main.rs"
        )));
        assert!(matches!(
            agent.turns.iter().find(|turn| matches!(turn, Turn::Tool { .. })),
            Some(Turn::Tool { output, .. })
                if output.contains("Which language?: Rust")
                    && output.contains("Which file?: main.rs")
        ));
    }

    #[test]
    fn a_forbidden_command_is_refused_and_reported_to_the_model() {
        let mut agent = Agent::new();
        agent.app_deny = vec!["rm -rf *".to_owned()];
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"rm -rf /"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(matches!(&agent.turns[0], Turn::Tool { failed: true, .. }));
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_pane_tool_runs_the_command_it_builds_once_approved() {
        let mut agent = Agent::new();
        when_offered(&mut agent, "docker_start");
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "web\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: "docker_start".to_owned(),
                arguments: r#"{"container":"web"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.approving(), "a docker command has no allow rule yet");
        agent.approve(true, None, &mut session);
        agent.poll(&mut session);
        assert!(matches!(
            agent.turns.last(),
            Some(Turn::Tool { failed: false, .. })
        ));
        assert!(
            fake.commands
                .iter()
                .any(|(_, command)| command.contains("docker start 'web'")),
            "{:?}",
            fake.commands
        );
    }

    #[test]
    fn a_pane_tool_without_its_container_answers_instead_of_running() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        let output = run_tool(&mut agent, &mut fake, "docker_stop", "{}");
        assert!(output.contains("container argument"), "{output}");
        assert!(fake.commands.is_empty(), "{:?}", fake.commands);
    }

    #[test]
    fn a_command_outside_the_rules_waits_for_approval() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"systemctl restart nginx"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.approving());
        // Without a session the denial still lets the model continue.
        agent.approve(false, None, &mut session);
        assert!(!agent.approving());
        assert!(agent.needs_reply);
        assert!(matches!(&agent.turns[0], Turn::Tool { failed: true, .. }));
    }

    #[test]
    fn a_running_command_can_be_stopped_from_the_transcript() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["pwd".to_owned()];
        let mut fake = FakeSession::default();
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"pwd"}"#.to_owned(),
            },
            &mut session,
        );
        // The fake never answers, so the card is the running one.
        assert_eq!(agent.running_command(), Some(0));
        agent.stop_command(&mut session);
        assert_eq!(fake.cancelled, [EXEC_KEY.to_owned()]);
        // A finished tool has nothing to stop.
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        assert_eq!(agent.running_command(), None);
        agent.stop_command(&mut session);
    }

    #[test]
    fn folding_keeps_the_recent_exchange_and_carries_the_summary() {
        let mut agent = Agent::new();
        for step in 0..4 {
            assert!(agent.push_user(&format!("question {step}"), UserMeta::default()));
            agent.history.push(ApiMessage::Assistant {
                content: format!("answer {step}"),
                reasoning_content: None,
                tool_calls: Vec::new(),
            });
            agent.turns.push(Turn::Assistant {
                text: format!("answer {step}"),
                stopped: false,
            });
        }

        // Four exchanges, the last two kept: the cut lands on a user message.
        let cut = agent.compact_split(4).expect("half the chat can be folded");
        assert_eq!(cut, 4);
        assert!(agent.folded_text(cut).contains("question 0"));

        assert_eq!(agent.fold(cut, "Goal: nothing yet"), 4);
        assert_eq!(agent.compacted.as_deref(), Some("Goal: nothing yet"));
        assert_eq!(
            agent.history.first(),
            Some(&ApiMessage::User {
                content: "question 2".to_owned(),
                images: Vec::new(),
            }),
            "the kept tail starts at the first message that stays"
        );
        assert_eq!(
            agent.turns.first(),
            Some(&Turn::User {
                text: "question 2".to_owned(),
                meta: UserMeta::default()
            }),
            "the transcript loses the turns folded with it, so it still lines up"
        );
        assert_eq!(agent.history.len(), 4);
        assert_eq!(agent.turns.len(), 4);
    }

    #[test]
    fn a_short_conversation_is_left_alone() {
        let mut agent = Agent::new();
        assert!(agent.push_user("hello", UserMeta::default()));
        assert_eq!(agent.compact_split(6), None, "nothing to fold yet");
    }

    #[test]
    fn the_fold_never_separates_a_tool_result_from_its_call() {
        let mut agent = Agent::new();
        assert!(agent.push_user("first", UserMeta::default()));
        agent.history.push(ApiMessage::Assistant {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![ApiToolCall {
                id: "call-1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: "{}".to_owned(),
            }],
        });
        agent.history.push(ApiMessage::Tool {
            tool_call_id: "call-1".to_owned(),
            content: "uptime output".to_owned(),
        });
        assert!(agent.push_user("second", UserMeta::default()));
        agent.turns.push(Turn::Assistant {
            text: "answer".to_owned(),
            stopped: false,
        });
        agent.history.push(ApiMessage::Assistant {
            content: "answer".to_owned(),
            reasoning_content: None,
            tool_calls: Vec::new(),
        });

        // The tail has to start at a user message, never at a tool result.
        let cut = agent.compact_split(2).expect("one exchange can be folded");
        assert_eq!(cut, 3, "the tail starts at the second question");
        assert!(matches!(agent.history[cut], ApiMessage::User { .. }));
        agent.fold(cut, "Goal: uptime checked");
        assert!(matches!(agent.history.first(), Some(ApiMessage::User { .. })));
    }

    #[test]
    fn a_clear_drops_the_compacted_summary() {
        let mut agent = Agent::new();
        agent.compacted = Some("older context".to_owned());
        agent.clear();
        assert_eq!(agent.compacted, None);
    }

    #[test]
    fn the_conversation_size_follows_every_append_and_growth() {
        let mut agent = Agent::new();
        assert_eq!(agent.history_chars(), 0);
        assert!(agent.push_user("hello", UserMeta::default()));
        assert_eq!(agent.history_chars(), 5);
        agent.history.push(ApiMessage::Assistant {
            content: "hi".to_owned(),
            reasoning_content: None,
            tool_calls: Vec::new(),
        });
        assert_eq!(agent.history_chars(), 7);
        // A streaming reply grows the last message without adding one.
        if let Some(ApiMessage::Assistant { content, .. }) = agent.history.last_mut() {
            content.push_str(" there");
        }
        assert_eq!(agent.history_chars(), 13);
    }

    #[test]
    fn rewinding_after_a_compaction_lines_up_with_the_history() {
        let mut agent = Agent::new();
        for step in 0..4 {
            assert!(agent.push_user(&format!("question {step}"), UserMeta::default()));
            agent.history.push(ApiMessage::Assistant {
                content: format!("answer {step}"),
                reasoning_content: None,
                tool_calls: Vec::new(),
            });
            agent.turns.push(Turn::Assistant {
                text: format!("answer {step}"),
                stopped: false,
            });
        }
        let cut = agent.compact_split(2).expect("the older half can be folded");
        agent.fold(cut, "Goal: earlier questions");
        // Transcript and history carry the same user turns again.
        let index = agent
            .turns
            .iter()
            .position(|turn| matches!(turn, Turn::User { text, .. } if text == "question 3"))
            .expect("the last question is still there");
        assert_eq!(agent.rewind_to(index).as_deref(), Some("question 3"));
        assert!(agent.history.is_empty(), "the question is being rewritten");
        assert_eq!(
            agent.compacted.as_deref(),
            Some("Goal: earlier questions"),
            "what was compacted away is still the work behind the chat"
        );
    }

    #[test]
    fn a_reply_with_nothing_to_say_leaves_no_empty_bubble() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["pwd".to_owned()];
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "/srv/app\n__WASSH_STATUS:0__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        // A reply that only asked for a tool: the provider still needs the assistant turn,
        // the transcript does not.
        agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        agent.finish_reply(
            0,
            Ok(Completion {
                text: String::new(),
                reasoning_content: Some("checking the environment".to_owned()),
                tool_calls: vec![ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_COMMAND.to_owned(),
                    arguments: r#"{"command":"pwd"}"#.to_owned(),
                }],
                stop_reason: None,
            }),
            &mut session,
        );
        assert!(
            matches!(agent.turns.first(), Some(Turn::Tool { .. })),
            "an empty assistant turn was kept: {:?}",
            agent.turns
        );
        assert!(matches!(
            agent.history.first(),
            Some(ApiMessage::Assistant {
                content,
                reasoning_content: Some(reasoning),
                ..
            }) if content.is_empty() && reasoning == "checking the environment"
        ));

        // Whitespace alone is no more a message than nothing at all.
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        agent.finish_reply(
            0,
            Ok(Completion {
                text: "\n \n".to_owned(),
                ..Completion::default()
            }),
            &mut session,
        );
        assert!(agent.turns.is_empty(), "{:?}", agent.turns);
    }

    #[test]
    fn a_denied_capability_never_runs() {
        let mut agent = Agent::new();
        agent.permissions = Permissions::from_settings(&serde_json::Map::new());
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_WEB_FETCH.to_owned(),
                arguments: r#"{"url":"https://example.com"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content == NOT_PERMITTED
        ));
        assert!(agent.needs_reply, "the model is told and carries on");
    }

    #[test]
    fn an_asked_capability_waits_for_the_user() {
        let mut agent = Agent::new();
        agent.phase = None;
        agent.permissions = Permissions::from_settings(&serde_json::Map::from_iter([(
            super::super::permissions::group_key(super::super::permissions::GROUP_TERMINAL),
            Value::Bool(true),
        )]));
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_TERMINAL_WRITE.to_owned(),
                arguments: r#"{"text":"ls\n"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.allowing(), "the pane has to ask");
        assert!(agent.blocked(), "and hold the conversation");
        agent.answer_permission(false, &mut session);
        assert!(!agent.allowing());
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content == DENIED_BY_USER
        ));
        assert!(agent.needs_reply);
    }

    #[test]
    fn allowing_everything_covers_the_rest_of_the_session() {
        let mut agent = Agent::new();
        agent.permissions = Permissions::from_settings(&serde_json::Map::from_iter([(
            super::super::permissions::group_key(super::super::permissions::GROUP_TERMINAL),
            Value::Bool(true),
        )]));
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_TERMINAL_WRITE.to_owned(),
                arguments: r#"{"text":"ls"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.allowing(), "typing into the terminal asks first");
        agent.allow_everything(&mut session);
        assert!(!agent.allowing());

        // The next capability runs without a card...
        agent.begin_tool(
            ApiToolCall {
                id: "c2".to_owned(),
                name: TOOL_TERMINAL_WRITE.to_owned(),
                arguments: r#"{"text":"pwd"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(!agent.allowing(), "the session answer was forgotten");
        // ...and so does a command that would otherwise need approval.
        agent.begin_tool(
            ApiToolCall {
                id: "c3".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"rm -rf /tmp/wassh-test"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(!agent.approving(), "commands are covered too");
    }

    #[test]
    fn approving_everything_covers_all_prompts_but_keeps_the_sudo_password_prompt() {
        let mut agent = Agent::new();
        agent.permissions = Permissions::from_settings(&serde_json::Map::from_iter([(
            super::super::permissions::group_key(super::super::permissions::GROUP_TERMINAL),
            Value::Bool(true),
        )]));
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "approval".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"systemctl restart nginx"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.approving(), "the initial command should ask");
        agent.approve_everything(&mut session);
        assert!(agent.allow_all, "the session-wide permission was not stored");
        agent.permissions = Permissions::from_settings(&serde_json::Map::from_iter([
            (
                super::super::permissions::group_key(
                    super::super::permissions::GROUP_TERMINAL,
                ),
                Value::Bool(true),
            ),
            (
                super::super::permissions::method_key(TOOL_TERMINAL_WRITE),
                Value::String("deny".to_owned()),
            ),
        ]));
        agent.app_deny = vec!["systemctl stop*".to_owned()];
        agent.scopes.load(scopes::find("host-terminal").unwrap());
        assert!(agent.offers_tool(TOOL_TERMINAL_KEYS));
        assert!(!agent.offers_tool(TOOL_TERMINAL_WRITE));

        agent.begin_tool(
            ApiToolCall {
                id: "capability".to_owned(),
                name: TOOL_TERMINAL_WRITE.to_owned(),
                arguments: r#"{"text":"pwd"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(!agent.allowing(), "a denied capability asked instead of refusing");
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content == NOT_PERMITTED
        ));

        agent.begin_tool(
            ApiToolCall {
                id: "ask-capability".to_owned(),
                name: TOOL_TERMINAL_KEYS.to_owned(),
                arguments: r#"{"keys":"CTRL+C"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(!agent.allowing(), "an Ask capability still prompted");
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content == "The session is not connected."
        ));

        agent.begin_tool(
            ApiToolCall {
                id: "command".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"systemctl status nginx"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(!agent.approving(), "a later command still asked");

        agent.begin_tool(
            ApiToolCall {
                id: "denied-command".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"systemctl stop nginx"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(!agent.approving(), "a denied command prompted instead of refusing");
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. }) if content.contains("not allowed by the permission rules")
        ));

        let mut fake = FakeSession::default();
        let mut connected_session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "sudo".to_owned(),
                name: TOOL_RUN_SUDO.to_owned(),
                arguments: r#"{"command":"id"}"#.to_owned(),
            },
            &mut connected_session,
        );
        assert_eq!(agent.sudo_prompt(), Some(("id", None)));
    }

    #[test]
    fn an_allowed_capability_runs_straight_away() {
        let mut agent = Agent::new();
        agent.permissions = Permissions::from_settings(&serde_json::Map::from_iter([(
            super::super::permissions::group_key(super::super::permissions::GROUP_KNOWLEDGE_BASE),
            Value::Bool(true),
        )]));
        agent.tools.rag_folders = vec![std::env::temp_dir().display().to_string()];
        when_offered(&mut agent, TOOL_RAG_SEARCH);
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RAG_SEARCH.to_owned(),
                arguments: r#"{"query":"anything"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(
            matches!(agent.phase, Some(Phase::Running { .. })),
            "no approval"
        );
    }

    #[test]
    fn plan_mode_offers_reads_and_refuses_what_changes_something() {
        let mut agent = Agent::new();
        agent.permissions = Permissions::from_settings(&serde_json::Map::from_iter([(
            super::super::permissions::group_key(super::super::permissions::GROUP_REMOTE_FS),
            Value::Bool(true),
        )]));
        agent.scopes.load(scopes::find("host-files").unwrap());
        agent.plan = true;
        assert!(agent.offers_tool(TOOL_DEV_VIEW));
        assert!(!agent.offers_tool(TOOL_DEV_EDIT));
        assert!(!agent.offers_tool(TOOL_RUN_COMMAND));
        assert!(agent.offers_tool(TOOL_LOAD_SCOPE));

        let mut fake = FakeSession::default();
        fake.contents
            .insert("/srv/app/main.rs".to_owned(), "fn main() {}\n".to_owned());
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.cwd = "/srv/app".to_owned();
        // A call that changes something is answered, not run, even though this host would
        // allow it in Agent mode.
        agent.begin_tool(
            ApiToolCall {
                id: "edit".to_owned(),
                name: TOOL_DEV_EDIT.to_owned(),
                arguments: r#"{"path":"main.rs","oldText":"main","newText":"start"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(
            matches!(
                agent.turns.last(),
                Some(Turn::Tool { failed: true, output, .. }) if output == PLAN_READ_ONLY
            ),
            "{:?}",
            agent.turns.last()
        );
        // Reading still answers.
        agent.begin_tool(
            ApiToolCall {
                id: "view".to_owned(),
                name: TOOL_DEV_VIEW.to_owned(),
                arguments: r#"{"path":"main.rs"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(
            matches!(agent.turns.last(), Some(Turn::Tool { failed: false, .. })),
            "{:?}",
            agent.turns.last()
        );

        // And a sub-agent works under the same restriction.
        let index = agent.begin_subagent("look", "read main.rs");
        assert!(agent.subagents[index].agent.plan);
        assert!(!agent.subagents[index].agent.offers_tool(TOOL_DEV_EDIT));
    }

    #[test]
    fn the_scope_tools_walk_the_graph_and_carry_the_tools_along() {
        let mut agent = Agent::new();
        agent.permissions = Permissions::from_settings(&serde_json::Map::from_iter([(
            super::super::permissions::group_key(super::super::permissions::GROUP_REMOTE_FS),
            Value::Bool(true),
        )]));
        let mut session: Option<&mut dyn SessionAccess> = None;
        let mut ask = |agent: &mut Agent, name: &str, scope: &str| {
            agent.begin_tool(
                ApiToolCall {
                    id: format!("{name} {scope}"),
                    name: name.to_owned(),
                    arguments: format!(r#"{{"scope":"{scope}"}}"#),
                },
                &mut session,
            );
            match agent.turns.last() {
                Some(Turn::Tool { output, failed, .. }) => (*failed, output.clone()),
                other => panic!("expected a tool turn, got {other:?}"),
            }
        };

        // A child is only reachable once the scope above it is loaded.
        let (failed, output) = ask(&mut agent, TOOL_LOAD_SCOPE, "host-files");
        assert!(failed, "{output}");
        assert!(output.contains("Loadable now: host"), "{output}");

        // A tool the host may use, in a scope that is not loaded, is answered instead of run.
        let mut fake = FakeSession::default();
        let mut connected: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "read".to_owned(),
                name: TOOL_REMOTE_READ.to_owned(),
                arguments: r#"{"path":"/etc/hosts"}"#.to_owned(),
            },
            &mut connected,
        );
        assert!(
            matches!(
                agent.turns.last(),
                Some(Turn::Tool { failed: true, output, .. })
                    if output.contains("load_scope") && output.contains("host-files")
            ),
            "{:?}",
            agent.turns.last()
        );

        let (failed, output) = ask(&mut agent, TOOL_LOAD_SCOPE, "host");
        assert!(!failed, "{output}");
        assert!(agent.scopes.contains("host"));
        let (failed, output) = ask(&mut agent, TOOL_LOAD_SCOPE, "host");
        assert!(!failed, "{output}");
        assert!(output.contains("already"), "{output}");
        let (failed, output) = ask(&mut agent, TOOL_LOAD_SCOPE, "host-files");
        assert!(!failed, "{output}");
        assert!(output.contains("12 tools"), "{output}");
        assert!(agent.offers_tool(TOOL_DEV_VIEW));

        // Unloading takes the tools back out; the root stays whatever is asked.
        let (failed, output) = ask(&mut agent, TOOL_UNLOAD_SCOPE, "host");
        assert!(!failed, "{output}");
        assert!(!agent.offers_tool(TOOL_DEV_VIEW));
        let (failed, output) = ask(&mut agent, TOOL_UNLOAD_SCOPE, scopes::ROOT);
        assert!(failed, "{output}");
        assert!(agent.scopes.contains(scopes::ROOT));
        assert!(agent.offers_tool(TOOL_RUN_COMMAND));
    }

    #[test]
    fn ask_user_waits_for_the_answer() {
        let mut agent = Agent::new();
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_ASK_USER.to_owned(),
                arguments: r#"{"text":"Which file?"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.asking());
        agent.answer(&["notes.txt".to_owned()], &mut session);
        assert!(!agent.asking());
        assert!(agent.turns.iter().any(|turn| matches!(
            turn,
            Turn::Question { answer: Some(answer), .. } if answer == "notes.txt"
        )));
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_remote_tool_without_a_session_says_so() {
        let mut agent = Agent::new();
        when_offered(&mut agent, TOOL_REMOTE_LIST);
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_REMOTE_LIST.to_owned(),
                arguments: r#"{"path":"/etc"}"#.to_owned(),
            },
            &mut session,
        );
        assert!(agent.needs_reply);
        match &agent.turns[0] {
            Turn::Tool { output, failed, .. } => {
                assert!(failed);
                assert!(output.contains("not connected"), "{output}");
            }
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }

    /// A session that answers from memory, so the tool paths can be driven.
    #[derive(Default)]
    struct FakeSession {
        screen: String,
        typed: String,
        listing: Option<crate::files::SftpListing>,
        contents: std::collections::HashMap<String, String>,
        /// Raw bytes for files whose content is not text.
        blobs: std::collections::HashMap<String, Vec<u8>>,
        commands: Vec<(String, String)>,
        /// What each `exec_input` wrote to stdin.
        stdin: Vec<String>,
        /// Keys whose command the pane stopped.
        cancelled: Vec<String>,
        answers: std::collections::HashMap<String, crate::command::CommandOutput>,
        uploads: Vec<(String, String)>,
        writes: Vec<(String, Vec<u8>)>,
        transferring: bool,
        status: Option<String>,
    }

    impl crate::files::SftpAccess for FakeSession {
        fn request_listing(&mut self, _path: &str) {}
        fn refresh(&mut self, _path: &str) {}
        fn listing(&self) -> Option<&crate::files::SftpListing> {
            self.listing.as_ref()
        }
        fn listing_revision(&self) -> u64 {
            0
        }
        fn is_loading(&self) -> bool {
            false
        }
        fn error(&self) -> Option<&str> {
            None
        }
        fn upload(&mut self, local: &str, remote: &str) {
            let body = std::fs::read_to_string(local).unwrap_or_default();
            self.uploads.push((remote.to_owned(), body));
            self.status = Some(format!("Uploaded {remote}"));
            self.transferring = false;
        }

        fn write_file(&mut self, remote: &str, content: Vec<u8>) {
            self.writes.push((remote.to_owned(), content));
            self.status = Some(format!("Uploaded {remote}"));
            self.transferring = false;
        }
        fn download(&mut self, _remote: &str, _local: &str) {}
        fn is_transferring(&self) -> bool {
            self.transferring
        }
        fn transfer_status(&self) -> Option<&str> {
            self.status.as_deref()
        }
        fn report_problem(&mut self, message: &str) {
            self.status = Some(message.to_owned());
        }
        fn resolve(&mut self, path: &str) -> Option<String> {
            Some(path.to_owned())
        }
        fn mkdir(&mut self, _path: &str) {}
        fn rename(&mut self, _from: &str, _to: &str) {}
        fn remove(&mut self, _path: &str, _is_dir: bool) {}
        fn chmod(&mut self, _path: &str, _mode: u32) {}
        fn read(&mut self, path: &str, _max_bytes: u64) -> Option<crate::files::FileContent> {
            if let Some(data) = self.blobs.get(path) {
                return Some(crate::files::FileContent {
                    data: data.clone(),
                    truncated: false,
                });
            }
            self.contents
                .get(path)
                .map(|text| crate::files::FileContent {
                    data: text.as_bytes().to_vec(),
                    truncated: false,
                })
        }
        fn upload_many(&mut self, _items: Vec<(String, String)>) {}
        fn download_many(&mut self, _items: Vec<(String, String)>) {}
        fn copy_many(&mut self, _items: Vec<(String, String)>, _move_items: bool) {}
        fn progress(&self) -> Option<(u64, Option<u64>)> {
            None
        }
        fn download_tree(&mut self, _items: Vec<String>, _dir: &str) {}
        fn download_zip(&mut self, _items: Vec<String>, _archive: &str) {}
        fn transfers(&self) -> Vec<crate::files::TransferRow> {
            Vec::new()
        }
        fn retry(&mut self) {}
    }

    impl crate::monitor::MonitorAccess for FakeSession {}

    impl crate::command::CommandAccess for FakeSession {
        fn exec(&mut self, key: &str, command: &str) {
            self.commands.push((key.to_owned(), command.to_owned()));
        }
        fn exec_input(&mut self, key: &str, command: &str, input: &str) {
            self.stdin.push(input.to_owned());
            self.exec(key, command);
        }
        fn output(&self, key: &str) -> Option<&crate::command::CommandOutput> {
            self.answers.get(key)
        }
        fn failure(&self, _key: &str) -> Option<&str> {
            None
        }
        fn is_running(&self, _key: &str) -> bool {
            false
        }
        fn cancel(&mut self, key: &str) {
            self.cancelled.push(key.to_owned());
        }
    }

    impl crate::command::TcpAccess for FakeSession {
        fn open_forward(&mut self, _key: &str, _host: &str, _port: u16) {}

        fn forward_address(&self, _key: &str) -> Option<&str> {
            None
        }

        fn forward_error(&self, _key: &str) -> Option<&str> {
            None
        }

        fn forward_pending(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::plugin::TerminalAccess for FakeSession {
        fn screen_text(&self) -> String {
            self.screen.clone()
        }
        fn type_text(&mut self, text: &str) {
            self.typed.push_str(text);
        }
        fn is_closed(&self) -> bool {
            false
        }
    }

    /// Load the scope a tool lives in, the way a request carrying that tool would have.
    fn when_offered(agent: &mut Agent, tool: &str) {
        if let Some(scope) = super::scopes::scope_of(tool) {
            agent.scopes.load(scope);
        }
    }

    /// Drive one tool call against `fake` until it settles.
    fn run_tool(agent: &mut Agent, fake: &mut FakeSession, name: &str, arguments: &str) -> String {
        when_offered(agent, name);
        let mut session: Option<&mut dyn SessionAccess> = Some(fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: name.to_owned(),
                arguments: arguments.to_owned(),
            },
            &mut session,
        );
        agent.poll(&mut session);
        match agent.turns.last() {
            Some(Turn::Tool { output, .. }) => output.clone(),
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }

    #[test]
    fn a_graph_tool_on_an_empty_map_asks_for_a_scan_and_a_pass() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        assert!(agent.map.is_empty());
        let output = run_tool(&mut agent, &mut fake, TOOL_ENV_OVERVIEW, "{}");
        assert!(
            agent.deep_map_wanted(),
            "the pane is not told to map the tree: {output}"
        );
        assert!(
            output.contains("scan of the project") && output.contains("mapping pass"),
            "the model is not told what is starting: {output}"
        );
    }

    #[test]
    fn a_bare_cd_answers_where_the_shell_ended_up() {
        let mut agent = Agent::new();
        agent.allow_all = true;
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "\n__WASSH_STATUS:0__\n__WASSH_PWD:/srv/app__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_RUN_COMMAND,
            r#"{"command":"cd /srv/app"}"#,
        );
        assert_eq!(
            output,
            "Working directory is now /srv/app.
[exit code: 0]"
        );
        assert_eq!(agent.cwd, "/srv/app");
    }

    #[test]
    fn a_command_runs_through_the_session_and_moves_the_directory() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["pwd".to_owned()];
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "/srv/app\n__WASSH_STATUS:0__\n__WASSH_PWD:/srv/app__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_RUN_COMMAND,
            r#"{"command":"pwd"}"#,
        );
        assert_eq!(
            output,
            "/srv/app
Working directory is now /srv/app.
[exit code: 0]"
        );
        // The wrapper ran in the cwd and reported it back; the move is said out loud, so a
        // model that cd-ed keeps the directory instead of repeating it in every command.
        let sent = &fake.commands[0].1;
        assert!(sent.starts_with("cd '/' "), "{sent}");
        assert_eq!(agent.cwd, "/srv/app");
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_file_search_on_an_unmapped_tree_points_at_the_graph() {
        let mut agent = Agent::new();
        when_offered(&mut agent, TOOL_DEV_GREP);
        let mut fake = FakeSession::default();
        fake.answers.insert(
            PROBE_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "main.rs:1:fn main\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let output = run_tool(&mut agent, &mut fake, TOOL_DEV_GREP, r#"{"pattern":"main"}"#);
        assert!(
            output.contains(NO_MAP_HINT),
            "the search did not point at the map: {output}"
        );
        // Once the tree is mapped the search stands on its own.
        agent.map = super::super::map::EnvMap::parse(
            "/srv/app",
            1,
            "10 5 /srv/app/main.rs\n__wassh_symbols__\n",
        );
        let output = run_tool(&mut agent, &mut fake, TOOL_DEV_GREP, r#"{"pattern":"main"}"#);
        assert!(!output.contains(NO_MAP_HINT), "{output}");
    }

    #[test]
    fn a_working_directory_change_moves_the_calls_after_it() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "/srv/app\n__WASSH_STATUS:0__\n__WASSH_PWD:/srv/app__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_CHANGE_DIR,
            r#"{"path":"/srv/app"}"#,
        );
        assert_eq!(agent.cwd, "/srv/app", "{output}");
        // A relative path of a later call lands in the new directory.
        assert_eq!(relative_remote(&agent.cwd, "src"), "/srv/app/src");
        assert!(output.contains("/srv/app"), "{output}");
        assert!(
            fake.commands[0].1.contains("cd '/srv/app' && pwd"),
            "the change did not run as a cd: {:?}",
            fake.commands[0].1
        );
    }

    #[test]
    fn a_change_to_a_directory_that_is_not_there_leaves_the_cwd_alone() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "\n__WASSH_STATUS:2__\n__WASSH_PWD:/__\n".to_owned(),
                stderr: "cd: can't cd to /nope\n".to_owned(),
                code: Some(2),
            },
        );
        let output = run_tool(&mut agent, &mut fake, TOOL_CHANGE_DIR, r#"{"path":"/nope"}"#);
        assert_eq!(agent.cwd, "/", "the working directory followed a failed cd");
        assert!(output.contains("can't cd to /nope"), "{output}");
        assert!(output.contains("[exit code: 2]"), "{output}");
    }

    #[test]
    fn a_remote_read_answers_from_the_session() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        fake.contents.insert(
            "/etc/hosts".to_owned(),
            "127.0.0.1 localhost
"
            .to_owned(),
        );
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_REMOTE_READ,
            r#"{"path":"/etc/hosts"}"#,
        );
        assert_eq!(
            output,
            "127.0.0.1 localhost
"
        );
    }

    #[test]
    fn an_image_tool_shows_the_model_the_picture() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        fake.blobs.insert(
            "/tmp/a.png".to_owned(),
            vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1],
        );
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_DEV_IMAGE,
            r#"{"path":"/tmp/a.png"}"#,
        );
        assert!(output.contains("is shown to you"), "{output}");
        match agent.history.last() {
            Some(ApiMessage::User { images, .. }) => {
                assert_eq!(images.len(), 1, "{images:?}");
                assert_eq!(images[0].media_type, "image/png");
            }
            other => panic!("expected the image message, got {other:?}"),
        }
    }

    #[test]
    fn a_picture_a_script_reads_stays_out_of_the_provider_history() {
        let mut agent = Agent::new();
        agent
            .scopes
            .load(scopes::find("host-files").expect("the file scope"));
        let mut fake = FakeSession::default();
        fake.blobs
            .insert("/tmp/inbox.png".to_owned(), png_bytes());
        start_script(
            &mut agent,
            &mut fake,
            "const r = await tools.dev_view_image({ path: '/tmp/inbox.png' });\n\
             return { text: r.text };",
        );
        // The picture belongs to the script: the model reads the one result instead, so no
        // message is left between the script call and the answer to it.
        assert!(
            agent
                .history
                .iter()
                .all(|message| !matches!(message, ApiMessage::User { .. })),
            "{:?}",
            agent.history
        );
        assert!(
            matches!(
                agent.history.last(),
                Some(ApiMessage::Tool { content, .. }) if content.contains("is shown to you")
            ),
            "{:?}",
            agent.history
        );
        assert!(agent.needs_reply);
    }

    #[test]
    fn a_picture_read_between_two_calls_waits_for_both_answers() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["pwd".to_owned()];
        when_offered(&mut agent, TOOL_DEV_IMAGE);
        let mut fake = FakeSession::default();
        fake.blobs.insert("/tmp/a.png".to_owned(), png_bytes());
        agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.finish_reply(
                0,
                Ok(Completion {
                    text: String::new(),
                    tool_calls: vec![
                        ApiToolCall {
                            id: "image".to_owned(),
                            name: TOOL_DEV_IMAGE.to_owned(),
                            arguments: r#"{"path":"/tmp/a.png"}"#.to_owned(),
                        },
                        ApiToolCall {
                            id: "command".to_owned(),
                            name: TOOL_RUN_COMMAND.to_owned(),
                            arguments: r#"{"command":"pwd"}"#.to_owned(),
                        },
                    ],
                    ..Completion::default()
                }),
                &mut session,
            );
            // The picture answers first, while the command is still out.
            agent.poll(&mut session);
        }
        // The image message waits so the command's own result follows the call it answers.
        assert!(
            matches!(agent.history.last(), Some(ApiMessage::Tool { tool_call_id, .. }) if tool_call_id == "image"),
            "{:?}",
            agent.history
        );
        assert!(
            agent
                .shown_images
                .iter()
                .any(|(content, _)| content.contains("/tmp/a.png")),
            "the picture is not held: {:?}",
            agent.history
        );
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "/srv/app\n__WASSH_STATUS:0__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.poll(&mut session);
        }
        // Both calls are answered before the picture joins the conversation.
        assert!(
            matches!(
                &agent.history[agent.history.len() - 2],
                ApiMessage::Tool { tool_call_id, .. } if tool_call_id == "command"
            ),
            "{:?}",
            agent.history
        );
        match agent.history.last() {
            Some(ApiMessage::User { images, .. }) => assert_eq!(images.len(), 1),
            other => panic!("expected the image message, got {other:?}"),
        }
    }

    /// A one-pixel PNG: the header is what `dev_view_image` reads the type from.
    fn png_bytes() -> Vec<u8> {
        vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1]
    }

    #[test]
    fn stored_history_keeps_attached_images() {
        let messages = vec![ApiMessage::User {
            content: "look".to_owned(),
            images: vec![ImagePart {
                media_type: "image/png".to_owned(),
                data: vec![1, 2, 3],
            }],
        }];
        let stored = history_to_json(&messages);
        assert_eq!(stored[0]["images"][0]["data"], "AQID");
        assert_eq!(history_from_json(&stored), messages);
    }

    #[test]
    fn a_remote_read_can_answer_in_base64() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        fake.contents.insert("/bin/tool".to_owned(), "hi".to_owned());
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_REMOTE_READ,
            r#"{"path":"/bin/tool","encoding":"base64"}"#,
        );
        assert_eq!(output, "aGk=");
    }

    #[test]
    fn a_patch_file_is_read_then_applied_to_the_target() {
        let mut agent = Agent::new();
        when_offered(&mut agent, TOOL_REMOTE_PATCH);
        let mut fake = FakeSession::default();
        fake.contents
            .insert("/srv/app/main.rs".to_owned(), "one\ntwo\n".to_owned());
        fake.answers.insert(
            PROBE_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "@@ -1,2 +1,2 @@\n one\n-two\n+three\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_REMOTE_PATCH.to_owned(),
                arguments: r#"{"path":"/srv/app/main.rs","patchFile":"/srv/app/fix.diff"}"#
                    .to_owned(),
            },
            &mut session,
        );
        for _ in 0..4 {
            agent.poll(&mut session);
        }
        assert!(
            fake.commands
                .iter()
                .any(|(_, command)| command == "cat '/srv/app/fix.diff'"),
            "{:?}",
            fake.commands
        );
        let write = fake.writes.last().cloned();
        assert_eq!(
            write.as_ref().map(|(path, _)| path.as_str()),
            Some("/srv/app/main.rs")
        );
        assert!(
            write.is_some_and(|(_, content)| content == b"one\nthree\n"),
            "{:?}",
            fake.writes
        );
    }

    #[test]
    fn a_remote_edit_writes_owned_bytes_and_reports_the_diff() {
        let mut agent = Agent::new();
        when_offered(&mut agent, TOOL_REMOTE_EDIT);
        let mut fake = FakeSession::default();
        // The edit resolves `translated.txt` against the working directory.
        fake.contents
            .insert("/translated.txt".to_owned(), "one\ntwo\n".to_owned());
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_REMOTE_EDIT.to_owned(),
                arguments: r#"{"path":"translated.txt","oldText":"two","newText":"TWO"}"#
                    .to_owned(),
            },
            &mut session,
        );
        agent.poll(&mut session);
        let write = fake.writes.last().cloned();
        assert_eq!(
            write.as_ref().map(|(path, _)| path.as_str()),
            Some("/translated.txt")
        );
        assert!(
            write
                .as_ref()
                .is_some_and(|(_, content)| content.as_slice() == b"one\nTWO\n")
        );
        match agent.turns.last().cloned().expect("a tool turn") {
            Turn::Tool { output, failed, .. } => {
                assert!(!failed, "{output}");
                assert!(output.contains("+   1 TWO"), "{output}");
            }
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }

    #[test]
    fn a_remote_listing_is_formatted_for_the_model() {
        let mut agent = Agent::new();
        let mut fake = FakeSession {
            listing: Some(crate::files::SftpListing {
                path: "/etc".to_owned(),
                entries: vec![
                    crate::files::SftpEntry {
                        name: "nginx".to_owned(),
                        path: "/etc/nginx".to_owned(),
                        is_dir: true,
                        is_link: false,
                        size: 0,
                        modified: None,
                        permissions: None,
                    },
                    crate::files::SftpEntry {
                        name: "hosts".to_owned(),
                        path: "/etc/hosts".to_owned(),
                        is_dir: false,
                        is_link: false,
                        size: 220,
                        modified: None,
                        permissions: None,
                    },
                ],
            }),
            ..FakeSession::default()
        };
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_REMOTE_LIST,
            r#"{"path":"/etc"}"#,
        );
        assert!(output.contains("dir   nginx"), "{output}");
        assert!(output.contains("file       220  hosts"), "{output}");
    }

    #[test]
    fn the_developer_file_tools_work_on_the_host_not_this_computer() {
        let mut agent = Agent::new();
        agent.cwd = "/srv/app".to_owned();
        let mut fake = FakeSession::default();
        fake.contents.insert(
            "/srv/app/main.rs".to_owned(),
            "fn main() {\n    greet();\n}\n".to_owned(),
        );

        // Viewing numbers the lines of the host's file, resolved against the cwd.
        let view = run_tool(
            &mut agent,
            &mut fake,
            TOOL_DEV_VIEW,
            r#"{"path":"main.rs","startLine":2}"#,
        );
        assert!(view.starts_with("/srv/app/main.rs\n"), "{view}");
        assert!(view.contains("    2\t    greet();"), "{view}");
        assert!(!view.contains("fn main"), "the range starts at line 2: {view}");

        // Editing writes the new text back to the host.
        let diff = run_tool(
            &mut agent,
            &mut fake,
            TOOL_DEV_EDIT,
            r#"{"path":"main.rs","oldText":"greet","newText":"hello"}"#,
        );
        assert!(diff.contains("+   1     hello();"), "{diff}");
        assert_eq!(
            fake.writes.last().map(|(path, _)| path.as_str()),
            Some("/srv/app/main.rs")
        );
    }

    #[test]
    fn the_developer_search_tools_ask_the_host() {
        let mut agent = Agent::new();
        agent.cwd = "/srv/app".to_owned();
        let mut fake = FakeSession::default();
        run_tool(
            &mut agent,
            &mut fake,
            TOOL_DEV_GREP,
            r#"{"pattern":"fn main","path":"src"}"#,
        );
        run_tool(&mut agent, &mut fake, TOOL_DEV_FIND, r#"{"namePattern":"*.rs"}"#);
        run_tool(
            &mut agent,
            &mut fake,
            TOOL_DEV_LIST,
            r#"{"path":"src","recursive":true}"#,
        );
        let commands: Vec<&str> = fake
            .commands
            .iter()
            .map(|(_, command)| command.as_str())
            .collect();
        assert_eq!(commands.len(), 3, "{commands:?}");
        assert!(
            commands[0].starts_with("if command -v rg"),
            "{commands:?}"
        );
        assert!(commands[0].contains("'/srv/app/src'"), "{commands:?}");
        assert!(commands[1].contains("-iname '*.rs'"), "{commands:?}");
        assert!(commands[1].contains("'/srv/app'"), "{commands:?}");
        assert!(commands[2].contains("-maxdepth"), "{commands:?}");
        assert!(commands[2].contains("'/srv/app/src'"), "{commands:?}");
    }

    #[test]
    fn a_developer_diff_reads_the_host_file_when_the_old_text_is_missing() {
        let mut agent = Agent::new();
        agent.cwd = "/srv/app".to_owned();
        let mut fake = FakeSession::default();
        fake.contents
            .insert("/srv/app/note.txt".to_owned(), "one\ntwo\n".to_owned());
        let diff = run_tool(
            &mut agent,
            &mut fake,
            TOOL_DEV_DIFF,
            r#"{"path":"note.txt","newText":"one\nTWO\n"}"#,
        );
        assert!(diff.contains("+   1 TWO"), "{diff}");

        // Both sides in the call: the file is never read.
        fake.contents.clear();
        let both = run_tool(
            &mut agent,
            &mut fake,
            TOOL_DEV_DIFF,
            r#"{"path":"note.txt","oldText":"a\n","newText":"b\n"}"#,
        );
        assert!(both.contains("+   0 b"), "{both}");
    }

    #[test]
    fn terminal_tools_read_and_type_on_the_real_screen() {
        let mut agent = Agent::new();
        let mut fake = FakeSession {
            screen: "$ uptime
 10:00 up 3 days"
                .to_owned(),
            ..FakeSession::default()
        };
        let read = run_tool(&mut agent, &mut fake, TOOL_TERMINAL_READ, "{}");
        assert!(read.contains("up 3 days"), "{read}");

        let typed = run_tool(
            &mut agent,
            &mut fake,
            TOOL_TERMINAL_WRITE,
            r#"{"text":"ls","pressEnter":true}"#,
        );
        assert!(typed.contains("Typed 2 characters and Enter"), "{typed}");
        assert_eq!(fake.typed, "ls\r");

        let keys = run_tool(
            &mut agent,
            &mut fake,
            TOOL_TERMINAL_KEYS,
            r#"{"keys":"ctrl-c"}"#,
        );
        assert!(keys.contains("Sent ctrl-c"), "{keys}");
        assert!(fake.typed.ends_with('\u{3}'));
    }

    #[test]
    fn a_wait_returns_once_the_screen_changes() {
        let mut agent = Agent::new();
        agent.now_ms = 1_000;
        when_offered(&mut agent, TOOL_TERMINAL_WAIT);
        let mut fake = FakeSession {
            screen: "working".to_owned(),
            ..FakeSession::default()
        };
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_TERMINAL_WAIT.to_owned(),
                    arguments: r#"{"timeoutMs":5000}"#.to_owned(),
                },
                &mut session,
            );
            // Nothing changed yet and the timeout has not passed.
            assert!(!agent.poll(&mut session));
        }
        fake.screen = "done".to_owned();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            assert!(agent.poll(&mut session));
        }
        match agent.turns.last() {
            Some(Turn::Tool { output, .. }) => assert!(output.contains("done"), "{output}"),
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }

    #[test]
    fn the_sudo_tool_asks_for_the_password_and_then_runs_the_command() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["apt-get*".to_owned()];
        let mut fake = FakeSession::default();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_SUDO.to_owned(),
                    arguments: r#"{"command":"apt-get update","reason":"refresh packages"}"#
                        .to_owned(),
                },
                &mut session,
            );
        }

        // Nothing runs until the user types a password.
        assert!(
            fake.commands.is_empty(),
            "the command ran without a password"
        );
        assert_eq!(
            agent.sudo_prompt(),
            Some(("apt-get update", Some("refresh packages"))),
            "the pane must be told to ask for the password"
        );
        assert!(!agent.needs_reply);

        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "Reading package lists...\n__WASSH_STATUS:0__\n__WASSH_PWD:/__\n"
                    .to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.submit_sudo("hunter2", &mut session);
        }
        assert!(agent.sudo_prompt().is_none(), "the prompt must close");
        let sent = &fake.commands.last().expect("the command did not run").1;
        assert!(sent.contains("sudo -S -p '' -v"), "{sent}");
        assert!(sent.contains("apt-get update"), "{sent}");
        assert!(
            !sent.contains("hunter2"),
            "the password must not reach the command line"
        );
        assert_eq!(fake.stdin.last().map(String::as_str), Some("hunter2"));
    }

    #[test]
    fn a_command_that_wanted_root_is_answered_with_the_run_sudo_hint() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["du*".to_owned()];
        let mut fake = FakeSession::default();
        fake.answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: String::new(),
                stderr: "sudo: a password is required\n".to_owned(),
                code: Some(1),
            },
        );
        let output = run_tool(
            &mut agent,
            &mut fake,
            TOOL_RUN_COMMAND,
            r#"{"command":"du -sh /root"}"#,
        );
        assert!(
            output.contains(SUDO_HINT),
            "the model was not told: {output}"
        );
        // A failure that has nothing to do with root is left alone.
        assert!(!needs_sudo_help("cat: missing: No such file or directory"));
    }

    #[test]
    fn cancelling_the_sudo_prompt_reports_that_nothing_ran() {
        let mut agent = Agent::new();
        let mut fake = FakeSession::default();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_SUDO.to_owned(),
                    arguments: r#"{"command":"systemctl restart nginx"}"#.to_owned(),
                },
                &mut session,
            );
        }
        assert!(agent.sudo_prompt().is_some());

        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.cancel_sudo(&mut session);
        }
        assert!(agent.sudo_prompt().is_none());
        assert!(fake.commands.is_empty(), "a cancelled command must not run");
        match agent.turns.last() {
            Some(Turn::Tool { output, failed, .. }) => {
                assert!(output.contains("did not run"), "{output}");
                assert!(*failed);
            }
            other => panic!("expected a tool turn, got {other:?}"),
        }
        assert!(
            agent.needs_reply,
            "the model has to be told the tool failed"
        );
    }

    #[test]
    fn a_sudo_command_reads_its_password_from_stdin() {
        let wrapped = wrap_command_with_sudo("/srv", "apt-get update");
        assert!(wrapped.contains("IFS= read -r __wassh_sudo_pw"));
        assert!(wrapped.contains("sudo -S -p '' -v"));
        // A bare command is run as root; one that already calls sudo is left alone.
        assert!(wrapped.contains("sudo -n apt-get update"), "{wrapped}");
        assert!(!wrapped.contains("sudo -n sudo"));
        assert!(
            wrap_command_with_sudo("/srv", "sudo du -sh /root").contains("\n  sudo du -sh /root\n")
        );
        assert!(wrapped.contains(SUDO_AUTH_FAILED_MARKER));
        // The password itself is never part of the command line.
        assert!(!wrapped.contains("hunter2"));
    }

    #[test]
    fn a_run_command_that_calls_sudo_asks_for_the_password() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["sudo*".to_owned()];
        let mut fake = FakeSession::default();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_COMMAND.to_owned(),
                    arguments: r#"{"command":"sudo du -xhd1 /"}"#.to_owned(),
                },
                &mut session,
            );
        }
        assert!(fake.commands.is_empty(), "sudo ran without a password");
        assert_eq!(agent.sudo_prompt(), Some(("sudo du -xhd1 /", None)));

        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.submit_sudo("hunter2", &mut session);
        }
        let sent = &fake.commands.last().expect("the command did not run").1;
        assert!(sent.contains("sudo -S -p '' -v"), "{sent}");
        assert!(sent.contains("\n  sudo du -xhd1 /\n"), "{sent}");
        assert_eq!(fake.stdin.last().map(String::as_str), Some("hunter2"));
    }

    #[test]
    fn a_command_without_sudo_does_not_ask_for_a_password() {
        let mut agent = Agent::new();
        agent.app_allow = vec!["du*".to_owned()];
        let mut fake = FakeSession::default();
        {
            let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
            agent.begin_tool(
                ApiToolCall {
                    id: "c1".to_owned(),
                    name: TOOL_RUN_COMMAND.to_owned(),
                    arguments: r#"{"command":"du -sh /tmp"}"#.to_owned(),
                },
                &mut session,
            );
        }
        assert!(agent.sudo_prompt().is_none(), "no password is needed here");
        assert_eq!(fake.commands.len(), 1, "the command runs straight away");
    }

    #[test]
    fn a_spawn_starts_a_background_subagent_that_inherits_the_conversation() {
        let mut agent = Agent::new();
        agent.scopes.load(super::scopes::find("web").unwrap());
        agent.app_allow = vec!["uptime".to_owned()];
        agent.session_allow = vec!["systemctl status*".to_owned()];
        let mut fake = FakeSession::default();
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_SPAWN_SUBAGENT.to_owned(),
                arguments: r#"{"title":"check the logs","prompt":"Find the failing unit."}"#
                    .to_owned(),
            },
            &mut session,
        );

        // The agent cannot build a provider request, so it hands the pane the spawn.
        let spawn = agent.take_spawn_request().expect("a spawn");
        assert_eq!(spawn.title, "check the logs");
        let index = agent.begin_subagent(&spawn.title, &spawn.prompt);
        let sub = &agent.subagents[index];
        assert!(sub.agent.subagent, "it must not spawn anything of its own");
        assert!(sub.agent.scopes.contains("web"), "the loadout is inherited");
        assert_eq!(sub.agent.app_allow, ["uptime"]);
        assert_eq!(sub.agent.session_allow, ["systemctl status*"]);
        assert_eq!(sub.agent.cwd, agent.cwd);
        assert_eq!(sub.agent.keys.exec, format!("{EXEC_KEY}#sub-1"));
        assert_ne!(
            sub.agent.keys.exec, agent.keys.exec,
            "two agents may run commands at once"
        );
        assert!(sub.running());
    }

    #[test]
    fn spawning_stops_at_the_limit_and_a_subagent_cannot_spawn() {
        let mut agent = Agent::new();
        agent.max_subagents = 1;
        let first = agent.begin_subagent("one", "first task");
        // A stream still open keeps the first one running.
        let (_sender, events) = std::sync::mpsc::channel();
        agent.subagents[first].agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        agent.subagents[first].agent.phase = Some(Phase::Streaming { turn: 1, events });
        assert_eq!(agent.subagents_running(), 1);
        let output = run_tool(
            &mut agent,
            &mut FakeSession::default(),
            TOOL_SPAWN_SUBAGENT,
            r#"{"prompt":"second task"}"#,
        );
        assert!(output.contains("limit"), "{output}");

        let mut child = Agent::new();
        child.subagent = true;
        let output = run_tool(
            &mut child,
            &mut FakeSession::default(),
            TOOL_SPAWN_SUBAGENT,
            r#"{"prompt":"deeper"}"#,
        );
        assert_eq!(output, NO_NESTED_SUBAGENTS);
    }

    #[test]
    fn waiting_for_a_subagent_answers_with_its_result() {
        let mut agent = Agent::new();
        let index = agent.begin_subagent("check the logs", "Find the failing unit.");
        agent.subagents[index].agent.turns.push(Turn::Assistant {
            text: "api.service is failing".to_owned(),
            stopped: false,
        });
        agent.subagents[index].end = Some(SubagentEnd::Done);
        let mut fake = FakeSession::default();
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut fake);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_WAIT_SUBAGENT.to_owned(),
                arguments: r#"{"id":"sub-1"}"#.to_owned(),
            },
            &mut session,
        );
        agent.poll(&mut session);
        match agent
            .turns
            .iter()
            .rev()
            .find(|turn| matches!(turn, Turn::Tool { .. }))
        {
            Some(Turn::Tool { output, failed, .. }) => {
                assert!(!*failed);
                assert!(output.contains("api.service is failing"), "{output}");
            }
            other => panic!("expected a tool turn, got {other:?}"),
        }
    }

    #[test]
    fn a_finished_subagent_lands_in_the_transcript_and_the_history() {
        let mut agent = Agent::new();
        let index = agent.begin_subagent("check the logs", "Find the failing unit.");
        let (sender, events) = std::sync::mpsc::channel();
        agent.subagents[index].agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        agent.subagents[index].agent.phase = Some(Phase::Streaming { turn: 1, events });
        sender
            .send(StreamEvent::Done(Completion {
                text: "api.service is failing".to_owned(),
                ..Completion::default()
            }))
            .expect("send");
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.poll(&mut session);

        assert_eq!(agent.subagents[index].state(), SubagentState::Done);
        assert!(
            agent.turns.iter().any(
                |turn| matches!(turn, Turn::Notice(text) if text.contains("api.service is failing"))
            ),
            "{:?}",
            agent.turns
        );
        // The answer joins the history once the conversation is between turns.
        agent.poll(&mut session);
        assert!(
            matches!(agent.history.last(), Some(ApiMessage::User { content, .. }) if content.contains("api.service is failing")),
            "{:?}",
            agent.history
        );
    }

    #[test]
    fn a_running_subagent_follows_the_permissions_the_parent_gains() {
        let mut agent = Agent::new();
        let index = agent.begin_subagent("check", "do it");
        // A stream still open keeps it running across the poll.
        let (_sender, events) = std::sync::mpsc::channel();
        agent.subagents[index].agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        agent.subagents[index].agent.phase = Some(Phase::Streaming { turn: 1, events });
        agent.session_allow = vec!["systemctl restart*".to_owned()];
        agent.allow_all = true;
        let mut session: Option<&mut dyn SessionAccess> = None;
        agent.poll(&mut session);
        assert_eq!(
            agent.subagents[index].agent.session_allow,
            ["systemctl restart*"]
        );
        assert!(agent.subagents[index].agent.allow_all);
    }

    #[test]
    fn the_summary_prompt_carries_the_transcript() {
        let mut agent = Agent::new();
        agent.turns.push(Turn::User {
            text: "restart nginx".to_owned(),
            meta: UserMeta::default(),
        });
        agent.turns.push(Turn::Assistant {
            text: "done".to_owned(),
            stopped: false,
        });
        let text = agent.transcript_text();
        assert!(text.contains("User: restart nginx"));
        assert!(text.contains("Assistant: done"));
        assert!(SUMMARY_SYSTEM_PROMPT.contains("Goal"));
    }

    /// The app's connections, with one saved host and a session per handle.
    #[derive(Default)]
    struct FakeRemote {
        hosts: Vec<RemoteHost>,
        states: std::collections::HashMap<String, RemoteState>,
        sessions: std::collections::HashMap<String, FakeSession>,
        opened: Vec<String>,
        passwords: Vec<String>,
    }

    impl FakeRemote {
        /// A broker holding one host, in `state`, with its session ready to answer.
        fn with_host(id: &str, name: &str, state: RemoteState) -> Rc<RefCell<Self>> {
            let mut remote = Self::default();
            remote.hosts.push(RemoteHost {
                id: id.to_owned(),
                name: name.to_owned(),
                address: "deploy@10.0.0.4:22".to_owned(),
                tags: vec!["prod".to_owned()],
                problem: None,
            });
            remote.states.insert(host_handle(id), state);
            remote.sessions.insert(host_handle(id), FakeSession::default());
            Rc::new(RefCell::new(remote))
        }

        /// Hand the conversation the broker, as the pane does every frame.
        fn attach(agent: &mut Agent, remote: &Rc<RefCell<Self>>) {
            let broker: Rc<RefCell<dyn RemoteHosts>> = remote.clone();
            agent.set_remote(Some(broker));
        }

        /// Let the connection finish opening.
        fn open(&mut self, id: &str) {
            self.states
                .insert(host_handle(id), RemoteState::Open { environment: "a test box".to_owned() });
        }

        /// The session commands on `id` run through.
        fn session_mut(&mut self, id: &str) -> &mut FakeSession {
            self.sessions.get_mut(&host_handle(id)).expect("a session")
        }
    }

    impl RemoteHosts for FakeRemote {
        fn hosts(&self) -> &[RemoteHost] {
            &self.hosts
        }
        fn connect(&mut self, handle: &str, host_id: &str) {
            self.opened.push(host_id.to_owned());
            self.states.insert(handle.to_owned(), RemoteState::Connecting);
        }
        fn state(&self, handle: &str) -> RemoteState {
            self.states.get(handle).cloned().unwrap_or_default()
        }
        fn supply_password(&mut self, handle: &str, password: &str) {
            self.passwords.push(password.to_owned());
            self.states.insert(
                handle.to_owned(),
                RemoteState::Open { environment: "a test box".to_owned() },
            );
        }
        fn session(&mut self, handle: &str) -> Option<&mut dyn SessionAccess> {
            self.sessions
                .get_mut(handle)
                .map(|session| session as &mut dyn SessionAccess)
        }
        fn close(&mut self, handle: &str) {
            self.states.remove(handle);
        }
    }

    /// A conversation with one saved host, ready to be asked to work on it.
    fn with_saved_host(state: RemoteState) -> (Agent, Rc<RefCell<FakeRemote>>) {
        let remote = FakeRemote::with_host("db-1", "db-1", state);
        let mut agent = Agent::new();
        agent.allow_all = true;
        FakeRemote::attach(&mut agent, &remote);
        (agent, remote)
    }

    #[test]
    fn hosts_list_names_the_saved_hosts_and_how_to_use_them() {
        let (mut agent, _remote) = with_saved_host(RemoteState::Idle);
        let mut tab = FakeSession::default();
        let output = run_tool(&mut agent, &mut tab, TOOL_HOSTS_LIST, "{}");
        assert!(output.contains("- db-1 — db-1 (deploy@10.0.0.4:22)"), "{output}");
        assert!(output.contains("tags: prod"), "{output}");
        assert!(output.contains("Pass a host's id as `host`"), "{output}");
        assert!(!output.contains("password"), "{output}");
    }

    #[test]
    fn a_call_on_another_host_asks_before_it_connects() {
        let (mut agent, remote) = with_saved_host(RemoteState::Idle);
        // The command itself is allowed; the connection to another machine is what asks.
        agent.allow_all = false;
        agent.app_allow = vec!["uname*".to_owned()];
        let mut tab = FakeSession::default();
        when_offered(&mut agent, TOOL_RUN_COMMAND);
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut tab);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"uname -a","host":"db-1"}"#.to_owned(),
            },
            &mut session,
        );
        let Some((HostCard::Ask, host)) = agent.host_card() else {
            panic!("the card does not ask: {}", agent.host_card().is_some());
        };
        assert_eq!(host.name, "db-1");
        assert!(remote.borrow().opened.is_empty(), "it connected before asking");

        agent.answer_connect(true, &mut session);
        assert_eq!(remote.borrow().opened, ["db-1"]);
        assert!(!agent.host_card().is_some_and(|(card, _)| card == HostCard::Ask));

        // While the connection is opening the call waits.
        let handle = host_handle("db-1");
        agent.poll(&mut session);
        assert!(matches!(agent.phase, Some(Phase::Remote { .. })));
        remote.borrow_mut().open("db-1");
        remote.borrow_mut().session_mut("db-1").answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "Linux db-1\n__WASSH_STATUS:0__\n__WASSH_PWD:/root__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        // The call runs once the other host answers, and its output comes back through
        // that host's own session a frame later.
        agent.poll(&mut session);
        agent.poll(&mut session);
        assert!(tab.commands.is_empty(), "the command ran on this tab's host");
        let sent = remote.borrow().sessions[&handle].commands.clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].1.contains("uname -a"), "{sent:?}");
        assert!(
            matches!(
                agent.turns.last(),
                Some(Turn::Tool { output, .. }) if output.contains("Linux db-1")
            ),
            "{:?}",
            agent.turns.last()
        );
    }

    #[test]
    fn a_denied_connection_is_reported_without_connecting() {
        let (mut agent, remote) = with_saved_host(RemoteState::Idle);
        agent.allow_all = false;
        let mut tab = FakeSession::default();
        when_offered(&mut agent, TOOL_RUN_COMMAND);
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut tab);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"hostname","host":"db-1"}"#.to_owned(),
            },
            &mut session,
        );
        agent.answer_connect(false, &mut session);
        assert!(remote.borrow().opened.is_empty());
        assert!(matches!(
            agent.history.last(),
            Some(ApiMessage::Tool { content, .. })
                if content.contains("did not allow this chat to connect to db-1")
        ));
    }

    #[test]
    fn an_unknown_host_is_refused_with_the_list_to_call() {
        let (mut agent, _remote) = with_saved_host(RemoteState::Idle);
        let mut tab = FakeSession::default();
        when_offered(&mut agent, TOOL_RUN_COMMAND);
        let output = run_tool(
            &mut agent,
            &mut tab,
            TOOL_RUN_COMMAND,
            r#"{"command":"hostname","host":"web-9"}"#,
        );
        assert!(output.contains("no saved host `web-9`"), "{output}");
        assert!(output.contains(TOOL_HOSTS_LIST), "{output}");
        assert!(tab.commands.is_empty());
    }

    #[test]
    fn a_tool_that_cannot_run_elsewhere_says_which_ones_can() {
        let (mut agent, _remote) = with_saved_host(RemoteState::Idle);
        when_offered(&mut agent, TOOL_REMOTE_EDIT);
        let mut tab = FakeSession::default();
        tab.contents.insert("/srv/app/main.rs".to_owned(), "fn main() {}\n".to_owned());
        let output = run_tool(
            &mut agent,
            &mut tab,
            TOOL_REMOTE_EDIT,
            r#"{"path":"/srv/app/main.rs","oldText":"main","newText":"start","host":"db-1"}"#,
        );
        assert!(output.contains("runs on this tab's host only"), "{output}");
        assert!(output.contains(TOOL_RUN_COMMAND), "{output}");
    }

    #[test]
    fn a_remembered_denial_refuses_the_connection_without_a_card() {
        let (mut agent, remote) = with_saved_host(RemoteState::Idle);
        agent.allow_all = false;
        agent.reach_deny = vec!["db-1".to_owned()];
        let mut tab = FakeSession::default();
        let output = run_tool(
            &mut agent,
            &mut tab,
            TOOL_RUN_COMMAND,
            r#"{"command":"hostname","host":"db-1"}"#,
        );
        assert!(output.contains("do not let this chat connect to db-1"), "{output}");
        assert!(agent.host_card().is_none(), "the card asked anyway");
        assert!(remote.borrow().opened.is_empty());
    }

    #[test]
    fn a_remembered_allow_connects_without_a_card() {
        let (mut agent, remote) = with_saved_host(RemoteState::Idle);
        agent.allow_all = false;
        agent.reach_allow = vec!["db-1".to_owned()];
        let mut tab = FakeSession::default();
        when_offered(&mut agent, TOOL_RUN_COMMAND);
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut tab);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"hostname","host":"db-1"}"#.to_owned(),
            },
            &mut session,
        );
        assert_eq!(
            agent.host_card().map(|(card, _)| card),
            Some(HostCard::Connecting),
            "the card asked anyway"
        );
        assert_eq!(remote.borrow().opened, ["db-1"]);
    }

    #[test]
    fn a_host_without_a_stored_password_asks_for_one() {
        let (mut agent, remote) = with_saved_host(RemoteState::NeedsPassword);
        let mut tab = FakeSession::default();
        when_offered(&mut agent, TOOL_RUN_COMMAND);
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut tab);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"hostname","host":"db-1"}"#.to_owned(),
            },
            &mut session,
        );
        let Some((HostCard::Password, host)) = agent.host_card() else {
            panic!("the card does not ask for a password: {}", agent.host_card().is_some());
        };
        assert_eq!(host.id, "db-1");

        remote.borrow_mut().session_mut("db-1").answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "db-1\n__WASSH_STATUS:0__\n__WASSH_PWD:/home/deploy__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        agent.supply_host_password("hunter2");
        assert_eq!(remote.borrow().passwords, ["hunter2"]);
        agent.poll(&mut session);
        agent.poll(&mut session);
        assert!(tab.commands.is_empty(), "the command ran on this tab's host");
        let sent = remote.borrow().sessions[&host_handle("db-1")].commands.clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
    }

    #[test]
    fn a_file_written_on_another_host_lands_there() {
        let (mut agent, remote) =
            with_saved_host(RemoteState::Open { environment: "a test box".to_owned() });
        agent.allow_all = true;
        let mut tab = FakeSession::default();
        when_offered(&mut agent, TOOL_REMOTE_WRITE);
        remote.borrow_mut().session_mut("db-1").answers.insert(
            PROBE_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "no\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let mut session: Option<&mut dyn SessionAccess> = Some(&mut tab);
        agent.begin_tool(
            ApiToolCall {
                id: "c1".to_owned(),
                name: TOOL_REMOTE_WRITE.to_owned(),
                arguments: r#"{"path":"/srv/app/main.rs","content":"fn main() {}\n","host":"db-1"}"#
                    .to_owned(),
            },
            &mut session,
        );
        // The probe answers on the host, then the upload finishes there.
        agent.poll(&mut session);
        agent.poll(&mut session);
        assert!(tab.writes.is_empty(), "the file was written on this tab's host");
        let written = remote.borrow().sessions[&host_handle("db-1")].writes.clone();
        assert_eq!(
            written,
            [(
                "/srv/app/main.rs".to_owned(),
                b"fn main() {}\n".to_vec()
            )]
        );
    }

    #[test]
    fn the_working_directory_is_kept_per_host() {
        let (mut agent, remote) = with_saved_host(RemoteState::Open { environment: "a test box".to_owned() });
        agent.allow_all = true;
        let mut tab = FakeSession::default();
        when_offered(&mut agent, TOOL_RUN_COMMAND);
        remote.borrow_mut().session_mut("db-1").answers.insert(
            EXEC_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: "/srv/app\n__WASSH_STATUS:0__\n__WASSH_PWD:/srv/app__\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        let output = run_tool(
            &mut agent,
            &mut tab,
            TOOL_RUN_COMMAND,
            r#"{"command":"cd /srv/app","host":"db-1"}"#,
        );
        assert!(output.contains("Working directory is now /srv/app"), "{output}");
        assert_eq!(agent.cwd, "/", "another host moved this tab's directory");

        // A relative path on that host resolves against its directory, not this tab's.
        remote.borrow_mut().session_mut("db-1").contents.insert(
            "/srv/app/main.rs".to_owned(),
            "fn main() {}\n".to_owned(),
        );
        let read = run_tool(
            &mut agent,
            &mut tab,
            TOOL_REMOTE_READ,
            r#"{"path":"main.rs","host":"db-1"}"#,
        );
        assert!(read.contains("fn main() {}"), "{read}");
    }
}
