//! The AI agent pane.
//!
//! A chat with an LLM that can act on the session: run commands, read and write
//! files, drive the terminal, search the web or the knowledge base. Provider and
//! tool work runs off the UI thread, so typing, scrolling and Stop stay
//! responsive while a turn is in flight.

use std::sync::mpsc::Receiver;

use egui::Ui;
use serde_json::{Value, json};
use wassh_core::{Palette, RADIUS, SessionStatus};
use wassh_core::HeaderHealth;
use wassh_ui::widgets::{
    Button, ButtonKind, COPY_BUTTON_SIZE, COPY_FEEDBACK_MS, copy_button, password_input,
    select_width, small_button, small_button_enabled, text_area, text_area_with_id, ToastKind,
};

use super::agent::{
    Agent, AgentQuestion, Phase, SUMMARY_SYSTEM_PROMPT, Subagent, SubagentEnd, SubagentState,
    ToolSettings, Turn, UserMeta, Waiting, announce_wait, end_subagent, history_from_json, history_to_json,
    subagents_from_json, subagents_to_json, turns_from_json, turns_to_json,
};
use super::markdown::{Block, Ink};
use super::permissions::{GROUP_KEY_PREFIX, METHOD_KEY_PREFIX, Permissions, setting_key};
use super::protocol::{
    CONVERSATIONS_PER_HOST_MAX, DATA_VERSION, DEFAULT_CHAT_TITLE, DEFAULT_MAX_SUBAGENTS,
    ImagePart, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENT_CHARS, MAX_ATTACHMENTS, MAX_IMAGE_BYTES,
    MAX_IMAGES, Protocol, ProviderConfig, SAFE_RULES, SETTING_AUTO_COMPACT,
    SETTING_DEFAULT_ALLOW_RULES, SETTING_DEFAULT_DENY_RULES, SETTING_GLOBAL_PROMPT,
    SETTING_HOST_ALLOW_RULES, SETTING_HOST_DENY_RULES, SETTING_HOST_PROMPT, SETTING_HOST_RAG_FOLDER,
    SETTING_HOST_SCRIPT_MODE, SETTING_MAX_SUBAGENTS, SETTING_MODEL_CONTEXTS, SETTING_RAG_FOLDER,
    SETTING_RAG_MODEL, SETTING_RAG_PROVIDER, SETTING_SCRIPT_MODE, SETTING_WEB_SEARCH_KEY,
    SETTING_WEB_SEARCH_PROVIDER, TITLE_MAX_CHARS, image_media_type,
};
use super::providers::{CompletionRequest, EmbedConfig, complete_blocking, provider_models};
use super::rules::ApprovalScope;
use super::scopes::{self, Loaded};
use super::settings_ui::{PROVIDERS_KEY, provider_secret_field};
use super::tools;
use wassh_ui::to_color32;
use crate::plugin::{Plugin, PluginContext};
use crate::{PluginId, PluginManifest};

/// Key the conversation is stored under.
const CONVERSATION_KEY: &str = "conversation";
/// Key the host's environment map is stored under.
const MAP_KEY: &str = "envMap";
/// Key the host's ports and services are stored under.
const SYSTEM_KEY: &str = "envSystem";
/// The scope that holds the environment map.
const ENV_SCOPE: &str = "env";
/// User-facing name for the environment-map scope.
const GRAPH_SCOPE_LABEL: &str = "Graph";
/// Settings [`AiAgent::apply_settings`] derives the agent's rules and tools from, besides
/// the per-group and per-tool permission keys it reads through [`Permissions`].
const DERIVED_SETTINGS: &[&str] = &[
    SETTING_DEFAULT_ALLOW_RULES,
    SETTING_DEFAULT_DENY_RULES,
    SETTING_HOST_ALLOW_RULES,
    SETTING_HOST_DENY_RULES,
    SETTING_RAG_FOLDER,
    SETTING_RAG_PROVIDER,
    SETTING_RAG_MODEL,
    SETTING_HOST_RAG_FOLDER,
    SETTING_WEB_SEARCH_PROVIDER,
    SETTING_MAX_SUBAGENTS,
    SETTING_SCRIPT_MODE,
    SETTING_HOST_SCRIPT_MODE,
];
/// Tokens a reply may use.
const MAX_TOKENS: u32 = 4096;
/// Characters a token is estimated at when sizing the conversation.
const CHARS_PER_TOKEN: usize = 4;
/// Tokens the system prompt and the tool schemas take out of the window, until a request has
/// been measured.
const PROMPT_OVERHEAD_TOKENS: usize = 4_000;
/// Context window of a model the settings do not name.
const DEFAULT_CONTEXT_WINDOW_TOKENS: usize = 128_000;
/// Messages that stay when the conversation is compacted.
const COMPACT_KEEP_MESSAGES: usize = 6;
/// Where the compacted part of the conversation sits in the system prompt.
const COMPACTED_HEADING: &str = "Earlier conversation, compacted:";
/// Heading of the environment map's digest in the system prompt.
const MAP_HEADING: &str = "Environment map (query it with env_find, env_search and env_outline; \
refresh or extend it with env_scan):";

/// What the mapping sub-agent is asked, with the root and the file budget filled in.
const MAP_TASK: &str = "\
Map the project rooted at {root} for the other chats on this host.

Work in this order:
1. Call env_overview with no arguments to see the tree the environment map already holds.
2. Choose the files that explain the most: entry points, build and configuration files, and the
   modules the rest of the tree depends on. Skip vendored, generated and lock files, and stay
   inside {root}.
3. Read the files you chose — load_scope first if the file tools are not loaded — and note what
   each one is for and what it imports, calls or feeds. Describe at most {files} of them.
4. Answer with one JSON object and nothing else. Never answer with prose and never ask a
   question: when the budget is spent, or you have nothing worth adding, answer with
   {\"files\": []} and nothing else.

The object has this shape:
{\"files\": [{\"path\": \"<absolute or root-relative path>\", \"summary\": \"<what the file is \
for, at most 40 words>\", \"connects\": [\"<path, module or service it imports, calls or \
feeds>\"]}]}";

/// What a mapping sub-agent is told when its answer carried no file list.
const MAP_REPAIR_TASK: &str = "\
Your answer did not carry the JSON object, so it was thrown away. Read the files you chose if you \
have not read them yet, then answer with one JSON object and nothing else — no prose, no \
questions. An empty list is a fine answer: {\"files\": []}";

/// Answers one mapping pass accepts before it gives up on the shape of them.
const MAP_ATTEMPTS: u8 = 2;
/// What the transcript says when a mapping pass answered without describing anything.
const MAP_EMPTY_NOTICE: &str = "The mapping pass described no files.";
/// What the transcript says while it asks the model for the object again.
const MAP_RETRY_NOTICE: &str = "The mapping answer had no file list; asking again.";

/// Name the chat list gives the mapping sub-agent.
const MAP_SUBAGENT_TITLE: &str = "map the project";
/// What the transcript says when a mapping pass lands.
const MAP_READY_NOTICE: &str = "Map ready: {described} files described under {root}.";
/// Call labels the popup lists while a mapping pass runs.
const MAP_DETAIL_LINES: usize = 5;
/// Milliseconds in a second, for a mapping pass's elapsed time.
const MILLIS_PER_SECOND: u64 = 1_000;
/// Seconds in a minute, for the same.
const SECONDS_PER_MINUTE: u64 = 60;
/// Heading of the host's ports and services in the system prompt.
const SYSTEM_HEADING: &str = "Host setup (from env_system):";
/// Transcript note left where the conversation was compacted.
const COMPACTED_NOTICE: &str = "Earlier messages were compacted into a summary:";
/// Gap between the provider and model dropdowns under the composer.
const FOOTER_GAP: f32 = 6.0;/// Type size of the two dropdowns.
const FOOTER_TEXT_SIZE: f32 = 11.0;
/// Labels of the two modes the footer switches between.
const MODE_PLAN_LABEL: &str = "Plan";
/// The other mode's label.
const MODE_AGENT_LABEL: &str = "Agent";
/// What Plan mode means, on the mode button.
const MODE_TO_AGENT_HINT: &str = "Plan mode: every tool only reads. Click for Agent mode, where \
the agent may change files, run commands and act on the host.";
/// What Agent mode means, on the mode button.
const MODE_TO_PLAN_HINT: &str = "Agent mode: the agent may change files, run commands and act \
on the host. Click for Plan mode, where it only reads.";
/// Room two dropdowns need before the mode and Compact buttons are drawn at all.
const MIN_SELECT_ROOM: f32 = 80.0;
/// Tooltip of the Compact button.
const COMPACT_HINT: &str = "Fold the older part of this chat into a summary now.";
/// Caption of the Compact button.
const COMPACT_LABEL: &str = "Compact";
/// Said when Compact is pressed while a run is in flight.
const COMPACT_BUSY: &str = "Stop the run before compacting the chat.";
/// Said when there is nothing old enough to fold.
const COMPACT_EMPTY: &str = "Nothing to compact yet: the chat is as short as it gets.";
/// Narrowest the composer text area gets.
const MIN_COMPOSER_WIDTH: f32 = 80.0;
/// Gap between two entries of the transcript.
const TURN_GAP: f32 = 8.0;
/// Share of the pane a chat message may take.
const MESSAGE_BUBBLE_SHARE: f32 = 0.92;
/// Transparency of the context tags on the accent-filled user bubble.
const USER_BUBBLE_TAG_ALPHA: u8 = 200;
/// Lines the prompt editor opens with at least.
const EDITOR_MIN_ROWS: usize = 2;
/// Lines the prompt editor grows to before it starts scrolling.
const EDITOR_MAX_ROWS: usize = 10;
/// Keys the prompt editor answers to, under the field.
const EDITOR_HINT: &str = "Enter to send · Esc to cancel";
/// Why an earlier prompt cannot be edited just now.
const EDIT_WHILE_BUSY: &str = "Stop the current run before editing an earlier message.";
/// Share of a tool badge's colour mixed into its pill.
const BADGE_TINT: f32 = 0.25;
/// Characters kept from one tool's output.
const TOOL_OUTPUT_MAX_CHARS: usize = 4_000;
/// Badge on a `run_script` card.
const SCRIPT_BADGE: &str = "Script";
/// Header of the program in a script card.
const SCRIPT_CODE_LABEL: &str = "JavaScript";
/// Header of what the script returned.
const SCRIPT_RESULT_LABEL: &str = "Result";
/// Header of what the script logged.
const SCRIPT_CONSOLE_LABEL: &str = "Console";
/// Marks a call the script made that worked.
const SCRIPT_OK_MARK: &str = "✓";
/// Marks a call the script made that failed.
const SCRIPT_FAILED_MARK: &str = "✕";
/// Marks a call the script made that has not answered yet.
const SCRIPT_PENDING_MARK: &str = "…";
/// Size of the program, the calls and the result in a script card.
const SCRIPT_TEXT_SIZE: f32 = 10.5;
/// Widest the output text gets before it has to shrink for its copy button.
const MIN_OUTPUT_WIDTH: f32 = 60.0;
/// What a card with nothing to show says instead of an empty box.
const NO_OUTPUT: &str = "(no output)";
/// Prompt marker the shell tools put in front of the command they ran.
const COMMAND_PROMPT: &str = "$ ";
/// Marker a Local-PC command adds in front of that prompt.
const LOCAL_COMMAND_PREFIX: &str = "local ";
/// Characters shown from a queued message.
const QUEUE_PREVIEW_MAX_CHARS: usize = 120;
const QUESTION_INDEX_OFFSET: usize = 1;
/// Most a prompt card grows to, its own controls included.
const PROMPT_CARD_MAX_HEIGHT: f32 = 480.0;
/// Smallest a capped prompt card gets.
const PROMPT_CARD_MIN_HEIGHT: f32 = 260.0;
/// Share of the pane a prompt card may take.
const PROMPT_CARD_SHARE: f32 = 0.60;
/// Gap between a card's scrolling body and its fixed controls.
const PROMPT_CARD_GAP: f32 = 8.0;
/// Narrowest an answer field in a prompt card may get.
const MIN_ANSWER_WIDTH: f32 = 80.0;
/// Characters of a command kept in a prompt card.
const PROMPT_COMMAND_MAX_CHARS: usize = 400;
/// Characters of a command kept in an "always allow" button's label.
const SCOPE_LABEL_MAX_CHARS: usize = 56;
/// What a loaded scope's tag says when hovered.
const SCOPE_CHIP_HINT: &str = "Scope loaded. Click for its options";
/// Caption of the row of loaded scopes over the composer.
const SCOPE_LABEL: &str = "Loaded scopes:";
/// Type size of a loaded scope's tag and of its caption.
const SCOPE_TAG_TEXT_SIZE: f32 = 10.0;
/// Height of a loaded scope's tag.
const SCOPE_TAG_HEIGHT: f32 = 16.0;
/// Side padding inside a loaded scope's tag.
const SCOPE_TAG_PADDING: f32 = 6.0;
/// Width of the popup a tag opens.
const SCOPE_POPUP_WIDTH: f32 = 260.0;
/// Padding inside that popup.
const SCOPE_POPUP_PADDING: i8 = 10;
/// Gap between the popup's lines.
const SCOPE_POPUP_GAP: f32 = 6.0;
/// Padding inside a prompt card.
const PROMPT_CARD_PADDING: i8 = 10;
/// Side the button that leaves a chat for the list is drawn at.
const BACK_BUTTON_SIZE: f32 = 26.0;
/// Half the arrow's shaft, from the centre of its button.
const BACK_ARROW_SHAFT: f32 = 7.5;
/// Length of each side of the arrow's head.
const BACK_ARROW_HEAD: f32 = 4.5;
/// Thickness of the arrow.
const BACK_ARROW_WIDTH: f32 = 1.8;
/// Heading of the chat list.
const CHAT_LIST_HEADING: &str = "Chats";
/// Label of the header button that leaves the open chat and starts an empty one.
const NEW_CHAT_LABEL: &str = "New chat";
/// What the chat list says when there is nothing to reopen.
const CHAT_LIST_EMPTY: &str = "No previous chats yet.";
/// What the pane says while a sub-agent's transcript is on screen.
const SUBAGENT_VIEW_NOTICE: &str = "Read-only: this sub-agent works on its own. Answers it needs are asked here; Back returns to the chat.";
/// Type size of a chat's name in the list and of the open chat's title.
const CHAT_TITLE_TEXT_SIZE: f32 = 12.0;
/// Side padding inside a chat list row.
const CHAT_ROW_PADDING: f32 = 10.0;
/// System instruction for the short title generated from the first prompt.
const CHAT_TITLE_SYSTEM_PROMPT: &str =
    "Create a short digest title for the user's prompt. Reply with 3 to 8 words only. \
No quotes, punctuation, prefix, or explanation.";

/// How long one dot of the animated "Thinking…" stays on.
const THINKING_DOT_MS: u64 = 400;
/// Dots cycled while the model has not answered yet.
const THINKING_DOT_COUNT: usize = 3;
/// Caption of the file-attach button.
const ATTACH_GLYPH: &str = "📎";
/// Caption of the terminal-context toggle.
///
/// ASCII on purpose: the bundled fonts have no glyph for a symbol like "⌁", so it drew as
/// an empty box next to the paperclip.
const TERMINAL_ATTACH_GLYPH: &str = ">_";
/// Characters of terminal output attached to a message as context.
const TERMINAL_CONTEXT_CHARS: usize = 6_000;
/// Heading of that block in the outgoing message.
const TERMINAL_CONTEXT_MARKER: &str = "[Recent terminal output]";
/// Text used when the terminal has shown nothing yet.
const TERMINAL_CONTEXT_EMPTY: &str = "[No terminal output captured yet.]";
/// A slash command the composer understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    /// Word without the slash.
    pub name: &'static str,
    /// How it is spelled in the list.
    pub label: &'static str,
    /// One-line explanation.
    pub description: &'static str,
}

/// The command the Compact button runs.
const COMPACT_COMMAND: SlashCommand = SlashCommand {
    name: "compact",
    label: "/compact",
    description: "Fold the older part of this chat into a summary",
};

/// The command that puts the chat back in Plan mode.
const PLAN_COMMAND: SlashCommand = SlashCommand {
    name: "plan",
    label: "/plan",
    description: "Plan mode: the agent can only read, and proposes instead of changing",
};

/// The command that puts the chat back in Agent mode.
const AGENT_COMMAND: SlashCommand = SlashCommand {
    name: "agent",
    label: "/agent",
    description: "Agent mode: the agent may change files, run commands and act on the host",
};

/// The commands the reference offers.
pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "summary",
        label: "/summary",
        description: "Summarize this chat into context for a new chat",
    },
    SlashCommand {
        name: "context",
        label: "/context",
        description: "Same as /summary — carry-over context for a new chat",
    },
    SlashCommand {
        name: "new",
        label: "/new",
        description: "Start a new chat",
    },
    COMPACT_COMMAND,
    PLAN_COMMAND,
    AGENT_COMMAND,
    SlashCommand {
        name: "transfer",
        label: "/transfer",
        description: "Summarize this chat, start a new chat, and put the summary there",
    },
];

/// What a composer message means when it starts with a slash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashInput {
    /// Lowercase text typed after the slash.
    pub prefix: String,
    /// Commands whose name starts with `prefix`.
    pub matches: Vec<SlashCommand>,
    /// The command `prefix` spells out exactly.
    pub exact: Option<SlashCommand>,
}

/// Interpret composer text as a command, or `None` for an ordinary message.
pub fn parse_slash_input(raw: &str) -> Option<SlashInput> {
    let text = raw.trim();
    let prefix = text.strip_prefix('/')?.to_lowercase();
    if prefix.chars().any(char::is_whitespace) {
        return None;
    }
    let matches: Vec<SlashCommand> = SLASH_COMMANDS
        .iter()
        .copied()
        .filter(|command| command.name.starts_with(&prefix))
        .collect();
    let exact = SLASH_COMMANDS
        .iter()
        .copied()
        .find(|command| command.name == prefix);
    Some(SlashInput {
        prefix,
        matches,
        exact,
    })
}

/// One file the user attached to the next message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// Path on this machine.
    pub path: String,
    /// Its text, empty when the file is an image.
    pub text: String,
    /// The image, when the file is one the model can be shown.
    pub image: Option<ImagePart>,
}

impl Attachment {
    /// Name shown on the chip.
    fn name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }
}

/// What a summary worker is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SummaryMode {
    /// Show the summary here.
    Summary,
    /// Show it, then start a new chat carrying it.
    Transfer,
    /// Fold the conversation up to a point into it, and keep going in this chat.
    Compact,
}

/// A model-list fetch running on its own thread: the provider index and the worker.
type ModelFetch = (usize, Receiver<Result<Vec<String>, String>>);

/// A summary running on its own thread: what it is for, the history index it folds up to,
/// and the worker.
type SummaryJob = (SummaryMode, usize, Receiver<Result<String, String>>);

/// A digest of a chat's first prompt being written by the model.
type TitleJob = Receiver<Result<String, String>>;

/// A previous conversation, kept so the user can go back to it.
pub struct StoredChat {
    /// Digest of its first prompt, written by the model.
    pub title: String,
    /// When it was started, so the list keeps creation order however often a chat is
    /// reopened.
    pub created: u64,
    /// First prompt still waiting for a run to finish before its digest is asked for.
    title_prompt: Option<String>,
    /// Digest being written for this chat.
    title_job: Option<TitleJob>,
    /// The transcript.
    pub turns: Vec<Turn>,
    /// The provider history.
    pub history: Vec<super::protocol::ApiMessage>,
    /// Sub-agents it started, which may still be working.
    pub subagents: Vec<Subagent>,
    /// Working directory it ended in.
    pub cwd: String,
    /// Model it used.
    pub model: String,
    /// Provider id it used.
    pub provider: String,
    /// Summary of the part of it that was compacted away.
    pub compacted: Option<String>,
    /// Scopes it carried tools from.
    pub scopes: Loaded,
    /// Whether it was a Plan-mode chat, which only reads.
    pub plan: bool,
}

/// A message typed while a run was in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Queued {
    /// The prompt as typed, put back in the composer by "Edit".
    text: String,
    /// The message as it will be sent, with attachments and terminal context folded in.
    message: String,
    /// What travels with it, for the tags on its bubble.
    meta: UserMeta,
    /// Images the message carries.
    images: Vec<ImagePart>,
}

/// One turn's Markdown, with the fingerprint of the text it was parsed from.
///
/// Markdown parsing splits every line into styled runs and allocates for each of them, so
/// the result is kept in step with the transcript rather than rebuilt every frame.
#[derive(Default)]
struct ParsedTurn {
    fingerprint: u64,
    blocks: Vec<Block>,
}

/// What the user did with the sudo card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SudoAnswer {
    /// Run the command with the password they typed.
    Run,
    /// Leave the command unrun.
    Cancel,
}

/// The AI agent pane.
pub struct AiAgent {
    /// Conversation and tool loop.
    agent: Agent,
    /// Conversations set aside by "New chat", newest last.
    chats: Vec<StoredChat>,
    /// Providers the user can chat through.
    providers: Vec<ProviderConfig>,
    /// Index into [`AiAgent::providers`].
    provider: usize,
    /// Model used for the next message.
    model: String,
    /// API keys per provider id.
    keys: std::collections::BTreeMap<String, String>,
    /// What the user is typing.
    draft: String,
    /// Stable identity so conditional prompt cards cannot unset composer focus.
    composer_id: egui::Id,
    /// A message typed while a run was in flight, sent when it ends.
    queued: Option<Queued>,
    /// Files attached to the next message.
    attachments: Vec<Attachment>,
    /// Whether the next message carries the recent terminal output.
    attach_terminal: bool,
    /// A file picker running on its own thread.
    picking: Option<Receiver<Vec<String>>>,
    /// A provider's model list being fetched: its index and the worker.
    fetch: Option<ModelFetch>,
    /// Providers whose model list was already refreshed in this pane.
    refreshed: std::collections::BTreeSet<String>,
    /// App-wide settings this pane changed, handed to the host after drawing.
    app_writes: Vec<(String, Value)>,
    /// A summary running on its own thread, with the history index it folds up to.
    summary: Option<SummaryJob>,
    /// A digest of the first prompt being written by the model.
    title_job: Option<TitleJob>,
    /// First prompt waiting for the main run to finish before its digest is asked for.
    title_prompt: Option<String>,
    /// The model's digest of the chat, once it arrived.
    generated_title: Option<String>,
    /// When the live chat was started, carried into the list when it is filed.
    created: u64,
    /// Each turn's Markdown, parsed once so a frame does not parse the whole transcript.
    parsed: Vec<ParsedTurn>,
    /// The same for the sub-agent transcript on screen, which is read-only.
    parsed_sub: Vec<ParsedTurn>,
    /// The question composer, while `ask_user` waits.
    questions: QuestionWizard,
    /// The prompt opened for editing in the transcript, if any.
    editing: Option<MessageEdit>,
    /// The password being typed into the sudo card.
    sudo: String,
    /// Whether the stored conversation has been read back.
    loaded: bool,
    /// Caption and health for the pane header.
    caption: (String, HeaderHealth),
    /// What the caption was built from: running, summarising, session state and sub-agents.
    caption_for: Option<(bool, bool, SessionStatus, usize, bool)>,
    /// Last problem reported to the user.
    error: Option<String>,
    /// Problems raised in one frame, forwarded to the host as toasts.
    toasts: Vec<(ToastKind, String)>,
    /// Provider list and keys [`AiAgent::sync_providers`] last derived its state from.
    synced_providers: Option<Value>,
    synced_secrets: std::collections::BTreeMap<String, String>,
    /// Setting values [`AiAgent::apply_settings`] last derived the agent's rules from;
    /// `None` until it has run once.
    applied: Option<serde_json::Map<String, Value>>,
    /// Search key the agent's tools were last given.
    applied_web_key: Option<String>,
    /// Which chats the list shows its sub-agents for, by the chat's start stamp.
    ///
    /// Collapsed by default: a chat with sub-agents says how many there are and lists them
    /// only when the user clicks it.
    expanded_chats: std::collections::BTreeSet<u64>,
    /// Tool card whose copy button confirmed: which card, which button, and when.
    copied: Option<(usize, CopyTarget, u64)>,
    /// The scope popup a tag click opened, if any.
    scope_popup: Option<ScopePopup>,
    /// Map revision the host's plugin data holds, so a change is written once.
    map_saved: u64,
    /// System-facts revision the host's plugin data holds.
    system_saved: u64,
    /// Set by the popup: rebuild the map on the next frame.
    map_rebuild: bool,
    /// Set by the popup: have the files of the map described by the model.
    map_deep_wanted: bool,
    /// The mapping pass running for this pane, until its answer lands.
    map_deep: Option<MapRun>,
    /// Set by the popup: show what the mapping pass is doing, call by call.
    map_more: bool,
    /// Which of the pane's two views is drawn.
    view: ChatView,
}

/// A mapping pass running on a sub-agent.
struct MapRun {
    /// Its index in [`Agent::subagents`].
    subagent: usize,
    /// The tree it is mapping.
    root: String,
    /// When it started, for the popup's elapsed time.
    started: u64,
    /// Answers this pass has taken, so a shapeless one is asked again once.
    attempts: u8,
}

/// The popup a click on a loaded scope's tag opens.
struct ScopePopup {
    /// Scope it belongs to.
    id: &'static str,
    /// The tag it came from, so it sits under that tag and a press on it does not close the
    /// popup it just opened.
    anchor: egui::Rect,
}

/// Which of the pane's three views is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChatView {
    /// The chat list, with the composer that starts a new chat.
    List,
    /// The conversation itself.
    Chat,
    /// One sub-agent's transcript, read-only.
    Subagent(usize),
}

/// Which conversation the transcript draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptSource {
    /// The open chat.
    Main,
    /// One of its sub-agents.
    Subagent(usize),
}

impl Default for AiAgent {
    fn default() -> Self {
        Self {
            agent: Agent::new(),
            chats: Vec::new(),
            providers: Vec::new(),
            provider: 0,
            model: String::new(),
            keys: std::collections::BTreeMap::new(),
            draft: String::new(),
            composer_id: egui::Id::unique("ai-agent-composer"),
            queued: None,
            attachments: Vec::new(),
            attach_terminal: false,
            picking: None,
            fetch: None,
            refreshed: std::collections::BTreeSet::new(),
            app_writes: Vec::new(),
            summary: None,
            title_job: None,
            title_prompt: None,
            generated_title: None,
            created: 0,
            parsed: Vec::new(),
            parsed_sub: Vec::new(),
            questions: QuestionWizard::default(),
            editing: None,
            sudo: String::new(),
            loaded: false,
            caption: (String::new(), HeaderHealth::Ok),
            caption_for: None,
            error: None,
            toasts: Vec::new(),
            synced_providers: None,
            synced_secrets: std::collections::BTreeMap::new(),
            applied: None,
            applied_web_key: None,
            copied: None,
            scope_popup: None,
            expanded_chats: std::collections::BTreeSet::new(),
            map_saved: 0,
            system_saved: 0,
            map_rebuild: false,
            map_deep_wanted: false,
            map_deep: None,
            map_more: false,
            view: ChatView::List,
        }
    }
}

/// Whether `key` is a setting the agent's rules and tools are derived from.
fn derived_setting(key: &str) -> bool {
    DERIVED_SETTINGS.contains(&key)
        || key.starts_with(GROUP_KEY_PREFIX)
        || key.starts_with(METHOD_KEY_PREFIX)
}

/// Whether `name` is a tool that reads, searches or changes files on the connected host.
///
/// The `dev_*` tools are the richer set; the `remote_fs_*` ones cover what they do not,
/// such as deleting.
fn is_host_file_tool(name: &str) -> bool {
    name.starts_with("dev_") || name.starts_with("remote_fs_")
}

/// Whether `name` is a tool that works on the files of this computer.
fn is_local_file_tool(name: &str) -> bool {
    name.starts_with("local_fs_")
}

/// Read a settings value as trimmed text.
fn setting_text(settings: &serde_json::Map<String, Value>, key: &str) -> String {
    settings
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// How many tokens `model` can take, from the settings rows, else the default.
fn context_window(settings: &serde_json::Map<String, Value>, model: &str) -> usize {
    let model = model.trim().to_lowercase();
    let rows = settings.get(SETTING_MODEL_CONTEXTS).and_then(Value::as_array);
    for row in rows.into_iter().flatten() {
        let name = row
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let tokens = row.get("tokens").and_then(Value::as_i64).unwrap_or_default();
        if name.is_empty() || tokens <= 0 {
            continue;
        }
        if model.starts_with(&name) {
            return tokens as usize;
        }
    }
    DEFAULT_CONTEXT_WINDOW_TOKENS
}

/// Read a settings value as a rule list: one pattern per line or array item.
fn setting_rules(settings: &serde_json::Map<String, Value>, key: &str) -> Option<Vec<String>> {
    let lines: Vec<String> = match settings.get(key)? {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Value::String(text) => text
            .lines()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect(),
        _ => return None,
    };
    Some(lines)
}

/// The request one conversation sends, from the provider and model the pane picked.
fn build_request(
    agent: &mut Agent,
    provider: &ProviderConfig,
    api_key: String,
    model: &str,
    settings: &serde_json::Map<String, Value>,
    host: &str,
    environment: &str,
) -> CompletionRequest {
    agent.repair_history();
    let system = AiAgent::system_prompt(agent, settings, host, environment);
    let tools = tools::specs(
        &agent.permissions,
        &agent.scopes,
        !agent.subagent,
        agent.plan,
        agent.script_mode,
    );
    // Kept so compaction measures the scopes this conversation really carries.
    agent.prompt_chars = tools::prompt_chars(&tools, &system);
    CompletionRequest {
        base_url: provider.base_url.clone(),
        api_key,
        protocol: provider.protocol,
        model: model.to_owned(),
        system,
        messages: agent.history.clone(),
        tools,
        max_tokens: MAX_TOKENS,
    }
}

/// The last `max_chars` characters of `text`.
fn tail_chars(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    match text.char_indices().rev().nth(max_chars - 1) {
        Some((index, _)) => &text[index..],
        None => text,
    }
}

impl AiAgent {
    /// Move the live conversation into the shape the chat list keeps.
    fn stored_chat(&mut self) -> StoredChat {
        let title = self.title();
        let provider = self
            .active()
            .map(|provider| provider.id.clone())
            .unwrap_or_default();
        StoredChat {
            title,
            created: self.created,
            // A digest still to be written belongs to this conversation, not to the one
            // that follows it.
            title_prompt: self.title_prompt.take(),
            title_job: self.title_job.take(),
            turns: std::mem::take(&mut self.agent.turns),
            history: std::mem::take(&mut self.agent.history),
            subagents: std::mem::take(&mut self.agent.subagents),
            cwd: std::mem::take(&mut self.agent.cwd),
            model: self.model.clone(),
            provider,
            compacted: self.agent.compacted.take(),
            scopes: std::mem::take(&mut self.agent.scopes),
            plan: self.agent.plan,
        }
    }

    /// Set the live conversation aside so the chat list can reopen it.
    fn file_chat(&mut self) {
        if self.agent.turns.is_empty() {
            return;
        }
        let current = self.stored_chat();
        self.chats.push(current);
        let over = self.chats.len().saturating_sub(CONVERSATIONS_PER_HOST_MAX);
        if over > 0 {
            for chat in self.chats.drain(..over) {
                for mut sub in chat.subagents {
                    if sub.running() {
                        sub.agent.stop();
                    }
                }
            }
        }
    }

    /// Drop the live conversation, keeping whatever the composer holds.
    fn reset_chat(&mut self) {
        self.agent.clear();
        self.error = None;
        self.generated_title = None;
        self.title_job = None;
        self.title_prompt = None;
        self.created = self.agent.now_ms;
        self.questions = QuestionWizard::default();
        self.editing = None;
        self.parsed_sub.clear();
    }

    /// Set the live conversation aside and start an empty one, in the conversation view.
    fn begin_new_chat(&mut self) {
        self.file_chat();
        self.reset_chat();
        self.attachments.clear();
        self.view = ChatView::Chat;
    }
    /// Swap a stored conversation back into the pane.
    fn open_chat(&mut self, index: usize) {
        if index >= self.chats.len() {
            return;
        }
        let chat = if self.agent.turns.is_empty() {
            self.chats.remove(index)
        } else {
            let current = self.stored_chat();
            std::mem::replace(&mut self.chats[index], current)
        };
        self.agent.clear();
        self.agent.turns = chat.turns;
        self.agent.history = chat.history;
        self.agent.adopt_subagents(chat.subagents);
        self.agent.cwd = chat.cwd;
        self.agent.compacted = chat.compacted;
        self.agent.scopes = chat.scopes;
        self.agent.plan = chat.plan;
        self.created = chat.created;
        self.generated_title = (chat.title != DEFAULT_CHAT_TITLE).then(|| chat.title.clone());
        // A digest this chat was still waiting for goes on being written here.
        self.title_prompt = chat.title_prompt;
        self.title_job = chat.title_job;
        if !chat.model.is_empty() {
            self.model = chat.model;
        }
        if let Some(position) = self
            .providers
            .iter()
            .position(|item| item.id == chat.provider)
        {
            self.provider = position;
        }
        self.attachments.clear();
        self.error = None;
        self.editing = None;
        self.view = ChatView::Chat;
    }

    /// The provider in use.
    fn active(&self) -> Option<&ProviderConfig> {
        self.providers.get(self.provider)
    }

    /// The system prompt: what the agent is connected to, the tools this session really
    /// offers, and how to choose between them.
    fn system_prompt(
        agent: &Agent,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) -> String {
        let mut lines = Vec::new();
        if agent.subagent {
            lines.push(
                "You are a sub-agent that another agent delegated one task to. Work that task \
through to an answer, then reply with a concise summary of what you found or did and stop. \
You cannot ask the main conversation to do work for you and you cannot start sub-agents."
                    .to_owned(),
            );
        }
        lines.extend([
            format!(
                "You are an AI development agent connected to the active WaSSH session \"{host}\"."
            ),
            format!("Environment: {environment}."),
            format!("Current working directory: {}", agent.cwd),
        ]);
        if agent.plan {
            lines.push(
                "This chat is in Plan mode: every tool offered here only reads. Investigate \
as far as the question needs, then answer with what you would change, where, and why — you \
cannot change anything yourself, and the user switches to Agent mode to carry the plan out."
                    .to_owned(),
            );
        }
        lines.extend([
            String::new(),
            // Tools arrive by scope: sending every schema on every turn would spend the
            // prompt on tools nobody asked for.
            "Tools come in scopes, and only a loaded scope's tools can be called. Bring in \
another scope with load_scope and drop one with unload_scope. Loading the scope a task \
needs is the normal first step: the shell is the fallback, not the first choice. A scope \
that shows no tools of its own is a step on the way: loading it opens the scopes it nests."
                .to_owned(),
            String::new(),
            "Loaded scopes:".to_owned(),
        ]);
        // Only what the request actually sends: naming a tool this host may not call just
        // invites the model to try it and fall back to the shell.
        for scope in agent.scopes.scopes() {
            let names = Self::scope_tools(agent, scope);
            if !names.is_empty() {
                lines.push(format!("- {}: {}", scope.id, names.join(", ")));
            }
        }
        let loadable = scopes::visible(&agent.scopes, &agent.permissions);
        if !loadable.is_empty() {
            lines.push(String::new());
            lines.push("Scopes to load, by id:".to_owned());
            for scope in loadable {
                lines.push(format!("- {}: {}", scope.id, scope.summary));
            }
        }
        // Script mode sends `run_script` instead of one schema per tool, so the tools it can
        // call are spelled out here: a line each instead of a schema each.
        if agent.script_mode {
            let catalog =
                tools::catalog(&agent.permissions, &agent.scopes, !agent.subagent, agent.plan);
            if !catalog.is_empty() {
                lines.push(String::new());
                lines.push(format!(
                    "Call tools from a script. `run_script` runs a JavaScript program in a \
sandbox: `await tools.<name>(args)` runs a tool and resolves to its JSON result \
(`{{ ok: true, text }}`), and a failed call throws. A script gets {} seconds of JavaScript \
— time spent waiting on a tool does not count — and up to {} tool calls; one call that has \
not answered within {} seconds fails with a timeout. Run multiple tool calls in one go when \
it makes sense: put every call a step needs in a single script, batch independent calls \
together and chain the ones that depend on each other, instead of spending a round per \
call, because each round re-sends the whole prompt. Do the processing and the decisions in \
the script rather than back through the conversation: filter, join, compare and branch on \
results there. Spend real effort on shrinking what you return: filter down to the lines \
that answer the question, sort and de-duplicate them, and compact, count or summarise the \
rest instead of dumping raw output — that result is all the model sees, so keep it as \
small as the answer allows. Callable now:",
                    super::script::MAX_EXEC_MS / 1_000,
                    super::script::MAX_CALLS,
                    super::script::MAX_CALL_MS / 1_000
                ));
                lines.push(catalog);
            }
        }
        // The map's digest: the tree this conversation already knows, so its turns go on
        // the work rather than on finding the files again.
        if agent.scopes.contains(ENV_SCOPE) && !agent.map.is_empty() {
            lines.push(String::new());
            lines.push(MAP_HEADING.to_owned());
            lines.push(agent.map.overview());
        }
        if agent.scopes.contains(ENV_SCOPE) && !agent.system.is_empty() {
            lines.push(String::new());
            lines.push(SYSTEM_HEADING.to_owned());
            lines.push(agent.system.summary());
        }
        lines.push(String::new());
        lines.push("Guidelines:".to_owned());
        lines.extend(Self::tool_guidance(agent));
        lines.extend([
            "- After each command or tool call you see its actual result. Never invent output."
                .to_owned(),
            "- Ask before doing anything destructive or ambiguous.".to_owned(),
            "- When the task is complete, reply with a concise plain-text summary.".to_owned(),
        ]);
        let global = setting_text(settings, SETTING_GLOBAL_PROMPT);
        let host_text = setting_text(settings, SETTING_HOST_PROMPT);
        if !global.is_empty() || !host_text.is_empty() {
            lines.push(String::new());
            lines.push("Additional prompt instructions and context:".to_owned());
            lines.push(String::new());
            if !global.is_empty() {
                lines.push(format!("[Global prompt]\n{global}"));
            }
            if !host_text.is_empty() {
                lines.push(format!("[Host prompt]\n{host_text}"));
            }
        }
        if let Some(summary) = &agent.compacted {
            lines.push(String::new());
            lines.push(COMPACTED_HEADING.to_owned());
            lines.push(summary.clone());
        }
        lines.join("\n")
    }

    /// The tools of `scope` this conversation can call, as the model names them.
    fn scope_tools(agent: &Agent, scope: &scopes::Scope) -> Vec<&'static str> {
        let mut names = scope.tools.to_vec();
        if let Some(pane) = scope.pane {
            names.extend(crate::plugin::tools_of(pane).iter().map(|tool| tool.name));
        }
        names.retain(|name| agent.offers_tool(name));
        names
    }

    /// The scope this conversation would load to get at the host's files, and the file tools
    /// it holds.
    ///
    /// Naming them while they are not loaded is what stops the agent from reading, searching
    /// and editing files with `cat`, `grep` and `sed` instead of loading the scope.
    fn host_file_scope(agent: &Agent) -> Option<(&'static str, String)> {
        let scope = scopes::visible(&agent.scopes, &agent.permissions)
            .into_iter()
            .find(|scope| scope.tools.iter().any(|name| is_host_file_tool(name)))?;
        let names: Vec<&str> = scope
            .tools
            .iter()
            .copied()
            .filter(|name| is_host_file_tool(name))
            .collect();
        Some((scope.id, names.join(", ")))
    }

    /// How to choose between this session's tools, named as they are offered here.
    ///
    /// A family the host has switched off is not mentioned, so the agent never reads about
    /// a capability it cannot use; a file tool that is missing is reported as such rather
    /// than worked around with a shell command.
    fn tool_guidance(agent: &Agent) -> Vec<String> {
        let offered = |keep: fn(&str) -> bool| -> Vec<&'static str> {
            tools::TOOL_SPECS
                .iter()
                .map(|spec| spec.name)
                .filter(|name| keep(name) && agent.offers_tool(name))
                .collect()
        };
        let mut lines = vec![
            "- Reach for a tool before a shell command wherever one fits: tools cannot hang on \
             a pager, they answer with structured output, and the user sees exactly what was \
             read or changed."
                .to_owned(),
            "- A tool you want but do not have is not an invitation to improvise in the shell. \
             If it belongs to a scope listed above, load that scope with load_scope first."
                .to_owned(),
        ];
        let host_files = offered(is_host_file_tool);
        if !host_files.is_empty() {
            lines.push(format!(
                "- Files on the connected host: {}. Every read, listing, search and change of a \
                 file goes through these tools. Do not reach for cat, head, tail, ls, find, \
                 grep/rg, sed, awk or shell redirection for file work: the tools are quicker, \
                 they cannot hang on a pager, and they show the user what changed.",
                host_files.join(", ")
            ));
            if !agent.offers_tool(super::protocol::TOOL_ENV_FIND)
                && scopes::visible(&agent.scopes, &agent.permissions)
                    .iter()
                    .any(|scope| scope.id == ENV_SCOPE)
            {
                lines.push(
                    "- Development work has a separate Graph scope ('env'). Load it when you need \
                     to locate files or declarations across the host; env_search, env_find and \
                     env_outline query the graph instead of re-discovering the tree with shell \
                     commands."
                        .to_owned(),
                );
            }
        } else if let Some((scope, names)) = Self::host_file_scope(agent) {
            lines.push(format!(
                "- Files on the connected host are read, searched and changed with {names}, \
                 which the '{scope}' scope holds. Load that scope before touching a file there; \
                 do not use cat, head, tail, ls, find, grep/rg, sed, awk or shell redirection \
                 in its place. For development, also load 'env' (Graph) separately when you need \
                 its file/declaration index."
            ));
        }
        let local_files = offered(is_local_file_tool);
        if !local_files.is_empty() {
            lines.push(format!(
                "- Files on this computer: {}.",
                local_files.join(", ")
            ));
        }
        if agent.offers_tool(super::protocol::TOOL_ENV_FIND) {
            lines.push(
                "- Look in the environment map before searching the tree: env_search ranks \
                 files by how well their path and declarations answer a phrase, env_find \
                 answers \"where is X\", and env_outline describes a file, all without touching \
                 the host. Call env_scan for a directory the map does not hold yet. env_system \
                 says what listens on a port and which services run. The dev_* search and read \
                 tools are for what the map cannot answer."
                    .to_owned(),
            );
        }
        let shells: Vec<&str> = [
            super::protocol::TOOL_RUN_COMMAND,
            super::protocol::TOOL_RUN_COMMAND_BACKGROUND,
            super::protocol::TOOL_RUN_SUDO,
            super::protocol::TOOL_LOCAL_RUN,
        ]
        .into_iter()
        .filter(|name| agent.offers_tool(name))
        .collect();
        if !shells.is_empty() {
            lines.push(format!(
                "- {} {} for work no tool covers: builds, tests, services, package managers and \
                 version control. Never for reading, listing, searching or editing files.",
                shells.join(", "),
                if shells.len() == 1 { "is" } else { "are" }
            ));
        }
        if agent.offers_tool(super::protocol::TOOL_TERMINAL_READ) {
            lines.push(
                "- The terminal_* tools drive the live shell the user is watching; use them for \
                 what only an interactive program can do."
                    .to_owned(),
            );
        }
        if agent.offers_tool(super::protocol::TOOL_RUN_SUDO) {
            lines.push(
                "- When a task needs root, call run_sudo with the same command: it asks the user \
                 for their sudo password before running. Plain run_command cannot answer sudo's \
                 prompt, and never pipe a password to sudo yourself."
                    .to_owned(),
            );
        }
        lines.push(
            "- If a file tool you need is missing from the loaded scopes, load the scope that \
             holds it. If no scope offers it, the capability is switched off on this host — say \
             so instead of doing the same job with a shell command."
                .to_owned(),
        );
        lines
    }

    /// Rules and tool settings from the plugin data.
    ///
    /// `web_key` is the search provider's API key, which the shell decrypts from
    /// the credential vault instead of keeping it in a settings file.
    fn apply_settings(&mut self, settings: &serde_json::Map<String, Value>, web_key: Option<&str>) {
        if !self.settings_changed(settings) && self.applied_web_key.as_deref() == web_key {
            return;
        }
        self.applied = Some(
            settings
                .iter()
                .filter(|(key, _)| derived_setting(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        self.applied_web_key = web_key.map(str::to_owned);
        self.agent.app_allow = setting_rules(settings, SETTING_DEFAULT_ALLOW_RULES)
            .unwrap_or_else(|| SAFE_RULES.iter().map(|rule| (*rule).to_owned()).collect());
        self.agent.app_deny =
            setting_rules(settings, SETTING_DEFAULT_DENY_RULES).unwrap_or_default();
        self.agent.host_allow =
            setting_rules(settings, SETTING_HOST_ALLOW_RULES).unwrap_or_default();
        self.agent.host_deny = setting_rules(settings, SETTING_HOST_DENY_RULES).unwrap_or_default();
        self.agent.permissions = Permissions::from_settings(settings);
        self.agent.script_mode = settings
            .get(SETTING_HOST_SCRIPT_MODE)
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || settings
                .get(SETTING_SCRIPT_MODE)
                .and_then(Value::as_bool)
                .unwrap_or(false);
        self.agent.max_subagents = settings
            .get(SETTING_MAX_SUBAGENTS)
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_SUBAGENTS, |limit| limit as usize);

        let mut folders = Vec::new();
        let global_folder = setting_text(settings, SETTING_RAG_FOLDER);
        if !global_folder.is_empty() {
            folders.push(global_folder);
        }
        let host_folder = setting_text(settings, SETTING_HOST_RAG_FOLDER);
        if !host_folder.is_empty() {
            folders.push(host_folder);
        }
        let embed = self.embedding_config(settings);
        self.agent.tools = ToolSettings {
            web_provider: {
                let provider = setting_text(settings, SETTING_WEB_SEARCH_PROVIDER);
                (!provider.is_empty()).then_some(provider)
            },
            web_key: web_key
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_owned),
            rag_folders: folders,
            embed,
        };
    }

    /// Whether any setting the agent's rules come from changed since the last draw.
    ///
    /// The pane is drawn every frame, so re-deriving the rule lists, the permissions and
    /// the tool settings every frame would only produce the same values again.
    fn settings_changed(&self, settings: &serde_json::Map<String, Value>) -> bool {
        let Some(applied) = &self.applied else {
            return true;
        };
        let mut derived = 0usize;
        for (key, value) in settings {
            if !derived_setting(key) {
                continue;
            }
            if applied.get(key) != Some(value) {
                return true;
            }
            derived += 1;
        }
        derived != applied.len()
    }

    /// The embedding provider from the settings, when one is configured.
    fn embedding_config(&self, settings: &serde_json::Map<String, Value>) -> Option<EmbedConfig> {
        let id = setting_text(settings, SETTING_RAG_PROVIDER);
        if id.is_empty() {
            return None;
        }
        let model = setting_text(settings, SETTING_RAG_MODEL);
        if model.is_empty() {
            return None;
        }
        let provider = self.providers.iter().find(|provider| provider.id == id)?;
        // Only OpenAI-compatible providers expose an embeddings endpoint.
        if provider.protocol != Protocol::Openai {
            return None;
        }
        Some(EmbedConfig {
            base_url: provider.base_url.clone(),
            api_key: self.keys.get(&provider.id).cloned().unwrap_or_default(),
            protocol: provider.protocol,
            model,
        })
    }

    /// Build the request for the conversation as it stands.
    ///
    /// The history is repaired first: a run that was stopped, killed or lost in the middle
    /// leaves a tool call the provider would reject the whole conversation over. It is done
    /// here so every path that sends a request gets a history a provider accepts.
    fn request(
        &mut self,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) -> Option<CompletionRequest> {
        self.request_for(None, settings, host, environment)
    }

    /// Build the request for the conversation as it stands, or for one of its sub-agents.
    fn request_for(
        &mut self,
        subagent: Option<usize>,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) -> Option<CompletionRequest> {
        let provider = self.active()?.clone();
        let api_key = self.keys.get(&provider.id).cloned().unwrap_or_default();
        let model = self.model.trim().to_owned();
        let agent = match subagent {
            Some(index) => &mut self.agent.subagents.get_mut(index)?.agent,
            None => &mut self.agent,
        };
        Some(build_request(
            agent,
            &provider,
            api_key,
            &model,
            settings,
            host,
            environment,
        ))
    }

    /// Turn a spawn the agent asked for into a sub-agent with its first request.
    fn launch_spawn(
        &mut self,
        session: &mut Option<&mut dyn crate::plugin::SessionAccess>,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        let Some(spawn) = self.agent.take_spawn_request() else {
            return;
        };
        let index = self.agent.begin_subagent(&spawn.title, &spawn.prompt);
        match self.request_for(Some(index), settings, host, environment) {
            Some(request) => self
                .agent
                .launch_subagent(index, request, spawn.call, spawn.turn, session),
            None => {
                let reason = "No model is configured, so the sub-agent cannot start.";
                if let Some(sub) = self.agent.subagents.get_mut(index) {
                    sub.agent.error = Some(reason.to_owned());
                }
                self.agent.fail_spawn(spawn, reason.to_owned(), session);
            }
        }
    }

    /// Start the next request of every sub-agent that owes an answer.
    fn drive_subagents(
        &mut self,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        for index in 0..self.agent.subagents.len() {
            let owes = {
                let sub = &mut self.agent.subagents[index];
                sub.running() && !sub.agent.blocked() && sub.agent.take_needs_reply()
            };
            if !owes {
                continue;
            }
            match self.request_for(Some(index), settings, host, environment) {
                Some(request) => self.agent.subagents[index].agent.start(request),
                None => {
                    self.agent.subagents[index].agent.error =
                        Some("No model is configured, so the sub-agent stopped.".to_owned());
                }
            }
        }
    }

    /// Keep the filed chats' sub-agents working while another chat is open.
    fn poll_filed_subagents(
        &mut self,
        session: &mut Option<&mut dyn crate::plugin::SessionAccess>,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        let Some(provider) = self.active().cloned() else {
            return;
        };
        let api_key = self.keys.get(&provider.id).cloned().unwrap_or_default();
        let model = self.model.trim().to_owned();
        let now_ms = self.agent.now_ms;
        for chat in self.chats.iter_mut() {
            for index in 0..chat.subagents.len() {
                if !chat.subagents[index].running() {
                    continue;
                }
                chat.subagents[index].agent.now_ms = now_ms;
                chat.subagents[index].agent.poll(session);
                let (waiting, ended) = {
                    let sub = &mut chat.subagents[index];
                    let waiting = announce_wait(sub);
                    let mut ended = None;
                    // A filed chat's prompts wait until the chat is opened again.
                    if sub.agent.needs_user() {
                        // Nothing else to move: the run is parked on the user.
                    } else if sub.agent.take_needs_reply() {
                        let request = build_request(
                            &mut sub.agent,
                            &provider,
                            api_key.clone(),
                            &model,
                            settings,
                            host,
                            environment,
                        );
                        sub.agent.start(request);
                    } else if sub.agent.phase.is_none() && !sub.agent.spawning() {
                        ended = Some(end_subagent(sub));
                    }
                    (waiting, ended)
                };
                if let Some(turn) = waiting {
                    chat.turns.push(turn);
                }
                if let Some((turn, report)) = ended {
                    chat.turns.push(turn);
                    chat.history
                        .push(super::protocol::ApiMessage::User {
                            content: report,
                            images: Vec::new(),
                        });
                }
            }
        }
    }

    /// What the pane has to show: the main conversation's request, else a sub-agent's.
    fn pending_prompts(&self) -> (Prompts, PromptTarget) {
        let own = Prompts::of(&self.agent);
        if !own.is_empty() {
            return (own, PromptTarget::Main);
        }
        match self.agent.waiting_subagent() {
            Some(index) => (
                Prompts::of(&self.agent.subagents[index].agent),
                PromptTarget::Subagent(index),
            ),
            None => (own, PromptTarget::Main),
        }
    }

    /// The agent whose prompt is on screen.
    fn prompt_agent(&mut self, target: PromptTarget) -> &mut Agent {
        match target {
            PromptTarget::Main => &mut self.agent,
            PromptTarget::Subagent(index) => &mut self.agent.subagents[index].agent,
        }
    }

    /// Stop the run whose prompt is on screen.
    fn stop_prompted(&mut self, target: PromptTarget) {
        match target {
            PromptTarget::Main => self.agent.stop(),
            PromptTarget::Subagent(index) => {
                let Some(sub) = self.agent.subagents.get_mut(index) else {
                    return;
                };
                sub.agent.stop();
                sub.end = Some(SubagentEnd::Stopped);
            }
        }
    }

    /// What the action button does: stop the main run, then the sub-agents it left behind.
    fn action_state(&self, can_send: bool) -> ActionState {
        if can_send {
            return ActionState::Send;
        }
        if self.agent.blocked() {
            ActionState::StopMain
        } else if self.agent.subagents_running() > 0 {
            ActionState::StopSubagents
        } else {
            ActionState::Send
        }
    }

    /// Whether a message can go out: a provider and a model are configured.
    fn ready(&self) -> Result<(), &'static str> {
        if self.providers.is_empty() {
            return Err("No model providers configured — add one in Options.");
        }
        if self.model.trim().is_empty() {
            return Err("Pick a model before sending.");
        }
        Ok(())
    }

    fn report_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        self.error = Some(error.clone());
        self.toasts.push((ToastKind::Error, error));
    }

    /// Ask the model for a digest of `prompt`, if a provider is configured.
    fn title_request(&self, prompt: &str) -> Option<TitleJob> {
        let provider = self.active().cloned()?;
        let model = self.model.clone();
        let key = self.keys.get(&provider.id).cloned().unwrap_or_default();
        let prompt = prompt.trim().to_owned();
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let request = CompletionRequest {
                base_url: provider.base_url,
                api_key: key,
                protocol: provider.protocol,
                model,
                system: CHAT_TITLE_SYSTEM_PROMPT.to_owned(),
                messages: vec![super::protocol::ApiMessage::User {
                    content: prompt,
                    images: Vec::new(),
                }],
                tools: Vec::new(),
                max_tokens: 32,
            };
            let result = complete_blocking(&request).map(|completion| completion.text.trim().to_owned());
            let _ = sender.send(result);
        });
        Some(events)
    }

    /// Collect the digests the model wrote and ask for the next one that is due.
    fn poll_chat_title(&mut self) {
        // The live chat asks once its run is done, so the digest reads the prompt alone.
        if self.title_job.is_none()
            && !self.agent.blocked()
            && let Some(prompt) = self.title_prompt.clone()
            && let Some(job) = self.title_request(&prompt)
        {
            self.title_prompt = None;
            self.title_job = Some(job);
        }
        if let Some(answer) = self.title_job.as_ref().and_then(title_answer) {
            self.title_job = None;
            if let Some(title) = digest_title(answer) {
                self.generated_title = Some(title);
            }
        }
        // Filed chats keep their own work: a chat filed before its digest came back still
        // gets one, and the list shows it as soon as it lands.
        for chat in &mut self.chats {
            if let Some(answer) = chat.title_job.as_ref().and_then(title_answer) {
                chat.title_job = None;
                if let Some(title) = digest_title(answer) {
                    chat.title = title;
                }
            }
        }
        // One request at a time, so a list of untitled chats does not burst the provider.
        if self.title_job.is_none()
            && let Some(index) = self
                .chats
                .iter()
                .position(|chat| chat.title_prompt.is_some() && chat.title_job.is_none())
            && let Some(prompt) = self.chats[index].title_prompt.clone()
            && let Some(job) = self.title_request(&prompt)
        {
            self.chats[index].title_prompt = None;
            self.chats[index].title_job = Some(job);
        }
    }

    /// Hand one message to the agent: it becomes a turn and its reply starts.
    fn send_message(
        &mut self,
        message: &str,
        meta: UserMeta,
        images: Vec<ImagePart>,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        if !self.agent.push_user_with_images(message, meta, images) {
            return;
        }
        // The first message of a conversation stamps when it was started, for the list's
        // order.
        if self.created == 0 {
            self.created = self.agent.now_ms;
        }
        self.error = None;
        if self.generated_title.is_none() && self.title_prompt.is_none() {
            self.title_prompt = Some(message.to_owned());
        }
        if let Some(request) = self.request(settings, host, environment) {
            self.agent.start(request);
        }
    }

    /// Ask the model again after a run was cut off, like the reference's Continue.
    fn resume(
        &mut self,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        if !self.agent.can_resume() {
            return;
        }
        if let Err(error) = self.ready() {
            self.report_error(error);
            return;
        }
        if let Some(request) = self.request(settings, host, environment) {
            self.error = None;
            self.agent.start(request);
        }
    }

    /// Whether a reply is under way but has painted nothing yet.
    fn waiting_for_the_first_token(&self) -> bool {
        let Some(Phase::Streaming { turn, .. }) = &self.agent.phase else {
            return false;
        };
        matches!(
            self.agent.turns.get(*turn),
            Some(Turn::Assistant { text, .. }) if text.is_empty()
        )
    }

    /// Take the provider list and API keys from the app-wide settings and the vault.
    ///
    /// Options owns both, so they are re-read whenever they change and never written back
    /// here; the current provider and model stay per conversation. Keys arrive decrypted
    /// from the host, because a pane cannot reach the credential vault itself.
    fn sync_providers(
        &mut self,
        settings: &serde_json::Map<String, Value>,
        secrets: &std::collections::BTreeMap<String, String>,
    ) {
        let source = settings.get(PROVIDERS_KEY);
        if self.synced_providers.as_ref() == source && &self.synced_secrets == secrets {
            return;
        }
        self.synced_providers = source.cloned();
        self.synced_secrets.clone_from(secrets);
        let selected = self.active().map(|provider| provider.id.clone());
        self.providers = settings
            .get(PROVIDERS_KEY)
            .and_then(|value| serde_json::from_value::<Vec<ProviderConfig>>(value.clone()).ok())
            .unwrap_or_default();
        self.keys = self
            .providers
            .iter()
            .filter_map(|provider| {
                let field = super::settings_ui::provider_secret_field(&provider.id);
                secrets
                    .get(&field)
                    .map(|key| (provider.id.clone(), key.clone()))
            })
            .collect();
        self.provider = selected
            .and_then(|id| self.providers.iter().position(|provider| provider.id == id))
            .unwrap_or(0)
            .min(self.providers.len().saturating_sub(1));
        // A conversation without a model starts on the provider's first one.
        if self.model.is_empty()
            && let Some(provider) = self.providers.get(self.provider)
            && let Some(model) = provider.models.first()
        {
            self.model.clone_from(model);
        }
    }

    /// Fetch a provider's model list the first time it is used.
    ///
    /// The reference refreshes the list from the provider itself instead of
    /// trusting what Options last saved, so a new Ollama model shows up without
    /// anyone pressing a button.
    fn refresh_models(&mut self, context: &PluginContext<'_>) {
        if self.fetch.is_some() {
            return;
        }
        let Some(provider) = self.active() else {
            return;
        };
        if self.refreshed.contains(&provider.id) {
            return;
        }
        let key = context
            .secrets
            .get(&provider_secret_field(&provider.id))
            .cloned()
            .unwrap_or_default();
        // Anthropic lists its models only for a key; without one the list stays
        // as configured, like the reference.
        if key.is_empty() && provider.protocol == Protocol::Anthropic {
            return;
        }
        let index = self.provider;
        let provider = provider.clone();
        self.refreshed.insert(provider.id.clone());
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(provider_models(&provider, &key));
        });
        self.fetch = Some((index, events));
    }

    /// Collect a finished model list. Returns whether the list changed.
    fn poll_models(&mut self) -> bool {
        let Some((index, events)) = &self.fetch else {
            return false;
        };
        match events.try_recv() {
            Ok(Ok(mut models)) if !models.is_empty() => {
                let index = *index;
                // Same order as the reference: case-insensitive alphabetical.
                models.sort_by_key(|model| model.to_lowercase());
                self.fetch = None;
                if let Some(provider) = self.providers.get_mut(index) {
                    provider.models = models;
                }
                if self.model.is_empty()
                    && let Some(provider) = self.providers.get(index)
                    && let Some(model) = provider.models.first()
                {
                    self.model.clone_from(model);
                }
                // Options owns the provider list; the host files the fresh models there.
                self.app_writes.push((
                    PROVIDERS_KEY.to_owned(),
                    serde_json::to_value(&self.providers).unwrap_or(Value::Null),
                ));
                true
            }
            // An error or an empty list leaves the configured models alone.
            Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.fetch = None;
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }

    /// Read the stored conversation back in.
    fn load(&mut self, settings: &serde_json::Map<String, Value>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Some(stored) = settings.get(CONVERSATION_KEY) {
            self.restore(stored);
        }
        // The map belongs to the host, so it survives every chat on the connection.
        self.agent.map = super::map::EnvMap::from_json(settings.get(MAP_KEY));
        self.map_saved = self.agent.map.revision;
        self.agent.system = super::map::SystemFacts::from_json(settings.get(SYSTEM_KEY));
        self.system_saved = self.agent.system.revision;
        // The pane opens on the conversation it was in, or on the list when there is none.
        if !self.agent.turns.is_empty() {
            self.view = ChatView::Chat;
        }
    }

    /// Take the stored conversation and the chats filed under it.
    fn restore(&mut self, stored: &Value) {
        if let Some(model) = stored.get("model").and_then(Value::as_str)
            && !model.is_empty()
        {
            self.model = model.to_owned();
        }
        if let Some(provider) = stored.get("provider").and_then(Value::as_str)
            && let Some(index) = self.providers.iter().position(|item| item.id == provider)
        {
            self.provider = index;
        }
        if let Some(cwd) = stored.get("cwd").and_then(Value::as_str)
            && !cwd.is_empty()
        {
            self.agent.cwd = cwd.to_owned();
        }
        if let Some(turns) = stored.get("turns").and_then(Value::as_array) {
            self.agent.turns = turns_from_json(turns);
        }
        // The title is the model's digest; one the pane took from the prompt is asked for
        // again rather than shown.
        let stored_title = stored["title"].as_str().unwrap_or_default();
        if is_digest(stored_title, &self.agent.turns) {
            self.generated_title = Some(stored_title.to_owned());
        } else {
            self.title_prompt = first_prompt(&self.agent.turns).map(str::to_owned);
        }
        self.created = stored["created"]
            .as_u64()
            // An older file did not record it, and the live chat is the newest one.
            .unwrap_or(self.agent.now_ms);
        if let Some(history) = stored.get("history").and_then(Value::as_array) {
            self.agent.history = history_from_json(history);
        }
        self.agent.compacted = stored
            .get("compacted")
            .and_then(Value::as_str)
            .filter(|summary| !summary.is_empty())
            .map(str::to_owned);
        self.agent.scopes = Loaded::from_json(stored.get("scopes"));
        self.agent.plan = stored["planMode"].as_bool().unwrap_or(false);
        self.agent
            .adopt_subagents(subagents_from_json(stored.get("subagents")));
        if let Some(chats) = stored.get("chats").and_then(Value::as_array) {
            self.chats = chats
                .iter()
                .enumerate()
                .map(|(index, chat)| {
                    let turns = chat["turns"]
                        .as_array()
                        .map(|turns| turns_from_json(turns))
                        .unwrap_or_default();
                    let stored_title = chat["title"].as_str().unwrap_or_default();
                    let digest = is_digest(stored_title, &turns);
                    StoredChat {
                        title: if digest {
                            stored_title.to_owned()
                        } else {
                            DEFAULT_CHAT_TITLE.to_owned()
                        },
                        // A file written before chats carried their start keeps the order
                        // it stored them in; 0 stays free for "not stamped yet".
                        created: chat["created"].as_u64().unwrap_or(index as u64 + 1),
                        title_prompt: if digest {
                            None
                        } else {
                            first_prompt(&turns).map(str::to_owned)
                        },
                        title_job: None,
                        cwd: chat["cwd"].as_str().unwrap_or("/").to_owned(),
                        model: chat["model"].as_str().unwrap_or_default().to_owned(),
                        provider: chat["provider"].as_str().unwrap_or_default().to_owned(),
                        turns,
                        history: chat["history"]
                            .as_array()
                            .map(|history| history_from_json(history))
                            .unwrap_or_default(),
                        compacted: chat["compacted"]
                            .as_str()
                            .filter(|summary| !summary.is_empty())
                            .map(str::to_owned),
                        scopes: Loaded::from_json(chat.get("scopes")),
                        plan: chat["planMode"].as_bool().unwrap_or(false),
                        subagents: subagents_from_json(chat.get("subagents")),
                    }
                })
                .collect();
        }
    }

    /// Title for the header and the stored conversation: the model's digest of the chat,
    /// or a placeholder until it arrives.
    fn title(&self) -> String {
        self.generated_title
            .clone()
            .unwrap_or_else(|| DEFAULT_CHAT_TITLE.to_owned())
    }

    /// Start a summary of the conversation on a worker thread.
    ///
    /// `cut` is the history index a compaction folds up to; the other modes summarise the
    /// whole transcript.
    fn start_summary(
        &mut self,
        mode: SummaryMode,
        cut: usize,
        settings: &serde_json::Map<String, Value>,
        host: &str,
    ) {
        if self.summary.is_some() || self.agent.turns.is_empty() {
            return;
        }
        let Some(provider) = self.active().cloned() else {
            self.report_error("Add a provider first.");
            return;
        };
        let transcript = match mode {
            SummaryMode::Compact => self.agent.folded_text(cut),
            _ => self.agent.transcript_text(),
        };
        let model = self.model.clone();
        let key = self.keys.get(&provider.id).cloned().unwrap_or_default();
        let (sender, events) = std::sync::mpsc::channel();
        let _ = host;
        let _ = settings;
        std::thread::spawn(move || {
            let request = CompletionRequest {
                base_url: provider.base_url,
                api_key: key,
                protocol: provider.protocol,
                model,
                system: SUMMARY_SYSTEM_PROMPT.to_owned(),
                messages: vec![super::protocol::ApiMessage::User {
                    content: transcript,
                    images: Vec::new(),
                }],
                tools: Vec::new(),
                max_tokens: 1_024,
            };
            let result =
                complete_blocking(&request).map(|completion| completion.text.trim().to_owned());
            let _ = sender.send(result);
        });
        self.summary = Some((mode, cut, events));
    }

    /// Compact the conversation once it would no longer fit the model's context window.
    ///
    /// Off unless the host switched automatic compaction on: the model's window comes from
    /// the per-model settings rows, so a local model with a small window would otherwise be
    /// folded long before the user expects it. Only an idle chat is compacted: folding under
    /// a running turn would take away what it is answering.
    fn compact_if_needed(&mut self, settings: &serde_json::Map<String, Value>, host: &str) {
        if self.summary.is_some()
            || self.agent.blocked()
            || self.agent.needs_reply
            || self.queued.is_some()
            || !settings
                .get(SETTING_AUTO_COMPACT)
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return;
        }
        let window = context_window(settings, &self.model);
        // Measured from the last request, so a conversation is compacted against what its
        // loaded scopes actually cost rather than a guess.
        let overhead = if self.agent.prompt_chars == 0 {
            PROMPT_OVERHEAD_TOKENS
        } else {
            self.agent.prompt_chars / CHARS_PER_TOKEN
        };
        let budget = window.saturating_sub(MAX_TOKENS as usize + overhead);
        if self.agent.history_chars() / CHARS_PER_TOKEN <= budget {
            return;
        }
        let Some(cut) = self.agent.compact_split(COMPACT_KEEP_MESSAGES) else {
            return;
        };
        self.start_summary(SummaryMode::Compact, cut, settings, host);
    }

    /// Fold the older part of the chat into a summary because the user asked for it.
    ///
    /// This is what the Compact button and `/compact` do; with automatic compaction off it is
    /// the only path, and it says so when there is nothing to fold.
    fn compact_now(&mut self, settings: &serde_json::Map<String, Value>, host: &str) {
        if self.summary.is_some() {
            return;
        }
        if self.agent.blocked() || self.agent.needs_reply {
            self.report_error(COMPACT_BUSY);
            return;
        }
        let Some(cut) = self.agent.compact_split(COMPACT_KEEP_MESSAGES) else {
            self.agent
                .turns
                .push(Turn::Notice(COMPACT_EMPTY.to_owned()));
            return;
        };
        self.start_summary(SummaryMode::Compact, cut, settings, host);
    }

    /// Collect a finished summary.
    fn poll_summary(&mut self) {
        let Some((mode, cut, events)) = &self.summary else {
            return;
        };
        let ready = match events.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("The summary worker stopped.".to_owned()))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
        };
        let Some(result) = ready else {
            return;
        };
        let mode = *mode;
        let cut = *cut;
        self.summary = None;
        match result {
            Err(error) => self.report_error(error),
            Ok(summary) if summary.is_empty() => {
                self.report_error("The provider returned an empty summary.");
            }
            Ok(summary) => {
                if mode == SummaryMode::Compact {
                    self.agent.fold(cut, &summary);
                    self.agent
                        .turns
                        .insert(0, Turn::Notice(format!("{COMPACTED_NOTICE}\n{summary}")));
                } else if mode == SummaryMode::Transfer {
                    self.begin_new_chat();
                    self.agent.turns.push(Turn::Notice(format!(
                        "Chat transferred. Carried over:\n{summary}"
                    )));
                    self.agent.history.push(super::protocol::ApiMessage::User {
                        content: format!(
                            "[Context carried over from the previous chat]\n{summary}"
                        ),
                        images: Vec::new(),
                    });
                } else {
                    self.agent
                        .turns
                        .push(Turn::Notice(format!("Summary:\n{summary}")));
                }
            }
        }
    }

    /// Open the native file picker for attachments.
    fn pick_attachments(&mut self) {
        if self.picking.is_some() || self.attachments.len() >= MAX_ATTACHMENTS {
            return;
        }
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let files = rfd::FileDialog::new()
                .set_title("Attach files")
                .pick_files()
                .unwrap_or_default();
            let _ = sender.send(
                files
                    .into_iter()
                    .map(|path| path.display().to_string())
                    .collect(),
            );
        });
        self.picking = Some(events);
    }

    /// Collect the picked files.
    fn poll_picker(&mut self) {
        let Some(events) = &self.picking else {
            return;
        };
        match events.try_recv() {
            Ok(paths) => {
                self.picking = None;
                for path in paths {
                    self.attach(&path);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.picking = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Attach one file to the next message.
    fn attach(&mut self, path: &str) {
        if self.attachments.len() >= MAX_ATTACHMENTS {
            self.report_error(format!("At most {MAX_ATTACHMENTS} files can be attached."));
            return;
        }
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_IMAGE_BYTES as u64 => {
                self.report_error(format!(
                    "{} is larger than {} bytes.",
                    path, MAX_IMAGE_BYTES
                ));
                return;
            }
            Ok(_) => {}
            Err(error) => {
                self.report_error(format!("Cannot read {path}: {error}"));
                return;
            }
        }
        match std::fs::read(path) {
            Ok(bytes) => {
                if let Some(media_type) = image_media_type(&bytes) {
                    if self.attached_images().len() >= MAX_IMAGES {
                        self.report_error(format!(
                            "At most {MAX_IMAGES} images can be sent in one message."
                        ));
                        return;
                    }
                    self.attachments.push(Attachment {
                        path: path.to_owned(),
                        text: String::new(),
                        image: Some(ImagePart {
                            media_type: media_type.to_owned(),
                            data: bytes,
                        }),
                    });
                    return;
                }
                if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
                    self.report_error(format!(
                        "{path} is larger than {MAX_ATTACHMENT_BYTES} bytes."
                    ));
                    return;
                }
                if bytes.iter().take(2048).any(|byte| *byte == 0) {
                    self.report_error(format!("{path} looks like a binary file."));
                    return;
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let text: String = text.chars().take(MAX_ATTACHMENT_CHARS).collect();
                self.attachments.push(Attachment {
                    path: path.to_owned(),
                    text,
                    image: None,
                });
            }
            Err(error) => self.report_error(format!("Cannot read {path}: {error}")),
        }
    }

    /// The images the next message carries.
    fn attached_images(&self) -> Vec<ImagePart> {
        self.attachments
            .iter()
            .filter_map(|attachment| attachment.image.clone())
            .collect()
    }

    /// The user message with attachments and the terminal tail folded in.
    fn message_with_attachments(&self, text: &str, terminal: Option<&str>) -> String {
        let mut message = String::new();
        for attachment in &self.attachments {
            if attachment.image.is_some() {
                message.push_str(&format!("[Attached image {}]\n", attachment.name()));
                continue;
            }
            message.push_str(&format!(
                "[Attached file {}]\n{}\n\n",
                attachment.name(),
                attachment.text
            ));
        }
        message.push_str(text);
        if let Some(tail) = terminal {
            let tail = tail_chars(tail, TERMINAL_CONTEXT_CHARS);
            message.push_str(&format!("\n\n{TERMINAL_CONTEXT_MARKER}\n"));
            message.push_str(if tail.trim().is_empty() {
                TERMINAL_CONTEXT_EMPTY
            } else {
                tail
            });
        }
        message
    }

    /// Run a slash command.
    fn run_command(
        &mut self,
        command: SlashCommand,
        settings: &serde_json::Map<String, Value>,
        host: &str,
    ) {
        self.draft.clear();
        match command.name {
            "new" => self.begin_new_chat(),
            "compact" => self.compact_now(settings, host),
            "plan" => self.agent.plan = true,
            "agent" => self.agent.plan = false,
            "summary" | "context" => self.start_summary(SummaryMode::Summary, 0, settings, host),
            "transfer" => self.start_summary(SummaryMode::Transfer, 0, settings, host),
            _ => {}
        }
    }

    /// Keep the parsed Markdown of each turn in step with the transcript.
    ///
    /// Only text that changed is parsed again: a settled message is drawn from what an
    /// earlier frame parsed, and a streaming reply re-parses the one turn that is growing.
    fn parse_transcript(&mut self, source: TranscriptSource) {
        let turns: &[Turn] = match source {
            TranscriptSource::Main => &self.agent.turns,
            TranscriptSource::Subagent(index) => &self.agent.subagents[index].agent.turns,
        };
        let cache = match source {
            TranscriptSource::Main => &mut self.parsed,
            TranscriptSource::Subagent(_) => &mut self.parsed_sub,
        };
        for (index, turn) in turns.iter().enumerate() {
            let text = match turn {
                Turn::User { text, .. } | Turn::Assistant { text, .. } => text.as_str(),
                _ => "",
            };
            let fingerprint = super::markdown::fingerprint(text);
            if cache
                .get(index)
                .is_some_and(|parsed| parsed.fingerprint == fingerprint)
            {
                continue;
            }
            let parsed = ParsedTurn {
                fingerprint,
                blocks: super::markdown::blocks(text),
            };
            match cache.get_mut(index) {
                Some(cached) => *cached = parsed,
                None => cache.push(parsed),
            }
        }
        cache.truncate(turns.len());
    }

    /// The parsed Markdown of one turn of the conversation on screen.
    fn blocks(&self, source: TranscriptSource, index: usize) -> &[Block] {
        match source {
            TranscriptSource::Main => &self.parsed[index].blocks,
            TranscriptSource::Subagent(_) => &self.parsed_sub[index].blocks,
        }
    }

    /// Paint the transcript, answering where each user bubble landed on screen.
    fn transcript(
        &mut self,
        ui: &mut Ui,
        palette: &wassh_core::Palette,
        muted: egui::Color32,
        session: &mut Option<&mut dyn crate::plugin::SessionAccess>,
        source: TranscriptSource,
    ) -> TranscriptReply {
        self.parse_transcript(source);
        // A sub-agent's transcript is read-only: nothing here is edited, stopped or replayed.
        let interactive = source == TranscriptSource::Main;
        let agent = match source {
            TranscriptSource::Main => &self.agent,
            TranscriptSource::Subagent(index) => &self.agent.subagents[index].agent,
        };
        if agent.turns.is_empty() {
            let empty = if self.providers.is_empty() {
                "No model providers configured yet. Configure one in Options."
            } else if interactive {
                "Ask the agent to help with this session. It can use terminal tools when you approve them."
            } else {
                "This sub-agent has not written anything yet."
            };
            ui.add(egui::Label::new(egui::RichText::new(empty).size(12.0).color(muted)).wrap());
        }
        let mut open = None;
        let mut edit_action = EditAction::None;
        let editing = interactive
            .then(|| self.editing.as_ref().map(|edit| edit.index))
            .flatten();
        let mut bubbles = Vec::new();
        let mut stop_command = false;
        let mut copy_output = None;
        let running = agent.running_command();
        for (index, turn) in agent.turns.iter().enumerate() {
            match turn {
                Turn::User { meta, .. } => {
                    if editing == Some(index)
                        && let Some(edit) = self.editing.as_mut()
                    {
                        edit_action = prompt_editor(ui, palette, edit);
                    } else {
                        let reply = user_bubble(ui, palette, self.blocks(source, index), *meta);
                        if reply.clicked {
                            open = Some(index);
                        }
                        bubbles.push((index, reply.rect));
                    }
                }
                Turn::Assistant { text, stopped } => {
                    // A reply waiting for its first token, or one that only asked for
                    // tools, has no words to show: no bubble for it.
                    if !text.trim().is_empty() {
                        assistant_bubble(ui, palette, self.blocks(source, index));
                    }
                    if *stopped {
                        ui.label(
                            egui::RichText::new("interrupted")
                                .size(11.0)
                                .italics()
                                .color(muted),
                        );
                    }
                }
                Turn::Tool {
                    name,
                    label,
                    output,
                    failed,
                } => {
                    let reply = tool_card(
                        ui,
                        palette,
                        name,
                        label,
                        output,
                        ToolCardState {
                            failed: *failed,
                            running: running == Some(index),
                            turn_index: index,
                            copied: self.copied_recently(index),
                        },
                    );
                    stop_command |= reply.stop;
                    if let Some((target, text)) = reply.copy {
                        copy_output = Some((index, target, text));
                    }
                }
                Turn::Script {
                    code,
                    calls,
                    result,
                    failed,
                    console,
                } => {
                    let script_running = matches!(
                        agent.phase,
                        Some(Phase::Running {
                            turn,
                            waiting: Waiting::Script,
                            ..
                        }) if turn == index
                    );
                    if script_card(
                        ui,
                        palette,
                        ScriptCardState {
                            code,
                            calls,
                            result: result.as_deref(),
                            failed: *failed,
                            console,
                            running: script_running,
                            turn_index: index,
                        },
                    ) {
                        stop_command = true;
                    }
                }
                Turn::Denied(reason) => {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("⊘ {reason}"))
                                .size(11.5)
                                .color(to_color32(palette.danger)),
                        )
                        .wrap(),
                    );
                }
                Turn::Question { text, answer } => {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("? {text}"))
                                .size(12.0)
                                .color(to_color32(palette.text)),
                        )
                        .wrap(),
                    );
                    if let Some(answer) = answer {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(answer.as_str())
                                    .size(11.5)
                                    .color(to_color32(palette.accent)),
                            )
                            .wrap(),
                        );
                    }
                }
                Turn::Error(text) => {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("⚠ {text}"))
                                .size(11.5)
                                .color(to_color32(palette.danger)),
                        )
                        .wrap(),
                    );
                }
                Turn::Notice(text) => {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text.as_str())
                                .size(11.5)
                                .italics()
                                .color(muted),
                        )
                        .wrap(),
                    );
                }
            }
            ui.add_space(TURN_GAP);
        }
        // The reference shows this until the first token of the reply arrives.
        if interactive && self.waiting_for_the_first_token() {
            let step = (self.agent.now_ms / THINKING_DOT_MS) as usize;
            let dots: String = ".".repeat(1 + step % THINKING_DOT_COUNT);
            ui.label(
                egui::RichText::new(format!("Thinking{dots}"))
                    .size(12.0)
                    .italics()
                    .color(muted),
            );
        }
        // Clicking a message opens it for editing; the edit itself is sent by the pane.
        if interactive {
            if let Some(index) = open {
                self.edit_prompt(index);
            }
        } else {
            edit_action = EditAction::None;
        }
        let submitted = match edit_action {
            EditAction::Cancel => {
                self.editing = None;
                None
            }
            EditAction::Submit => self.editing.take().map(|edit| (edit.index, edit.text)),
            EditAction::None => None,
        };
        if stop_command && interactive {
            // A command the script is waiting on is cancelled first, so it does not keep
            // going on the host after the run is over.
            self.agent.stop_command(session);
            if matches!(
                self.agent.phase,
                Some(Phase::Running {
                    waiting: Waiting::Script,
                    ..
                })
            ) {
                self.agent.stop();
            }
        }
        if let Some((index, target, text)) = copy_output {
            ui.ctx().copy_text(text);
            self.mark_copied(index, target);
            // Come back once the tick has to turn back into a copy glyph.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(
                COPY_FEEDBACK_MS,
            ));
        }
        TranscriptReply {
            bubbles,
            submitted,
        }
    }

    /// Remember that a tool card's copy button confirmed.
    fn mark_copied(&mut self, turn_index: usize, target: CopyTarget) {
        self.copied = Some((turn_index, target, self.agent.now_ms));
    }

    /// The copy button of `turn_index` that is still showing its confirmation, if any.
    fn copied_recently(&self, turn_index: usize) -> Option<CopyTarget> {
        let (saved, target, at) = self.copied?;
        (saved == turn_index && self.agent.now_ms.saturating_sub(at) < COPY_FEEDBACK_MS)
            .then_some(target)
    }

    /// Open the prompt at `index` in the transcript, ready to be rewritten.
    fn edit_prompt(&mut self, index: usize) {
        let Some(Turn::User { text, .. }) = self.agent.turns.get(index) else {
            return;
        };
        if self.agent.blocked() {
            self.report_error(EDIT_WHILE_BUSY);
            return;
        }
        self.editing = Some(MessageEdit {
            index,
            text: text.clone(),
            opening: true,
        });
    }

    /// Send `text` in place of the prompt at `index`, dropping everything after it.
    fn replace_prompt(
        &mut self,
        index: usize,
        text: &str,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self.agent.blocked() {
            self.report_error(EDIT_WHILE_BUSY);
            return;
        }
        if self.agent.rewind_to(index).is_none() {
            return;
        }
        self.editing = None;
        self.draft.clear();
        self.attachments.clear();
        self.attach_terminal = false;
        self.queued = None;
        // The files and terminal tail the first version carried are not in the transcript,
        // so the edited prompt goes out as written.
        self.send_message(text, UserMeta::default(), Vec::new(), settings, host, environment);
    }

    /// Draw the popup a tag opened: what the scope is, the tools it carries, and the way to
    /// turn it off.
    fn scope_popover(&mut self, ui: &mut Ui, palette: &Palette) {
        let Some(popup) = self.scope_popup.as_ref() else {
            return;
        };
        let Some(scope) = scopes::find(popup.id) else {
            self.scope_popup = None;
            return;
        };
        let anchor = popup.anchor;
        let loaded = self.agent.scopes.contains(scope.id);
        let tools = self.scope_tool_names(scope);
        let mut turn_off = false;
        let mut scan = false;
        let mut deep = false;
        let mut clear = false;
        let mut more = false;
        let area = egui::Area::new(egui::Id::unique(("ai-scope-popup", scope.id)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_bottom() + egui::Vec2::new(0.0, SCOPE_POPUP_GAP))
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(to_color32(palette.bg_panel))
                    .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
                    .corner_radius(egui::CornerRadius::same(RADIUS as u8))
                    .inner_margin(egui::Margin::same(SCOPE_POPUP_PADDING))
                    .show(ui, |ui| {
                        ui.set_width(SCOPE_POPUP_WIDTH);
                        ui.spacing_mut().item_spacing.y = SCOPE_POPUP_GAP;
                        ui.label(
                            egui::RichText::new(if scope.id == ENV_SCOPE {
                                GRAPH_SCOPE_LABEL
                            } else {
                                scope.id
                            })
                                .size(12.0)
                                .strong()
                                .color(to_color32(palette.text)),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(scope.summary)
                                    .size(11.0)
                                    .color(to_color32(palette.text_muted)),
                            )
                            .wrap(),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "{} {}",
                                    if loaded {
                                        "Loaded, carrying"
                                    } else {
                                        "Not loaded. Would carry"
                                    },
                                    tools.len()
                                ))
                                .size(10.0)
                                .color(to_color32(palette.text_muted)),
                            )
                            .wrap(),
                        );
                        if !tools.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(tools.join(", "))
                                        .size(10.0)
                                        .color(to_color32(palette.accent)),
                                )
                                .wrap(),
                            );
                        }
                        if scope.id == ENV_SCOPE {
                            ui.separator();
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(self.map_summary())
                                        .size(10.0)
                                        .color(to_color32(palette.text_muted)),
                                )
                                .wrap(),
                            );
                            if self.map_deep.is_some() {
                                more = Button::new(if self.map_more { "Less" } else { "More" })
                                    .compact()
                                    .ghost()
                                    .show(ui, palette)
                                    .clicked();
                                if self.map_more {
                                    for line in self.map_detail_lines() {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(line)
                                                    .size(10.0)
                                                    .color(to_color32(palette.text_muted)),
                                            )
                                            .wrap(),
                                        );
                                    }
                                }
                            }
                            ui.horizontal(|ui| {
                                scan = Button::new("Scan")
                                    .compact()
                                    .enabled(self.agent.map_scan.is_none())
                                    .show(ui, palette)
                                    .clicked();
                                deep = Button::new("Map project")
                                    .compact()
                                    .enabled(
                                        self.map_deep.is_none() && self.agent.map_scan.is_none(),
                                    )
                                    .show(ui, palette)
                                    .clicked();
                                clear = Button::new("Clear")
                                    .compact()
                                    .enabled(!self.agent.map.is_empty())
                                    .show(ui, palette)
                                    .clicked();
                            });
                        }
                        if loaded {
                            ui.separator();
                            turn_off = Button::new("Turn off").compact().show(ui, palette).clicked();
                        }
                    });
            });
        if turn_off {
            let _ = self.agent.unload_scope(scope.id);
        }
        if clear {
            self.agent.map.clear();
        }
        if scan {
            self.map_rebuild = true;
        }
        if more {
            self.map_more = !self.map_more;
        }
        if deep {
            // A map that is not there yet means the cheap walk comes first; the description
            // follows it.
            if self.agent.map.is_empty() {
                self.map_rebuild = true;
            }
            self.map_deep_wanted = true;
        }
        let rect = area.response.rect;
        let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
        let pressed_outside = ui.input(|input| {
            input
                .pointer
                .any_pressed()
                .then(|| input.pointer.interact_pos())
                .flatten()
                .is_some_and(|pos| !rect.contains(pos) && !anchor.contains(pos))
        });
        if turn_off || clear || escape || pressed_outside {
            self.scope_popup = None;
        }
    }

    /// Run the mapping pass a tool answer asked for, and take in the one that finished.
    ///
    /// The files are already listed cheaply; this spends a sub-agent on describing what is
    /// in them and what they connect to, so later questions cost neither host calls nor
    /// tokens. One pass at a time, and only while a model is configured.
    fn poll_map_deep(
        &mut self,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        if let Some(run) = &self.map_deep {
            let finished = self
                .agent
                .subagents
                .get(run.subagent)
                .is_some_and(|sub| sub.end.is_some());
            if finished {
                let run = self.map_deep.take().unwrap_or_else(|| unreachable!("just matched"));
                let answer = self
                    .agent
                    .subagents
                    .get(run.subagent)
                    .map(|sub| sub.answer_full())
                    .unwrap_or_default();
                let files = super::map::parse_mapped(&answer);
                if files.is_empty() {
                    self.report_map_empty(&run, &answer, settings, host, environment);
                    return;
                }
                let described = self
                    .agent
                    .map
                    .merge_summaries(&run.root, self.agent.now_ms, &files);
                if described > 0 {
                    let notice = MAP_READY_NOTICE
                        .replace("{described}", &described.to_string())
                        .replace("{root}", &run.root);
                    self.show_map_ready(&notice);
                    // A run in flight carries the news into its next request.
                    if self.agent.blocked() {
                        self.agent.inject_user(&notice, UserMeta::default());
                    }
                }
            }
        }
        // A pass cannot start over a map that is not there yet or one that is being walked,
        // so what asked for it waits rather than being spent on nothing.
        if self.map_deep.is_some() || self.agent.map_scan.is_some() || self.agent.map.is_empty() {
            return;
        }
        let wanted =
            std::mem::take(&mut self.map_deep_wanted) || self.agent.take_deep_map_wanted();
        if !wanted {
            return;
        }
        let root = match self.agent.map.root.is_empty() {
            true => self.agent.cwd.clone(),
            false => self.agent.map.root.clone(),
        };
        let prompt = MAP_TASK
            .replace("{root}", &root)
            .replace("{files}", &super::map::MAX_MAPPED_FILES.to_string());
        let index = self.agent.begin_subagent(MAP_SUBAGENT_TITLE, &prompt);
        match self.request_for(Some(index), settings, host, environment) {
            Some(request) => {
                if let Some(sub) = self.agent.subagents.get_mut(index) {
                    sub.agent.start(request);
                }
                self.map_deep = Some(MapRun {
                    subagent: index,
                    root,
                    started: self.agent.now_ms,
                    attempts: 1,
                });
            }
            None => {
                // No model to do it; the next tool call asks again once one is configured.
                self.agent.subagents.pop();
            }
        }
    }

    /// Say in the transcript why a mapping pass described nothing, asking the model again
    /// while it still has an attempt left.
    fn report_map_empty(
        &mut self,
        run: &MapRun,
        answer: &str,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        let answered = self
            .agent
            .subagents
            .get(run.subagent)
            .is_some_and(|sub| sub.state() == SubagentState::Done);
        if answered && run.attempts < MAP_ATTEMPTS {
            if let Some(sub) = self.agent.subagents.get_mut(run.subagent) {
                sub.end = None;
                sub.agent.push_user(MAP_REPAIR_TASK, UserMeta::default());
            }
            let request = self.request_for(Some(run.subagent), settings, host, environment);
            if let Some(request) = request {
                self.agent.subagents[run.subagent].agent.start(request);
                self.map_deep = Some(MapRun {
                    subagent: run.subagent,
                    root: run.root.clone(),
                    started: run.started,
                    attempts: run.attempts + 1,
                });
                self.show_map_ready(MAP_RETRY_NOTICE);
                return;
            }
        }
        let said = answer.lines().next().unwrap_or_default().trim();
        let notice = match said.is_empty() {
            true => MAP_EMPTY_NOTICE.to_owned(),
            false => format!("{MAP_EMPTY_NOTICE} It said: {said}"),
        };
        self.show_map_ready(&notice);
    }

    /// Report a mapping pass in the transcript, in place of the sub-agent's raw JSON answer.
    fn show_map_ready(&mut self, notice: &str) {
        let finished = format!("Sub-agent \"{MAP_SUBAGENT_TITLE}\" finished:");
        let raw = self.agent.turns.iter_mut().rev().find_map(|turn| match turn {
            Turn::Notice(text) if text.starts_with(&finished) => Some(text),
            _ => None,
        });
        match raw {
            Some(text) => *text = notice.to_owned(),
            None => self.agent.turns.push(Turn::Notice(notice.to_owned())),
        }
    }

    /// One line about the map for its scope popup.
    fn map_summary(&self) -> String {
        if let Some(root) = &self.agent.map_scan {
            return match root.trim().is_empty() {
                true => "Scanning the project root the host finds…".to_owned(),
                false => format!("Scanning {root}…"),
            };
        }
        if let Some(run) = &self.map_deep {
            let doing = match self.agent.subagents.get(run.subagent) {
                Some(sub) => map_activity(sub),
                None => "finishing".to_owned(),
            };
            return format!("Describing {}: {doing}", run.root);
        }
        if self.agent.map.is_empty() {
            return format!(
                "Nothing mapped yet. A scan starts at the project root above {}.",
                self.agent.cwd
            );
        }
        let map = &self.agent.map;
        let described = match map.summarised() {
            0 => "no summaries".to_owned(),
            count => format!("{count} described"),
        };
        format!(
            "Graph: {} files under {} ({}, {described}), built {} min ago",
            map.len(),
            map.root,
            if map.truncated { "capped" } else { "complete" },
            map.age_minutes(self.agent.now_ms)
        )
    }

    /// What the mapping pass has done so far, newest last, for the popup's detail view.
    fn map_detail_lines(&self) -> Vec<String> {
        let Some(run) = &self.map_deep else {
            return Vec::new();
        };
        let Some(sub) = self.agent.subagents.get(run.subagent) else {
            return Vec::new();
        };
        let mut calls = 0;
        let mut lines = Vec::new();
        for turn in &sub.agent.turns {
            if let Turn::Tool {
                name, label, failed, ..
            } = turn
            {
                calls += 1;
                let text = if label.is_empty() { name } else { label };
                lines.push(if *failed {
                    format!("{text} (failed)")
                } else {
                    text.clone()
                });
                if lines.len() > MAP_DETAIL_LINES {
                    lines.remove(0);
                }
            }
        }
        let count = match calls {
            1 => "1 call".to_owned(),
            total => format!("{total} calls"),
        };
        let elapsed = elapsed_text(self.agent.now_ms.saturating_sub(run.started));
        lines.insert(0, format!("{count} in {elapsed}"));
        lines
    }

    /// The tools `scope` holds that this host may call, loaded or not.
    fn scope_tool_names(&self, scope: &scopes::Scope) -> Vec<&'static str> {
        let mut names = scope.tools.to_vec();
        if let Some(pane) = scope.pane {
            names.extend(crate::plugin::tools_of(pane).iter().map(|tool| tool.name));
        }
        names.retain(|name| self.agent.permissions.offers(name));
        names
    }

    /// Draw the list of earlier chats, newest first, and open the one the user picks.
    fn chat_list(&mut self, ui: &mut Ui, palette: &wassh_core::Palette, muted: egui::Color32) {
        if self.chats.is_empty() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(CHAT_LIST_EMPTY)
                        .size(CHAT_TITLE_TEXT_SIZE)
                        .color(muted),
                )
                .wrap(),
            );
            return;
        }
        let mut open = None;
        let mut open_subagent = None;
        let mut toggled = None;
        egui::ScrollArea::vertical()
            .id_salt("ai-chats")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for index in chat_order(&self.chats) {
                    let chat = &self.chats[index];
                    let expanded = self.expanded_chats.contains(&chat.created);
                    let row = chat_row(ui, palette, chat, expanded);
                    if row.open {
                        open = Some(index);
                    }
                    if row.toggle {
                        toggled = Some(chat.created);
                    }
                    if let Some(position) = row.subagent {
                        open_subagent = Some((index, position));
                    }
                }
            });
        if let Some(created) = toggled
            && !self.expanded_chats.remove(&created)
        {
            self.expanded_chats.insert(created);
        }
        if let Some((index, subagent)) = open_subagent {
            // Opening the parent puts its sub-agents back in the pane, in the same order.
            self.open_chat(index);
            if subagent < self.agent.subagents.len() {
                self.view = ChatView::Subagent(subagent);
            }
        } else if let Some(index) = open {
            self.open_chat(index);
        }
    }

    /// Draw the newest message that scrolled past the top of `viewport` again, pinned there.
    ///
    /// A pinned copy is opaque, so the transcript scrolls under it. Coming back into view
    /// releases it, and the message before it takes its place.
    fn pinned_bubble(
        &mut self,
        ui: &mut Ui,
        palette: &wassh_core::Palette,
        viewport: egui::Rect,
        bubbles: &[(usize, egui::Rect)],
    ) -> Option<PinnedBubble> {
        self.parse_transcript(TranscriptSource::Main);
        let (index, _) = bubbles
            .iter()
            .rev()
            .find(|(_, rect)| rect.top() < viewport.top())?;
        let Some(Turn::User { meta, .. }) = self.agent.turns.get(*index) else {
            return None;
        };
        let mut pinned = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(viewport)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        pinned.set_clip_rect(viewport);
        let clicked = egui::Frame::new()
            .fill(to_color32(palette.bg_panel))
            .show(&mut pinned, |ui| {
                // The backdrop spans the whole transcript, so nothing shows beside the
                // pinned bubble where it scrolls underneath.
                ui.set_min_width(ui.available_width());
                user_bubble(ui, palette, &self.parsed[*index].blocks, *meta).clicked
            })
            .inner;
        Some(PinnedBubble {
            index: *index,
            clicked,
        })
    }
}

/// The button that leaves a chat for the list.
///
/// The arrow is drawn rather than typed: a glyph like "◀" comes out of whichever font
/// claims it, which is how it ends up in the wrong size and colour.
fn back_button(ui: &mut Ui, palette: &Palette) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::splat(BACK_BUTTON_SIZE),
        egui::Sense::click(),
    );
    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, RADIUS, to_color32(palette.bg_panel_2));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let colour = to_color32(if hovered {
        palette.text
    } else {
        palette.text_muted
    });
    let stroke = egui::Stroke::new(BACK_ARROW_WIDTH, colour);
    let centre = rect.center();
    let tip = centre - egui::Vec2::new(BACK_ARROW_SHAFT, 0.0);
    ui.painter().line_segment(
        [centre + egui::Vec2::new(BACK_ARROW_SHAFT, 0.0), tip],
        stroke,
    );
    for side in [-1.0, 1.0] {
        ui.painter().line_segment(
            [tip, tip + egui::Vec2::new(BACK_ARROW_HEAD, side * BACK_ARROW_HEAD)],
            stroke,
        );
    }
    response.clicked()
}

/// One loaded scope, as a small quiet tag beside the composer.
///
/// Neutral rather than accent-filled: a loaded scope is context the conversation carries,
/// not a button, so only hovering gives it a surface.
fn scope_tag(ui: &mut Ui, palette: &Palette, label: &str) -> egui::Response {
    let font = egui::FontId::proportional(SCOPE_TAG_TEXT_SIZE);
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), to_color32(palette.text))
        .size()
        .x;
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::new(text_width + SCOPE_TAG_PADDING * 2.0, SCOPE_TAG_HEIGHT),
        egui::Sense::click(),
    );
    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, RADIUS, to_color32(palette.bg_panel_2));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_stroke(
        rect,
        RADIUS,
        egui::Stroke::new(
            1.0,
            to_color32(if hovered {
                palette.text_muted
            } else {
                palette.border
            }),
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        to_color32(if hovered {
            palette.text
        } else {
            palette.text_muted
        }),
    );
    response
}

/// One sub-agent card under its chat: name, state and how much it has written.
///
/// The card it sits in decides what a click means, so this paints only and reports where it
/// landed: a row inside a clickable card cannot win the pointer on its own.
fn subagent_row(ui: &mut Ui, palette: &Palette, sub: &Subagent) -> egui::Rect {
    let state = sub.state();
    let (badge, colour) = match state {
        SubagentState::Running => ("running", palette.accent),
        SubagentState::NeedsAnswer => ("needs answer", palette.danger),
        SubagentState::Done => ("done", palette.text_muted),
        SubagentState::Failed => ("failed", palette.danger),
        SubagentState::Stopped => ("stopped", palette.text_muted),
    };
    let rect = egui::Frame::NONE
        .fill(to_color32(palette.bg_panel))
        .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::symmetric(CHAT_ROW_PADDING as i8, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("\u{21b3} {}", sub.title))
                        .size(CHAT_TITLE_TEXT_SIZE)
                        .color(to_color32(palette.text)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(badge).size(10.0).color(to_color32(colour)));
                });
            });
        })
        .response
        .rect;
    ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), 4.0),
        egui::Sense::hover(),
    );
    rect
}

/// What one chat card in the list reported.
#[derive(Default)]
struct ChatRowReply {
    /// The card itself was clicked: open the chat.
    open: bool,
    /// The sub-agent note was clicked: show or hide the sub-agent cards.
    toggle: bool,
    /// One of the expanded sub-agent cards was clicked.
    subagent: Option<usize>,
}

/// One chat card: title, a compact stats line, and its sub-agents behind a note.
///
/// A chat with sub-agents says how many there are in one clickable line instead of listing
/// them: the list stays one card per conversation until the user asks to look inside.
fn chat_row(ui: &mut Ui, palette: &Palette, chat: &StoredChat, expanded: bool) -> ChatRowReply {
    let mut reply = ChatRowReply::default();
    let mut note_rect = None;
    let mut sub_rects: Vec<(usize, egui::Rect)> = Vec::new();
    let response = egui::Frame::NONE
        .fill(to_color32(palette.bg_panel_2))
        .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::symmetric(CHAT_ROW_PADDING as i8, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(&chat.title)
                        .size(CHAT_TITLE_TEXT_SIZE)
                        .strong()
                        .color(to_color32(palette.text)),
                );
                ui.add_space(4.0);
                let turns = chat.turns.len();
                let model = if chat.model.trim().is_empty() {
                    "No model"
                } else {
                    chat.model.as_str()
                };
                let cwd = if chat.cwd.trim().is_empty() {
                    "/"
                } else {
                    chat.cwd.as_str()
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{} turn{} · {} · {}",
                        turns,
                        if turns == 1 { "" } else { "s" },
                        model,
                        cwd
                    ))
                    .size(10.0)
                    .color(to_color32(palette.text_muted)),
                );
                if chat.subagents.is_empty() {
                    return;
                }
                ui.add_space(4.0);
                let note = format!(
                    "{} sub-agent{}",
                    chat.subagents.len(),
                    if chat.subagents.len() == 1 { "" } else { "s" }
                );
                note_rect = Some(
                    ui.label(
                        egui::RichText::new(note)
                            .size(10.0)
                            .color(to_color32(palette.accent)),
                    )
                    .rect,
                );
                if !expanded {
                    return;
                }
                ui.add_space(4.0);
                for (position, sub) in chat.subagents.iter().enumerate() {
                    sub_rects.push((position, subagent_row(ui, palette, sub)));
                }
            });
        })
        .response;
    ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), 8.0),
        egui::Sense::hover(),
    );
    // The card is clickable as a whole, which puts it above the note and the sub-agent cards
    // in the pointer order, so where the press landed decides what it meant.
    let clicked = response.interact(egui::Sense::click()).clicked();
    let pointer = ui.ctx().input(|input| input.pointer.interact_pos());
    if let Some(pos) = pointer {
        let over_note = note_rect.is_some_and(|rect| rect.contains(pos));
        let over_sub = sub_rects.iter().find(|(_, rect)| rect.contains(pos));
        if over_note || over_sub.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if clicked {
            match (over_note, over_sub) {
                (true, _) => reply.toggle = true,
                (false, Some((position, _))) => reply.subagent = Some(*position),
                _ => reply.open = true,
            }
        }
    }
    reply
}

/// The answer a title worker has given, once it has one.
fn title_answer(job: &TitleJob) -> Option<Result<String, String>> {
    match job.try_recv() {
        Ok(answer) => Some(answer),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            Some(Err("The chat title worker stopped.".to_owned()))
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
    }
}

/// The title a model's answer asks for, tidied and cut to length.
fn digest_title(answer: Result<String, String>) -> Option<String> {
    let answer = answer.ok()?;
    let title = answer
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches(['"', '\''])
        .trim();
    if title.is_empty() {
        return None;
    }
    let too_long = title.chars().count() > TITLE_MAX_CHARS;
    let mut title: String = title.chars().take(TITLE_MAX_CHARS).collect();
    if too_long {
        title.push('…');
    }
    Some(title)
}

/// The first prompt of a conversation.
fn first_prompt(turns: &[Turn]) -> Option<&str> {
    turns.iter().find_map(|turn| match turn {
        Turn::User { text, .. } if !text.trim().is_empty() => Some(text.trim()),
        _ => None,
    })
}

/// A title as the pane used to take it from the prompt, so a stored one is told apart
/// from a digest and asked for again.
fn prompt_title(turns: &[Turn]) -> Option<String> {
    let prompt = first_prompt(turns)?;
    let too_long = prompt.chars().count() > TITLE_MAX_CHARS;
    let mut title: String = prompt.chars().take(TITLE_MAX_CHARS).collect();
    if too_long {
        title.push('…');
    }
    Some(title)
}

/// Whether a stored title came from the model rather than from the first prompt.
fn is_digest(stored: &str, turns: &[Turn]) -> bool {
    !stored.trim().is_empty()
        && stored != DEFAULT_CHAT_TITLE
        && Some(stored) != prompt_title(turns).as_deref()
}

/// The chat list's order: creation order, newest first, so reopening a chat does not move
/// it up.
fn chat_order(chats: &[StoredChat]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..chats.len()).collect();
    order.sort_by_key(|&index| (chats[index].created, index));
    order.reverse();
    order
}

/// What a mapping sub-agent is doing now, in one line.
fn map_activity(sub: &Subagent) -> String {
    match sub.state() {
        SubagentState::NeedsAnswer => return "waiting for your answer".to_owned(),
        SubagentState::Running => {}
        state => return state.label().to_owned(),
    }
    match &sub.agent.phase {
        Some(Phase::Running { turn, .. }) => match sub.agent.turns.get(*turn) {
            Some(Turn::Tool { label, .. }) if !label.is_empty() => label.clone(),
            Some(Turn::Tool { name, .. }) => name.clone(),
            _ => "running a tool".to_owned(),
        },
        Some(Phase::Streaming { .. }) => "writing its answer".to_owned(),
        _ => "working".to_owned(),
    }
}

/// How long something took, as the popup says it.
fn elapsed_text(ms: u64) -> String {
    let seconds = ms / MILLIS_PER_SECOND;
    match seconds / SECONDS_PER_MINUTE {
        0 => format!("{seconds} s"),
        minutes => format!("{minutes} min"),
    }
}

/// A dropdown that never grows past `width`: a long label is cropped rather than
/// pushing whatever follows it out of the row.
fn cropped_select<T: Clone + PartialEq>(
    ui: &mut Ui,
    id: &str,
    selected: &T,
    options: &[(T, &str)],
    width: f32,
) -> Option<T> {
    let height = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(width, height), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.shrink_clip_rect(rect);
    select_width(&mut child, id, selected, options, width)
}

/// What a drawn user bubble reports back.
struct BubbleReply {
    /// The bubble was clicked, asking to edit the prompt.
    clicked: bool,
    /// Where the bubble landed on screen.
    rect: egui::Rect,
}

/// A message drawn again at the top of the transcript.
struct PinnedBubble {
    /// Index of the message in the transcript.
    index: usize,
    /// The pinned copy was clicked, asking to edit the prompt.
    clicked: bool,
}

/// What the transcript reports after a frame: where each prompt landed, and the edited
/// prompt the pane still has to send.
#[derive(Default)]
struct TranscriptReply {
    /// Screen rect of every user bubble drawn.
    bubbles: Vec<(usize, egui::Rect)>,
    /// The edited prompt waiting to go out, as turn index and text.
    submitted: Option<(usize, String)>,
}

/// A prompt opened for editing in the transcript.
struct MessageEdit {
    /// Turn index of the message being edited.
    index: usize,
    /// What the editor holds.
    text: String,
    /// Whether the editor still has to be focused and scrolled into view.
    opening: bool,
}

/// What the prompt editor did this frame.
enum EditAction {
    /// Still editing.
    None,
    /// Put the message back as it was.
    Cancel,
    /// Send the edited prompt in place of the old one.
    Submit,
}

/// A prompt opened for editing: the same accent bubble, holding a text field.
///
/// Enter sends the edited prompt, Escape puts the message back. The cue underneath says
/// so, because neither key is discoverable otherwise.
fn prompt_editor(ui: &mut Ui, palette: &Palette, edit: &mut MessageEdit) -> EditAction {
    let room = ui.available_width() * MESSAGE_BUBBLE_SHARE;
    let id = ui.make_persistent_id(("ai-prompt-editor", edit.index));
    let mut action = EditAction::None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
        egui::Frame::new()
            .fill(to_color32(palette.accent))
            .corner_radius(wassh_core::RADIUS)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_max_width(room);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    let rows = edit
                        .text
                        .lines()
                        .count()
                        .clamp(EDITOR_MIN_ROWS, EDITOR_MAX_ROWS);
                    let response =
                        text_area_with_id(ui, palette, &mut edit.text, rows, "Prompt", id);
                    if edit.opening {
                        response.request_focus();
                        ui.scroll_to_rect(response.rect, Some(egui::Align::TOP));
                        edit.opening = false;
                    }
                    if ui.memory(|memory| memory.has_focus(id))
                        && ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                        })
                    {
                        action = EditAction::Submit;
                    }
                    if ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                    }) {
                        action = EditAction::Cancel;
                    }
                    ui.label(
                        egui::RichText::new(EDITOR_HINT)
                            .size(9.5)
                            .color(egui::Color32::from_white_alpha(USER_BUBBLE_TAG_ALPHA)),
                    );
                });
            });
    });
    action
}

/// A user message: a bubble against the right edge, filled with the accent like a primary
/// button and carrying white ink. Clicking it opens the prompt for editing.
fn user_bubble(ui: &mut Ui, palette: &Palette, blocks: &[Block], meta: UserMeta) -> BubbleReply {
    let room = ui.available_width() * MESSAGE_BUBBLE_SHARE;
    let response = ui
        .with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            egui::Frame::new()
                .fill(to_color32(palette.accent))
                .corner_radius(wassh_core::RADIUS)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_max_width(room);
                    // The prompt is a block that opens an editor, not text to select: no
                    // text cursor, no selection, until it has been clicked.
                    ui.style_mut().interaction.selectable_labels = false;
                    // The frame hugs the right edge while its text stays left-aligned.
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        // What went out with the message, like the reference's ctx tags.
                        if meta.attachments > 0 || meta.terminal {
                            let mut tags: Vec<String> = Vec::new();
                            if meta.attachments > 0 {
                                let plural = if meta.attachments == 1 { "" } else { "s" };
                                tags.push(format!("+{} file{plural}", meta.attachments));
                            }
                            if meta.terminal {
                                tags.push("+terminal".to_owned());
                            }
                            ui.label(
                                egui::RichText::new(tags.join("  "))
                                    .size(9.5)
                                    .color(egui::Color32::from_white_alpha(USER_BUBBLE_TAG_ALPHA)),
                            );
                        }
                        super::markdown::draw(ui, Ink::on_accent(), blocks);
                    });
                })
                .response
        })
        .inner;
    BubbleReply {
        clicked: response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Click to edit this message")
            .clicked(),
        rect: response.rect,
    }
}

/// An assistant response: Markdown in a neutral bubble against the left edge.
fn assistant_bubble(ui: &mut Ui, palette: &Palette, blocks: &[Block]) {
    let room = ui.available_width() * MESSAGE_BUBBLE_SHARE;
    ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
        egui::Frame::new()
            .fill(to_color32(palette.bg_panel_2))
            .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
            .corner_radius(wassh_core::RADIUS)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_max_width(room);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    super::markdown::draw(ui, Ink::pane(palette), blocks);
                });
            });
    });
}

/// Which of a tool card's copy buttons a click belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyTarget {
    /// The command line the card ran.
    Command,
    /// What the command printed.
    Output,
}

/// One tool call: the reference's `.ai-agent-tool` card — a badge, the call, its outcome
/// and the output.
///
/// The card shows its header only until the user opens it: a transcript of several calls is
/// read, not scrolled through.
struct ToolCardState {
    failed: bool,
    running: bool,
    turn_index: usize,
    /// Copy button still showing its confirmation, if any.
    copied: Option<CopyTarget>,
}

/// What the pointer did to a tool card this frame.
struct ToolCardReply {
    stop: bool,
    /// The copy button that was clicked, with what it should put on the clipboard.
    copy: Option<(CopyTarget, String)>,
}

/// The parts of a `run_script` turn its card draws.
struct ScriptCardState<'a> {
    code: &'a str,
    calls: &'a [super::agent::ScriptRow],
    result: Option<&'a str>,
    failed: bool,
    console: &'a str,
    running: bool,
    turn_index: usize,
}

/// A `run_script` call: the program, the calls it made, and what came back.
///
/// One card rather than a card per inner call: a script may make dozens, and the transcript
/// is meant to be read, not scrolled through. It shows its header only until the user opens
/// it, like every other tool card.
fn script_card(ui: &mut Ui, palette: &Palette, state: ScriptCardState<'_>) -> bool {
    let ScriptCardState {
        code,
        calls,
        result,
        failed,
        console,
        running,
        turn_index,
    } = state;
    let muted = to_color32(palette.text_muted);
    let (outcome, outcome_colour) = if running {
        ("running", muted)
    } else if failed {
        ("failed", to_color32(palette.danger))
    } else {
        ("ok", to_color32(palette.ok))
    };
    let mut stop = false;
    egui::Frame::new()
        .fill(to_color32(palette.bg_input))
        .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
        .corner_radius(wassh_core::RADIUS)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                ui.make_persistent_id(("ai-script", turn_index)),
                false,
            )
            .show_header(ui, |ui| {
                badge_pill(ui, palette, SCRIPT_BADGE, palette.accent);
                ui.label(
                    egui::RichText::new(format!("{} calls", calls.len()))
                        .size(10.0)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(outcome)
                            .size(10.0)
                            .strong()
                            .color(outcome_colour),
                    );
                    if running
                        && Button::new("Stop")
                            .compact()
                            .kind(ButtonKind::Danger)
                            .show(ui, palette)
                            .clicked()
                    {
                        stop = true;
                    }
                });
            })
            .body(|ui| {
                script_card_body(ui, palette, turn_index, code, calls, result, console);
            });
        });
    stop
}

/// What an opened script card shows: the program, the calls it made, their result.
fn script_card_body(
    ui: &mut Ui,
    palette: &Palette,
    turn_index: usize,
    code: &str,
    calls: &[super::agent::ScriptRow],
    result: Option<&str>,
    console: &str,
) {
    let muted = to_color32(palette.text_muted);
    egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.make_persistent_id(("ai-script-code", turn_index)),
        false,
    )
    .show_header(ui, |ui| {
        ui.label(
            egui::RichText::new(SCRIPT_CODE_LABEL)
                .size(10.0)
                .color(muted),
        );
    })
    .body(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(code)
                    .size(SCRIPT_TEXT_SIZE)
                    .monospace()
                    .color(to_color32(palette.text_muted)),
            )
            .wrap(),
        );
    });
    for (call_index, row) in calls.iter().enumerate() {
        let (mark, mark_colour) = if row.failed {
            (SCRIPT_FAILED_MARK, to_color32(palette.danger))
        } else if row.output.is_empty() {
            (SCRIPT_PENDING_MARK, muted)
        } else {
            (SCRIPT_OK_MARK, to_color32(palette.ok))
        };
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id(("ai-script-call", turn_index, call_index)),
            false,
        )
        .show_header(ui, |ui| {
            ui.label(egui::RichText::new(mark).size(10.0).color(mark_colour));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(row.label.as_str())
                        .size(SCRIPT_TEXT_SIZE)
                        .monospace()
                        .color(to_color32(palette.text)),
                )
                .truncate(),
            );
        })
        .body(|ui| {
            tool_output(ui, palette, &card_preview(&row.output), false, false);
        });
    }
    if !console.trim().is_empty() {
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id(("ai-script-console", turn_index)),
            false,
        )
        .show_header(ui, |ui| {
            ui.label(
                egui::RichText::new(SCRIPT_CONSOLE_LABEL)
                    .size(10.0)
                    .color(muted),
            );
        })
        .body(|ui| {
            tool_output(ui, palette, &card_preview(console), false, false);
        });
    }
    if let Some(result) = result {
        ui.label(
            egui::RichText::new(SCRIPT_RESULT_LABEL)
                .size(10.0)
                .color(muted),
        );
        tool_output(ui, palette, &card_preview(result), false, false);
    }
}

/// The head of a long body, so a card cannot dominate the transcript.
fn card_preview(text: &str) -> String {
    if text.chars().count() <= TOOL_OUTPUT_MAX_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(TOOL_OUTPUT_MAX_CHARS).collect();
    format!("{head}\n…")
}

fn tool_card(
    ui: &mut Ui,
    palette: &Palette,
    name: &str,
    label: &str,
    output: &str,
    state: ToolCardState,
) -> ToolCardReply {
    let ToolCardState {
        failed,
        running,
        turn_index,
        copied,
    } = state;
    let shell = is_shell_tool(name);
    let command = command_text(label);
    let (badge, colour) = tool_badge(name, palette);
    let muted = to_color32(palette.text_muted);
    let output_id = ui.make_persistent_id(("ai-command-output", turn_index));
    let mut stop = false;
    let mut copy = None;
    egui::Frame::new()
        .fill(to_color32(palette.bg_input))
        .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
        .corner_radius(wassh_core::RADIUS)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let shown = card_preview(output);
            let (outcome, outcome_colour) = if running {
                ("running", muted)
            } else if failed {
                ("failed", to_color32(palette.danger))
            } else {
                ("ok", to_color32(palette.ok))
            };
            egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                output_id,
                false,
            )
            .show_header(ui, |ui| {
                badge_pill(ui, palette, badge, colour);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if shell && copy_button(ui, palette, copied == Some(CopyTarget::Command)) {
                        copy = Some((CopyTarget::Command, command.to_owned()));
                    }
                    // The command is still going: offer to end it here rather than on the host.
                    if running
                        && Button::new("Stop")
                            .compact()
                            .kind(ButtonKind::Danger)
                            .show(ui, palette)
                            .clicked()
                    {
                        stop = true;
                    }
                    ui.label(
                        egui::RichText::new(outcome)
                            .size(10.0)
                            .strong()
                            .color(outcome_colour),
                    );
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(label)
                                    .size(11.0)
                                    .monospace()
                                    .color(to_color32(palette.text)),
                            )
                            .truncate(),
                        );
                    });
                });
            })
            .body(|ui| {
                if tool_output(
                    ui,
                    palette,
                    &shown,
                    shell && !output.trim().is_empty(),
                    copied == Some(CopyTarget::Output),
                ) {
                    copy = Some((CopyTarget::Output, output.to_owned()));
                }
            });
        });
    ToolCardReply { stop, copy }
}

/// The output text, with the button that copies all of it level with its first line.
///
/// Returns whether that button was clicked.
fn tool_output(ui: &mut Ui, palette: &Palette, shown: &str, copyable: bool, copied: bool) -> bool {
    let mut copy = false;
    ui.horizontal_top(|ui| {
        let room = if copyable {
            // Both children: the gap between them is part of the budget.
            (ui.available_width() - COPY_BUTTON_SIZE - ui.spacing().item_spacing.x)
                .max(MIN_OUTPUT_WIDTH)
        } else {
            ui.available_width()
        };
        ui.allocate_ui_with_layout(
            egui::Vec2::new(room, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(room);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(if shown.is_empty() { NO_OUTPUT } else { shown })
                            .size(11.0)
                            .monospace()
                            .color(to_color32(palette.text_muted)),
                    )
                    .wrap(),
                );
            },
        );
        if copyable && copy_button(ui, palette, copied) {
            copy = true;
        }
    });
    copy
}

/// The command a shell card shows, without its prompt marker, ready to paste.
fn command_text(label: &str) -> &str {
    let label = label
        .strip_prefix(LOCAL_COMMAND_PREFIX)
        .unwrap_or(label);
    label.strip_prefix(COMMAND_PROMPT).unwrap_or(label)
}

/// Whether `name` is a tool that runs a shell command, whose card carries its output.
fn is_shell_tool(name: &str) -> bool {
    matches!(
        name,
        super::protocol::TOOL_RUN_COMMAND
            | super::protocol::TOOL_RUN_SUDO
            | super::protocol::TOOL_LOCAL_RUN
    )
}

/// One tool's pill, e.g. `SSH` or `REMOTE FILESYSTEM`.
fn badge_pill(ui: &mut Ui, palette: &Palette, label: &str, colour: wassh_core::Rgba) {
    egui::Frame::new()
        .fill(to_color32(wassh_core::colors::tint(
            colour,
            palette.bg_panel_2,
            BADGE_TINT,
        )))
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(label.to_uppercase())
                    .size(9.0)
                    .strong()
                    .color(to_color32(colour)),
            );
        });
}

/// The badge a tool call gets, named after the reference's `toolBadge`.
fn tool_badge(name: &str, palette: &Palette) -> (&'static str, wassh_core::Rgba) {
    let rgb = wassh_core::Rgba::rgb;
    match name {
        "" | super::protocol::TOOL_RUN_COMMAND => ("SSH", palette.accent),
        super::protocol::TOOL_RUN_SUDO => ("Root", palette.danger),
        other if other.starts_with("dev_") || other.starts_with("remote_fs_") => {
            ("Remote filesystem", rgb(0x3dd68c))
        }
        other if other.starts_with("local_fs_") => ("Local PC", rgb(0xf0b429)),
        super::protocol::TOOL_WEB_SEARCH => ("Search", rgb(0x38bdf8)),
        super::protocol::TOOL_WEB_FETCH => ("Web", rgb(0xa78bfa)),
        super::protocol::TOOL_CURRENT_TIME => ("Time", rgb(0xfb7185)),
        super::protocol::TOOL_RAG_SEARCH => ("RAG", rgb(0xc084fc)),
        other if other.starts_with("system_") => ("System", rgb(0x2dd4bf)),
        other if other.starts_with("terminal_") => ("Your terminal", rgb(0xf472b6)),
        super::protocol::TOOL_LOAD_SCOPE | super::protocol::TOOL_UNLOAD_SCOPE => {
            ("Scope", palette.text_muted)
        }
        _ => ("Tool", palette.text_muted),
    }
}

/// Frame a prompt card so it reads as a popup over the conversation rather than as more
/// transcript text.
///
/// The card is capped: a request the model wrote a whole script into must not push the
/// conversation and the composer out of the pane. Only `body` scrolls — `controls` are
/// measured first and drawn under it, so the buttons under the request keep their place
/// however long the request is, and a card holding one line stays one line tall.
fn prompt_card(
    ui: &mut Ui,
    palette: &Palette,
    id: &str,
    body: impl FnOnce(&mut Ui),
    mut controls: impl FnMut(&mut Ui),
) {
    let room = (ui.available_height() * PROMPT_CARD_SHARE)
        .clamp(PROMPT_CARD_MIN_HEIGHT, PROMPT_CARD_MAX_HEIGHT);
    let inner_width = ui.available_width() - 2.0 * f32::from(PROMPT_CARD_PADDING);
    let controls_height = measured_height(
        ui,
        egui::Id::unique(("ai-prompt-card-controls", id)),
        inner_width,
        room,
        |ui| controls(ui),
    );
    egui::Frame::new()
        .fill(to_color32(palette.bg_panel_2))
        .stroke(egui::Stroke::new(1.0, to_color32(palette.accent)))
        .corner_radius(wassh_core::RADIUS)
        .inner_margin(egui::Margin::same(PROMPT_CARD_PADDING))
        .show(ui, |ui| {
            let body_room = (room - controls_height - PROMPT_CARD_GAP).max(0.0);
            egui::ScrollArea::vertical()
                .id_salt(("ai-prompt-card", id))
                .max_height(body_room)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), body);
                });
            ui.add_space(PROMPT_CARD_GAP);
            controls(ui);
        });
}

fn prompt_action_row(ui: &mut Ui, id: &str, actions: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::horizontal()
        .id_salt(("ai-prompt-actions", id))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(actions);
        });
}

/// Height `content` takes at `width`, without drawing any of it and without moving `ui`'s
/// own cursor.
fn measured_height(
    ui: &Ui,
    id: egui::Id,
    width: f32,
    height: f32,
    content: impl FnOnce(&mut Ui),
) -> f32 {
    let mut ui = Ui::new(
        ui.ctx().clone(),
        ui.scope_id().with(id),
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                ui.next_widget_position(),
                egui::Vec2::new(width, height),
            ))
            .layout(egui::Layout::top_down(egui::Align::Min))
            .invisible()
            .sizing_pass(),
    );
    content(&mut ui);
    ui.min_rect().height()
}

/// The command a prompt card is about, cut so a whole script cannot fill the card.
fn prompt_command(ui: &mut Ui, palette: &Palette, text: &str) {
    let shown = if text.chars().count() <= PROMPT_COMMAND_MAX_CHARS {
        text.to_owned()
    } else {
        let head: String = text.chars().take(PROMPT_COMMAND_MAX_CHARS).collect();
        format!("{head}…")
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(shown)
                .size(11.5)
                .monospace()
                .color(to_color32(palette.accent)),
        )
        .wrap(),
    );
}

/// `text` cut to [`SCOPE_LABEL_MAX_CHARS`], so one "always allow" button stays a button.
fn scope_label(text: &str) -> String {
    if text.chars().count() <= SCOPE_LABEL_MAX_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(SCOPE_LABEL_MAX_CHARS).collect();
    format!("{head}…")
}

/// What the composer's action button does right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionState {
    /// Send what is typed.
    Send,
    /// Stop the run in front of the user.
    StopMain,
    /// Stop the sub-agents a stopped run left working.
    StopSubagents,
}

/// Which agent a prompt on screen belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptTarget {
    /// The main conversation.
    Main,
    /// One of its sub-agents.
    Subagent(usize),
}

/// What one agent is waiting for, taken out of it so the cards can be drawn while the pane
/// borrows itself mutably.
#[derive(Default)]
struct Prompts {
    /// Approval card: the command and the scopes on offer.
    approval: Option<(String, Vec<ApprovalScope>)>,
    /// Capability card: the label, and the tool it belongs to when it has a setting.
    allowing: Option<(String, Option<String>)>,
    /// Question card: the call id and the questions.
    asking: Option<(String, Vec<AgentQuestion>)>,
    /// Sudo card: the command and why it needs root.
    sudo: Option<(String, Option<String>)>,
}

impl Prompts {
    /// What `agent` is waiting for.
    fn of(agent: &Agent) -> Self {
        Self {
            approval: match &agent.phase {
                Some(Phase::Approving {
                    command, scopes, ..
                }) => Some((command.clone(), scopes.clone())),
                _ => None,
            },
            allowing: match &agent.phase {
                Some(Phase::Allowing { call, label, .. }) => {
                    Some((label.clone(), Some(call.name.clone())))
                }
                _ => None,
            },
            asking: match &agent.phase {
                Some(Phase::Asking { call, questions }) => {
                    Some((call.id.clone(), questions.clone()))
                }
                _ => None,
            },
            sudo: agent
                .sudo_prompt()
                .map(|(command, reason)| (command.to_owned(), reason.map(str::to_owned))),
        }
    }

    /// Whether there is nothing to ask.
    fn is_empty(&self) -> bool {
        self.approval.is_none()
            && self.allowing.is_none()
            && self.asking.is_none()
            && self.sudo.is_none()
    }
}

/// What the user did with the prompt cards this frame.
#[derive(Default)]
struct PromptAnswers {
    /// Approval card: (allow, scope to remember, allow every command).
    approval: Option<(bool, Option<usize>, bool)>,
    /// Capability card: (allow, remember the answer).
    permission: Option<(bool, bool)>,
    /// Capability card: allow every prompt for the rest of the session.
    allow_session: bool,
    /// The question card's answers, one per question.
    answers: Option<Vec<String>>,
    /// The sudo card.
    sudo: Option<SudoAnswer>,
    /// Stop the agent without answering the prompt.
    stop: bool,
}

/// What the question card holds: the call it belongs to, the question on screen and what
/// the user entered for each of them.
#[derive(Default)]
struct QuestionWizard {
    /// Id of the `ask_user` call the answers belong to.
    call: Option<String>,
    /// Question currently shown.
    page: usize,
    /// What the user entered per question, in the order they were asked.
    drafts: Vec<QuestionDraft>,
}

/// What the user entered on one question.
#[derive(Clone, Default)]
struct QuestionDraft {
    /// Option the user picked, when the answer is one of them.
    chosen: Option<usize>,
    /// Typed answer, which stands in place of an option.
    text: String,
}

impl QuestionWizard {
    /// Start on the first question of `call`, unless the card is already there.
    fn open(&mut self, call: &str, question_count: usize) {
        if self.call.as_deref() == Some(call) && self.drafts.len() == question_count {
            return;
        }
        self.call = Some(call.to_owned());
        self.page = 0;
        self.drafts = vec![QuestionDraft::default(); question_count];
    }

    /// The answer entered for the question on `page`, once it has one.
    fn answer(&self, page: usize, options: &[String]) -> Option<String> {
        let draft = self.drafts.get(page)?;
        let text = draft.text.trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
        draft.chosen.and_then(|index| options.get(index)).cloned()
    }

    /// Every question's answer, once each of them has one.
    fn answers(&self, questions: &[AgentQuestion]) -> Option<Vec<String>> {
        questions
            .iter()
            .enumerate()
            .map(|(page, question)| self.answer(page, &question.options))
            .collect()
    }
}

impl AiAgent {
    /// The prompt cards: an approval, a capability permission, a question and a sudo
    /// password, whichever the agent is waiting for.
    ///
    /// They are drawn above the composer rather than inside the transcript, so a long
    /// command cannot push the conversation around and a request always reads as a popup
    /// instead of more transcript text.
    fn prompt_cards(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        muted: egui::Color32,
        prompts: &Prompts,
        source: Option<&str>,
    ) -> PromptAnswers {
        let mut answers = PromptAnswers::default();

        // A card from a sub-agent says whose it is, so the main chat stays the one place
        // where every answer is given.
        if let Some(source) = source
            && !prompts.is_empty()
        {
            ui.label(
                egui::RichText::new(format!("From sub-agent \"{source}\""))
                    .size(10.0)
                    .strong()
                    .color(to_color32(palette.accent)),
            );
        }

        // The approval card: the command the model wants to run, and how far to allow it.
        if let Some((command, scopes)) = &prompts.approval {
            let mut chosen: Option<(bool, Option<usize>)> = None;
            let mut everything = false;
            prompt_card(
                ui,
                palette,
                "approval",
                |ui| {
                    ui.label(
                        egui::RichText::new("Run this command?")
                            .size(12.0)
                            .strong()
                            .color(to_color32(palette.text)),
                    );
                    prompt_command(ui, palette, command);
                },
                |ui| {
                    prompt_action_row(ui, "approval", |ui| {
                        if Button::new("Allow")
                            .compact()
                            .kind(ButtonKind::Primary)
                            .show(ui, palette)
                            .clicked()
                        {
                            chosen = Some((true, None));
                        }
                        if Button::new("Deny")
                            .compact()
                            .kind(ButtonKind::Danger)
                            .show(ui, palette)
                            .clicked()
                        {
                            chosen = Some((false, None));
                        }
                    });
                    // Session-wide, so it gets its own line: the row never scrolls.
                    if Button::new("Allow all this session")
                        .compact()
                        .show(ui, palette)
                        .clicked()
                    {
                        everything = true;
                    }
                    ui.vertical(|ui| {
                        for (index, scope) in scopes.iter().enumerate() {
                            let label = if scope.word_count == 0 {
                                format!("Always allow exactly: {}", scope_label(&scope.text))
                            } else {
                                format!(
                                    "Always allow commands starting with: {}",
                                    scope_label(&scope.text)
                                )
                            };
                            if Button::new(&label)
                                .compact()
                                .show(ui, palette)
                                .clicked()
                            {
                                chosen = Some((true, Some(index)));
                            }
                        }
                    });
                    if let Some((allow, remember)) = chosen {
                        answers.approval = Some((allow, remember, false));
                    } else if everything {
                        answers.approval = Some((true, None, true));
                    }
                },
            );
        }

        // The capability card: a tool whose permission is "ask".
        if let Some((label, _)) = &prompts.allowing {
            prompt_card(
                ui,
                palette,
                "permission",
                |ui| {
                    ui.label(
                        egui::RichText::new("Approve action?")
                            .size(12.0)
                            .strong()
                            .color(to_color32(palette.text)),
                    );
                    prompt_command(ui, palette, label);
                },
                |ui| {
                    prompt_action_row(ui, "permission-now", |ui| {
                        if Button::new("Approve once")
                            .compact()
                            .primary()
                            .show(ui, palette)
                            .clicked()
                        {
                            answers.permission = Some((true, false));
                        }
                        if Button::new("Deny once")
                            .compact()
                            .kind(ButtonKind::Danger)
                            .show(ui, palette)
                            .clicked()
                        {
                            answers.permission = Some((false, false));
                        }
                    });
                    // The remembered answers and the session-wide one each get their own
                    // line, so no row has to scroll sideways.
                    prompt_action_row(ui, "permission-always", |ui| {
                        if Button::new("Always allow")
                            .compact()
                            .primary()
                            .show(ui, palette)
                            .clicked()
                        {
                            answers.permission = Some((true, true));
                        }
                        if Button::new("Always deny")
                            .compact()
                            .kind(ButtonKind::Danger)
                            .show(ui, palette)
                            .clicked()
                        {
                            answers.permission = Some((false, true));
                        }
                    });
                    if Button::new("Allow all this session")
                        .compact()
                        .show(ui, palette)
                        .clicked()
                    {
                        answers.allow_session = true;
                    }
                },
            );
        }

        // The question card: one question at a time, every answer kept so Previous can
        // change it and only the last question submits.
        if let Some((call, questions)) = &prompts.asking {
            self.questions.open(call, questions.len());
            let page = self.questions.page;
            let question = &questions[page];
            let is_last = page + QUESTION_INDEX_OFFSET >= questions.len();
            prompt_card(
                ui,
                palette,
                "question",
                |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Question {} of {}",
                            page + QUESTION_INDEX_OFFSET,
                            questions.len()
                        ))
                        .size(10.0)
                        .color(muted),
                    );
                    ui.label(
                        egui::RichText::new(question.text.as_str())
                            .size(12.0)
                            .strong()
                            .color(to_color32(palette.text)),
                    );
                },
                |ui| {
                    let room = (ui.available_width() - PROMPT_CARD_GAP).max(MIN_ANSWER_WIDTH);
                    let mut picked = false;
                    let mut enter = false;
                    let mut confirmed = false;
                    let mut previous = false;
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(room, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(room);
                            let draft = &mut self.questions.drafts[page];
                            // An option is the whole answer, so picking one moves on — but
                            // never past the last question: that one still needs Answer.
                            for (index, option) in question.options.iter().enumerate() {
                                if ui.radio(draft.chosen == Some(index), option).clicked() {
                                    draft.chosen = Some(index);
                                    draft.text.clear();
                                    picked = true;
                                }
                            }
                            // "Write your own" is the text field itself, there from the
                            // start: no option to pick before it can be used.
                            let response =
                                text_area(ui, palette, &mut draft.text, 2, "Your answer…");
                            if !draft.text.trim().is_empty() {
                                draft.chosen = None;
                            }
                            enter = response.has_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                                && !ui.input(|input| input.modifiers.shift);
                        },
                    );
                    let answered = self.questions.answer(page, &question.options).is_some();
                    confirmed |= enter && answered;
                    ui.horizontal(|ui| {
                        if small_button_enabled(ui, palette, "Previous", page > 0) {
                            previous = true;
                        }
                        let label = if is_last { "Answer" } else { "Next" };
                        if small_button_enabled(ui, palette, label, answered) {
                            confirmed = true;
                        }
                        if Button::new("Stop")
                            .compact()
                            .kind(ButtonKind::Danger)
                            .show(ui, palette)
                            .clicked()
                        {
                            answers.stop = true;
                        }
                    });
                    if previous {
                        self.questions.page = page.saturating_sub(1);
                    } else if confirmed && is_last {
                        answers.answers = self.questions.answers(questions);
                    } else if confirmed || (picked && !is_last) {
                        self.questions.page = page + 1;
                    }
                },
            );
        }

        // The sudo card: the command that needs root, and the password for it.
        if let Some((command, reason)) = &prompts.sudo {
            prompt_card(
                ui,
                palette,
                "sudo",
                |ui| {
                    ui.label(
                        egui::RichText::new("Sudo password required")
                            .size(12.0)
                            .strong()
                            .color(to_color32(palette.text)),
                    );
                    prompt_command(ui, palette, command);
                    if let Some(reason) = reason {
                        ui.add(
                            egui::Label::new(egui::RichText::new(reason).size(11.0).color(muted))
                                .wrap(),
                        );
                    }
                },
                |ui| {
                    let room = (ui.available_width() - PROMPT_CARD_GAP).max(MIN_ANSWER_WIDTH);
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(room, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(room);
                            let response =
                                password_input(ui, palette, &mut self.sudo, "sudo password");
                            if response.has_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                answers.sudo = Some(SudoAnswer::Run);
                            }
                        },
                    );
                    ui.horizontal(|ui| {
                        if Button::new("Continue")
                            .compact()
                            .primary()
                            .show(ui, palette)
                            .clicked()
                        {
                            answers.sudo = Some(SudoAnswer::Run);
                        }
                        if Button::new("Cancel")
                            .compact()
                            .kind(ButtonKind::Danger)
                            .show(ui, palette)
                            .clicked()
                        {
                            answers.sudo = Some(SudoAnswer::Cancel);
                        }
                    });
                },
            );
        }

        answers
    }
}

impl Plugin for AiAgent {
    fn manifest(&self) -> PluginManifest {
        PluginId::AiAgent.manifest()
    }

    fn take_toasts(&mut self) -> Vec<(ToastKind, String)> {
        std::mem::take(&mut self.toasts)
    }

    fn header_status(&self) -> Option<(&str, HeaderHealth)> {
        (!self.caption.0.is_empty()).then_some((self.caption.0.as_str(), self.caption.1))
    }

    fn take_app_settings(&mut self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.app_writes)
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = &context.palette;
        let muted = to_color32(palette.text_muted);
        // Taken out so the borrow of one field does not block the rest.
        let mut session = context.session.take();
        self.sync_providers(context.settings, context.secrets);
        self.load(context.settings);
        // A sub-agent that went away with its chat takes its view with it.
        if let ChatView::Subagent(index) = self.view
            && index >= self.agent.subagents.len()
        {
            self.view = ChatView::Chat;
        }
        self.apply_settings(context.settings, context.secret(SETTING_WEB_SEARCH_KEY));
        self.poll_models();
        self.refresh_models(context);
        self.agent.now_ms = wassh_core::time::now_ms();
        self.poll_summary();
        self.poll_chat_title();
        self.poll_picker();
        self.compact_if_needed(context.settings, &context.host_label);

        // Drain workers, then answer any tool result it produced. Sub-agents are drained
        // with it, so they keep working while the user reads another chat.
        self.agent.poll(&mut session);
        self.launch_spawn(
            &mut session,
            context.settings,
            &context.host_label,
            &context.environment,
        );
        self.drive_subagents(context.settings, &context.host_label, &context.environment);
        // A chat set aside keeps its sub-agents working, so they finish while it is filed.
        self.poll_filed_subagents(
            &mut session,
            context.settings,
            &context.host_label,
            &context.environment,
        );
        // The environment map belongs to the host, so the pane scans it outside a turn.
        if self.agent.poll_map_scan(&mut session) && self.agent.map.is_empty() {
            let root = self.agent.map.root.clone();
            self.agent
                .turns
                .push(Turn::Notice(format!("Scan of {root} found no files.")));
        }
        if std::mem::take(&mut self.map_rebuild) {
            // The host picks the project root above the session.
            if let Err(error) = self.agent.start_map_scan("", &mut session) {
                self.report_error(error);
            }
        }
        self.poll_map_deep(
            context.settings,
            &context.host_label,
            &context.environment,
        );
        // A message typed during the last run joins the conversation before the next
        // request is built, so the model sees it.
        self.agent.flush_injected();
        if self.agent.take_needs_reply() {
            let request = self.request(context.settings, &context.host_label, &context.environment);
            if let Some(request) = request {
                self.agent.start(request);
            }
        }
        // A queued follow-up goes out as soon as the run in front of it ends.
        if !self.agent.blocked()
            && let Some(queued) = self.queued.take()
        {
            match self.ready() {
                Ok(()) => {
                    self.send_message(
                        &queued.message,
                        queued.meta,
                        queued.images,
                        context.settings,
                        &context.host_label,
                        &context.environment,
                    );
                }
                Err(error) => {
                    self.report_error(error);
                    self.queued = Some(queued);
                }
            }
        }
        // The caption only changes when the run state or the session does, so its text is
        // built then rather than every frame. The chat's own title sits in the body beside
        // the back button, so an idle pane says nothing here.
        let running = self.agent.subagents_running();
        let waiting = self.view != ChatView::List && self.agent.waiting_subagent().is_some();
        let caption_for = (
            self.agent.busy(),
            self.summary.is_some(),
            context.status,
            running,
            waiting,
        );
        if self.caption_for != Some(caption_for) {
            self.caption_for = Some(caption_for);
            self.caption = if waiting {
                ("A sub-agent needs an answer".to_owned(), HeaderHealth::Warn)
            } else if running > 0 {
                let plural = if running == 1 { "" } else { "s" };
                (
                    format!("{running} sub-agent{plural} running"),
                    HeaderHealth::Warn,
                )
            } else if caption_for.0 || caption_for.1 {
                ("Thinking…".to_owned(), HeaderHealth::Warn)
            } else if context.status == SessionStatus::Connected {
                (String::new(), HeaderHealth::Ok)
            } else {
                ("Session not connected".to_owned(), HeaderHealth::Warn)
            };
        }

        let busy = self.agent.busy();

        // A chat leads with a way back to the list and the chat's own title; the list leads
        // with its heading. Both keep a way into a fresh conversation at the right edge.
        // Provider and model live under the composer.
        let mut back = false;
        let mut new_chat = false;
        egui::containers::Sides::new()
            .height(BACK_BUTTON_SIZE)
            .shrink_left()
            .truncate()
            .show(
                ui,
                |ui| match self.view {
                    ChatView::Chat => {
                        back = back_button(ui, palette);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(self.title()).size(CHAT_TITLE_TEXT_SIZE),
                            )
                            .truncate(),
                        );
                    }
                    ChatView::Subagent(index) => {
                        back = back_button(ui, palette);
                        let (title, state) = match self.agent.subagents.get(index) {
                            Some(sub) => (sub.title.clone(), sub.state()),
                            None => (String::new(), SubagentState::Done),
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("\u{21b3} {title}"))
                                    .size(CHAT_TITLE_TEXT_SIZE),
                            )
                            .truncate(),
                        );
                        ui.label(
                            egui::RichText::new(state.label())
                                .size(FOOTER_TEXT_SIZE)
                                .color(muted),
                        );
                    }
                    ChatView::List => {
                        ui.label(
                            egui::RichText::new(CHAT_LIST_HEADING)
                                .size(FOOTER_TEXT_SIZE)
                                .color(muted),
                        );
                    }
                },
                |ui| {
                    // A sub-agent belongs to the chat that started it, so only the chat and
                    // the list offer a fresh one. A run in flight keeps its conversation.
                    new_chat = matches!(self.view, ChatView::Chat | ChatView::List)
                        && Button::new(NEW_CHAT_LABEL)
                            .compact()
                            .ghost()
                            .enabled(!busy)
                            .show(ui, palette)
                            .clicked();
                },
            );
        if new_chat {
            self.begin_new_chat();
        }
        if back {
            match self.view {
                // Back returns to the chat that owns the sub-agent.
                ChatView::Subagent(_) => self.view = ChatView::Chat,
                _ => {
                    // Leaving a settled chat files it, so the list holds everything the pane
                    // has. A run still going stays open: clearing it would end the run.
                    if !busy {
                        self.file_chat();
                        self.reset_chat();
                    }
                    self.view = ChatView::List;
                }
            }
        }

        // The provider, model and keys are configured in Options → AI agent; the pane
        // only picks which of them this conversation uses.
        ui.separator();

        // Everything pinned under the transcript sits in a bottom panel, so the transcript
        // gets exactly the rest of the pane and no room is left over.
        let mut submit = false;
        // Alt+Enter queues the message for after the run instead of injecting it.
        let mut queue = false;
        let mut command: Option<SlashCommand> = None;
        let mut prompts = PromptAnswers::default();
        // The main conversation's request comes first; a sub-agent's is shown when the
        // main one has nothing to answer.
        let (pending, prompt_target) = self.pending_prompts();
        let prompt_source = match prompt_target {
            PromptTarget::Main => None,
            PromptTarget::Subagent(index) => Some(self.agent.subagents[index].title.clone()),
        };
        let slash = parse_slash_input(&self.draft);
        egui::Panel::bottom("ai-composer")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .show(ui, |ui| {
                // A request the agent is blocked on comes first: it is the one thing the
                // user has to answer before the run can go on.
                prompts = self.prompt_cards(ui, palette, muted, &pending, prompt_source.as_deref());

                // A sub-agent's transcript is for watching: answers go through the main
                // chat, and Back returns to it.
                if let ChatView::Subagent(_) = self.view {
                    ui.label(
                        egui::RichText::new(SUBAGENT_VIEW_NOTICE)
                            .size(11.0)
                            .italics()
                            .color(muted),
                    );
                    return;
                }

                // A run cut off before the model answered can be picked up again.
                if self.agent.can_resume() {
                    let mut resume = false;
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("The previous run was interrupted.")
                                .size(11.0)
                                .color(muted),
                        );
                        resume = small_button_enabled(ui, palette, "Continue", true);
                    });
                    if resume {
                        self.resume(context.settings, &context.host_label, &context.environment);
                    }
                }

                // Attachment chips.
                if !self.attachments.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        let mut drop = None;
                        for (index, attachment) in self.attachments.iter().enumerate() {
                            let label = format!("{} ✕", attachment.name());
                            if small_button_enabled(ui, palette, &label, true) {
                                drop = Some(index);
                            }
                        }
                        if let Some(index) = drop {
                            self.attachments.remove(index);
                        }
                    });
                }

                // Slash suggestions.
                if let Some(slash) = &slash
                    && !slash.matches.is_empty()
                {
                    for command in &slash.matches {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} — {}",
                                command.label, command.description
                            ))
                            .size(11.0)
                            .color(muted),
                        );
                    }
                }

                // A message typed while the agent was busy, like the reference's queue strip.
                if let Some(queued) = self.queued.clone() {
                    let mut edit = false;
                    let mut drop = false;
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Queued")
                                .size(10.0)
                                .strong()
                                .color(to_color32(palette.accent)),
                        );
                        let preview = if queued.text.chars().count() > QUEUE_PREVIEW_MAX_CHARS {
                            let head: String =
                                queued.text.chars().take(QUEUE_PREVIEW_MAX_CHARS).collect();
                            format!("{head}…")
                        } else {
                            queued.text.clone()
                        };
                        let room = (ui.available_width() - 90.0).max(60.0);
                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(room, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_width(room);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(preview).size(11.0).color(muted),
                                    )
                                    .truncate(),
                                );
                            },
                        );
                        edit = small_button(ui, palette, "Edit");
                        drop = small_button(ui, palette, "\u{2715}");
                    });
                    if edit {
                        self.draft = queued.text;
                        self.queued = None;
                    } else if drop {
                        self.queued = None;
                    }
                }

                // Scopes this conversation carries; a click drops one and the room its tools
                // took goes with it.
                let loaded: Vec<&'static str> = self
                    .agent
                    .scopes
                    .ids()
                    .iter()
                    .copied()
                    .filter(|id| *id != scopes::ROOT)
                    .collect();
                if !loaded.is_empty() {
                    let mut opened = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(SCOPE_LABEL)
                                .size(SCOPE_TAG_TEXT_SIZE)
                                .color(muted),
                        );
                        for id in loaded {
                            let label = if id == ENV_SCOPE { GRAPH_SCOPE_LABEL } else { id };
                            let tag = scope_tag(ui, palette, label).on_hover_text(SCOPE_CHIP_HINT);
                            if tag.clicked() {
                                opened = Some((id, tag.rect));
                            }
                        }
                    });
                    if let Some((id, anchor)) = opened {
                        self.scope_popup = Some(ScopePopup { id, anchor });
                    }
                }

                // Composer: the include tools stack in a column beside the prompt, both
                // starting at the prompt's top edge.
                ui.separator();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        let attach = Button::new(ATTACH_GLYPH)
                            .compact()
                            .enabled(
                                self.picking.is_none() && self.attachments.len() < MAX_ATTACHMENTS,
                            )
                            .show(ui, palette);
                        if attach.clicked() {
                            self.pick_attachments();
                        }
                        let terminal = Button::new(TERMINAL_ATTACH_GLYPH)
                            .compact()
                            .active(self.attach_terminal)
                            .show(ui, palette);
                        if terminal.clicked() {
                            self.attach_terminal = !self.attach_terminal;
                        }
                    });
                    let room = ui.available_width().max(MIN_COMPOSER_WIDTH);
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(room, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(room);
                            let response = text_area_with_id(
                                ui,
                                palette,
                                &mut self.draft,
                                2,
                                "Ask the agent…",
                                self.composer_id,
                            );
                            if response.has_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                                && !ui.input(|input| input.modifiers.shift)
                            {
                                submit = true;
                                // Alt turns the send into a queue, like it does in other
                                // chat clients.
                                queue = ui.input(|input| input.modifiers.alt);
                            }
                        },
                    );
                });

                // Provider and model, with the action at the right end of the same row.
                let can_send = !self.providers.is_empty() && !self.draft.trim().is_empty();
                let action_state = self.action_state(can_send);
                // The stop hint says whether a second press is coming.
                let stop_hint = match action_state {
                    ActionState::StopMain if self.agent.subagents_running() > 0 => {
                        "Stop this run. Press again to stop the sub-agents."
                    }
                    ActionState::StopMain => "Stop this run.",
                    ActionState::StopSubagents => "Stop the sub-agents still running.",
                    ActionState::Send => "",
                };
                let (chosen_provider, chosen_model, action_clicked) = {
                    let mut plan_mode = self.agent.plan;
                    let mut compact_clicked = false;
                    let provider_options: Vec<(usize, &str)> = self
                        .providers
                        .iter()
                        .enumerate()
                        .map(|(index, provider)| (index, provider.name.as_str()))
                        .collect();
                    let model_options: Vec<(String, &str)> = self
                        .active()
                        .map(|provider| {
                            provider
                                .models
                                .iter()
                                .map(|model| (model.clone(), model.as_str()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let chosen = ui
                        .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = FOOTER_GAP;
                        // The reference footer is set in the same small type as the rest.
                        ui.style_mut().text_styles.insert(
                            egui::TextStyle::Button,
                            egui::FontId::proportional(FOOTER_TEXT_SIZE),
                        );
                        let action = match action_state {
                            ActionState::Send => Button::new("Send").compact().enabled(can_send),
                            ActionState::StopMain => {
                                Button::new("Stop").compact().kind(ButtonKind::Danger)
                            }
                            ActionState::StopSubagents => Button::new("Stop agents")
                                .compact()
                                .kind(ButtonKind::Danger)
                                .active(true),
                        };
                        // The mode says most about what this chat may do, so it leads; the
                        // Compact button sits where the model dropdown starts. A pane too
                        // narrow for both keeps the action and drops them.
                        let mode_button = Button::new(if plan_mode {
                            MODE_PLAN_LABEL
                        } else {
                            MODE_AGENT_LABEL
                        })
                        .compact()
                        .active(plan_mode);
                        let compact_button = Button::new(COMPACT_LABEL)
                            .compact()
                            .enabled(self.summary.is_none() && !self.agent.turns.is_empty());
                        let controls = mode_button.size(ui).x
                            + compact_button.size(ui).x
                            + FOOTER_GAP;
                        let room = (ui.available_width() - action.size(ui).x - FOOTER_GAP * 2.0)
                            .max(0.0);
                        if room >= controls + MIN_SELECT_ROOM * 2.0 {
                            let hint = if plan_mode {
                                MODE_TO_AGENT_HINT
                            } else {
                                MODE_TO_PLAN_HINT
                            };
                            if mode_button.show(ui, palette).on_hover_text(hint).clicked() {
                                plan_mode = !plan_mode;
                            }
                            compact_clicked = compact_button
                                .show(ui, palette)
                                .on_hover_text(COMPACT_HINT)
                                .clicked();
                        }
                        if self.providers.is_empty() {
                            // The action comes first; the hint is cropped to what is left.
                            let room =
                                (ui.available_width() - action.size(ui).x - FOOTER_GAP).max(0.0);
                            let height = ui.spacing().interact_size.y;
                            ui.add_sized(
                                egui::Vec2::new(room, height),
                                egui::Label::new(
                                    egui::RichText::new(
                                        "No model providers configured — add one in Options.",
                                    )
                                    .size(FOOTER_TEXT_SIZE)
                                    .color(muted),
                                )
                                .truncate(),
                            );
                            let response = action.show(ui, palette);
                            let clicked = if stop_hint.is_empty() {
                                response.clicked()
                            } else {
                                response.on_hover_text(stop_hint).clicked()
                            };
                            return (None, None, clicked);
                        }
                        // The dropdowns give way to the action, never the other way round.
                        let spare =
                            (ui.available_width() - action.size(ui).x - FOOTER_GAP * 2.0).max(0.0);
                        let select_room = spare / 2.0;
                        let provider = cropped_select(
                            ui,
                            "ai-provider",
                            &self.provider,
                            &provider_options,
                            select_room,
                        );
                        let model = cropped_select(
                            ui,
                            "ai-model",
                            &self.model,
                            &model_options,
                            select_room,
                        );
                        let response = action.show(ui, palette);
                        let clicked = if stop_hint.is_empty() {
                            response.clicked()
                        } else {
                            response.on_hover_text(stop_hint).clicked()
                        };
                        (provider, model, clicked)
                    })
                    .inner;
                    if plan_mode != self.agent.plan {
                        self.agent.plan = plan_mode;
                    }
                    if compact_clicked {
                        command = Some(COMPACT_COMMAND);
                    }
                    chosen
                };
                if action_clicked {
                    match action_state {
                        ActionState::StopMain => self.agent.stop(),
                        ActionState::StopSubagents => {
                            self.agent.stop_subagents();
                        }
                        ActionState::Send => submit = true,
                    }
                }
                if let Some(index) = chosen_provider {
                    self.provider = index;
                    // A provider starts on its first model, like the reference.
                    self.model = self
                        .providers
                        .get(index)
                        .and_then(|provider| provider.models.first())
                        .cloned()
                        .unwrap_or_default();
                }
                if let Some(model) = chosen_model {
                    self.model = model;
                }
            });

        // The chat list, the open chat, or a sub-agent's transcript.
        let mut reply = TranscriptReply::default();
        let source = match self.view {
            ChatView::List => None,
            ChatView::Chat => Some(TranscriptSource::Main),
            ChatView::Subagent(index) => Some(TranscriptSource::Subagent(index)),
        };
        if source.is_none() {
            self.chat_list(ui, palette, muted);
        } else if let Some(source) = source {
            let scroll = egui::ScrollArea::vertical()
                .id_salt("ai-transcript")
                .max_height(ui.available_height())
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    self.transcript(ui, palette, muted, &mut session, source)
                });
            reply = scroll.inner;

            // The message the transcript scrolled past the top stays pinned there, as in the
            // reference chat: a long reply cannot push it out of sight. It opens the same
            // editor as the message it copies.
            if source == TranscriptSource::Main
                && self.editing.is_none()
                && let Some(pinned) =
                    self.pinned_bubble(ui, palette, scroll.inner_rect, &reply.bubbles)
                && pinned.clicked
            {
                self.edit_prompt(pinned.index);
            }
        }
        if let Some((index, text)) = reply.submitted {
            self.replace_prompt(
                index,
                &text,
                context.settings,
                &context.host_label,
                &context.environment,
            );
        }

        // A popup the tags opened sits over the pane, so a press anywhere else closes it.
        self.scope_popover(ui, palette);

        if let Some((allow, remember, everything)) = prompts.approval {
            if everything {
                self.prompt_agent(prompt_target)
                    .approve_everything(&mut session);
            } else {
                self.prompt_agent(prompt_target)
                    .approve(allow, remember, &mut session);
            }
        }
        if prompts.stop {
            self.questions = QuestionWizard::default();
            self.stop_prompted(prompt_target);
        }
        if prompts.allow_session {
            self.prompt_agent(prompt_target)
                .allow_everything(&mut session);
        } else if let Some((allow, always)) = prompts.permission {
            if always
                && let Some((_, Some(tool))) = &pending.allowing
                && let Some(key) = setting_key(tool)
            {
                context.set_text(&key, if allow { "allow" } else { "deny" });
            }
            self.prompt_agent(prompt_target)
                .answer_permission(allow, &mut session);
        }
        if let Some(answers) = prompts.answers {
            self.questions = QuestionWizard::default();
            self.prompt_agent(prompt_target)
                .answer(&answers, &mut session);
        }
        match prompts.sudo {
            Some(SudoAnswer::Run) => {
                let password = std::mem::take(&mut self.sudo);
                self.prompt_agent(prompt_target)
                    .submit_sudo(&password, &mut session);
            }
            Some(SudoAnswer::Cancel) => self.prompt_agent(prompt_target).cancel_sudo(&mut session),
            None => {}
        }
        // A sub-agent's "always" answers join the main conversation, so every other
        // sub-agent and the next run inherit them.
        if let PromptTarget::Subagent(index) = prompt_target {
            self.agent.adopt_subagent_rules(index);
        }

        if submit
            && !busy
            && let Some(slash) = &slash
            && let Some(exact) = slash.exact
        {
            command = Some(exact);
        }
        if let Some(command) = command {
            self.run_command(command, context.settings, &context.host_label);
        } else if submit && !self.draft.trim().is_empty() {
            // Say why nothing would go out instead of dropping the message silently.
            if let Err(error) = self.ready() {
                self.report_error(error);
            } else {
                // A prompt typed on the chat list starts a new chat instead of joining the
                // one the list holds; a run still going in the background keeps its
                // conversation.
                if self.view == ChatView::List && !self.agent.blocked() {
                    self.file_chat();
                    self.reset_chat();
                }
                self.view = ChatView::Chat;
                // The toggle attaches the live terminal screen to this one message.
                let terminal = self.attach_terminal.then(|| {
                    session
                        .as_ref()
                        .map(|access| access.screen_text())
                        .unwrap_or_default()
                });
                let text = self.draft.trim().to_owned();
                let message = self.message_with_attachments(&text, terminal.as_deref());
                let meta = UserMeta {
                    attachments: self.attachments.len(),
                    terminal: terminal.is_some(),
                };
                let images = self.attached_images();
                self.draft.clear();
                self.attachments.clear();
                self.attach_terminal = false;
                if self.agent.blocked() && queue {
                    // Alt+Enter holds the message back until the run is over.
                    self.queued = Some(Queued {
                        text,
                        message,
                        meta,
                        images,
                    });
                } else if self.agent.blocked() {
                    // Anything else joins the conversation the agent is in the middle of.
                    self.agent.inject_user_with_images(&message, meta, images);
                } else {
                    self.send_message(
                        &message,
                        meta,
                        images,
                        context.settings,
                        &context.host_label,
                        &context.environment,
                    );
                }
            }
        }

        // Keep the conversation.
        let chats: Vec<Value> = self
            .chats
            .iter()
            .map(|chat| {
                json!({
                    "title": chat.title,
                    "created": chat.created,
                    "cwd": chat.cwd,
                    "model": chat.model,
                    "provider": chat.provider,
                    "turns": turns_to_json(&chat.turns),
                    "history": history_to_json(&chat.history),
                    "subagents": subagents_to_json(&chat.subagents),
                    "compacted": chat.compacted,
                    "scopes": chat.scopes.to_json(),
                    "planMode": chat.plan
                })
            })
            .collect();
        context.set_value(
            CONVERSATION_KEY,
            json!({
                "version": DATA_VERSION,
                "title": self.title(),
                "created": self.created,
                "model": self.model,
                "provider": self.active().map(|provider| provider.id.clone()),
                "cwd": self.agent.cwd,
                "turns": turns_to_json(&self.agent.turns),
                "history": history_to_json(&self.agent.history),
                "subagents": subagents_to_json(&self.agent.subagents),
                "compacted": self.agent.compacted,
                "scopes": self.agent.scopes.to_json(),
                "planMode": self.agent.plan,
                "chats": chats
            }),
        );
        context.session = session;
        // The map is host state: written once per change, not once per frame.
        if self.agent.map.revision != self.map_saved {
            self.map_saved = self.agent.map.revision;
            context.set_value(MAP_KEY, self.agent.map.to_json());
        }
        if self.agent.system.revision != self.system_saved {
            self.system_saved = self.agent.system.revision;
            context.set_value(SYSTEM_KEY, self.agent.system.to_json());
        }
        // A run in flight, a summary or an attachment being read all finish outside a frame.
        if self.agent.busy()
            || self.summary.is_some()
            || self.picking.is_some()
            || self.fetch.is_some()
            || self.map_deep.is_some()
            || self.agent.map_scan.is_some()
        {
            context.request_repaint_after(std::time::Duration::ZERO);
        }
    }
}

#[cfg(test)]
mod tests {
    /// No secret settings in these tests: panes read them through the context.
    static EMPTY_SECRETS: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    /// Pane widths the layout tests draw at.
    const WIDE_PANE: f32 = 560.0;
    const NARROW_PANE: f32 = 120.0;
    const PROMPT_SCROLL_TOP_TOLERANCE: f32 = 20.0;
    /// Highest a pinned bubble's text may start: the transcript sits just under the
    /// pane's own controls.
    const PINNED_TOP_MAX: f32 = 100.0;

    use super::super::settings_ui::{API_KEYS_KEY, provider_secret_field};
    use super::*;
    use wassh_core::Palette;
    use wassh_ui::widgets::{COPIED_GLYPH, COPY_GLYPH};

    /// Draw two frames and return the painted text.
    fn draw(plugin: &mut AiAgent, settings: &mut serde_json::Map<String, Value>) -> Vec<String> {
        draw_with_secrets(plugin, settings, &EMPTY_SECRETS)
    }

    /// Draw with a decrypted vault behind the pane.
    fn draw_with_secrets(
        plugin: &mut AiAgent,
        settings: &mut serde_json::Map<String, Value>,
        secrets: &std::collections::BTreeMap<String, String>,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(560.0, 760.0),
            )),
            ..Default::default()
        };
        let mut painted = Vec::new();
        for _ in 0..2 {
            let mut context = PluginContext {
                palette,
                host_label: "dev".to_owned(),
                environment: "a test shell".to_owned(),
                status: SessionStatus::Connected,
                settings,
                scope: "host-1".to_owned(),
                session: None,
                events: &[],
                pending_input: Vec::new(),
                repaint_after: None,
                secrets,
                secret_writes: Vec::new(),
            };
            let mut output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            let shapes = std::mem::take(&mut output.shapes);
            painted.clear();
            for clipped in shapes {
                if let egui::Shape::Text(text) = clipped.shape {
                    painted.push(text.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
        }
        painted
    }

    /// One frame of the pane with `events`, collecting every painted text and its position.
    fn frame(
        plugin: &mut AiAgent,
        settings: &mut serde_json::Map<String, Value>,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(i32, i32, String)> {
        frame_at(plugin, settings, ctx, events, WIDE_PANE)
    }

    /// One frame of the pane drawn `width` points wide.
    fn frame_at(
        plugin: &mut AiAgent,
        settings: &mut serde_json::Map<String, Value>,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        width: f32,
    ) -> Vec<(i32, i32, String)> {
        frame_commands(plugin, settings, ctx, events, width).0
    }

    /// One frame drawn `width` points wide: every painted text and what it asked the
    /// platform to do (a clipboard copy, say).
    fn frame_commands(
        plugin: &mut AiAgent,
        settings: &mut serde_json::Map<String, Value>,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        width: f32,
    ) -> (Vec<(i32, i32, String)>, Vec<egui::OutputCommand>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(width, 760.0),
            )),
            events,
            ..Default::default()
        };
        let mut context = PluginContext {
            palette: Palette::dark(),
            host_label: "dev".to_owned(),
            environment: "a test shell".to_owned(),
            status: SessionStatus::Connected,
            settings,
            scope: "host-1".to_owned(),
            session: None,
            events: &[],
            pending_input: Vec::new(),
            repaint_after: None,
            secrets: &EMPTY_SECRETS,
            secret_writes: Vec::new(),
        };
        let mut output = ctx.run_ui(input, |ui| plugin.show(ui, &mut context));
        let commands = std::mem::take(&mut output.platform_output.commands);
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        let mut rows: Vec<_> = shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.pos.y as i32,
                    text.pos.x as i32,
                    text.galley.text().to_owned(),
                )),
                _ => None,
            })
            .collect();
        rows.sort();
        (rows, commands)
    }

    /// The events that click once at `pos`.
    fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ]
    }

    /// A provider stand-in: lists `models` and answers one chat call with `chat`, then
    /// reports the chat request it was sent.
    fn provider(
        chat: &'static str,
        models: &'static str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        answering("text/event-stream", chat, models)
    }

    /// A provider stand-in whose chat call answers with `body` of `content_type`.
    ///
    /// Summaries ask for a plain JSON completion rather than a stream.
    fn answering(
        content_type: &'static str,
        body: &'static str,
        models: &'static str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = listener.local_addr().expect("the bound address").port();
        let (sender, requests) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let request = read_request(&mut stream);
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let reply = if path.contains("models") {
                    http_response("application/json", models)
                } else {
                    http_response(content_type, body)
                };
                let _ = stream.write_all(reply.as_bytes());
                if !path.contains("models") {
                    let _ = sender.send(request);
                    break;
                }
            }
        });
        (format!("http://127.0.0.1:{port}"), requests)
    }

    /// One readable HTTP response: the body is measured, so the peer needs no chunking.
    fn http_response(content_type: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// Read one whole HTTP request: the head, then the announced body.
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        use std::io::Read;
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(read @ 1..) = stream.read(&mut chunk) {
            request.extend_from_slice(&chunk[..read]);
            let Some(head) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let head_text = String::from_utf8_lossy(&request[..head]).to_lowercase();
            let length: usize = head_text
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            if request.len() >= head + 4 + length {
                break;
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    #[test]
    fn a_prompt_is_sent_to_the_provider_and_its_answer_shown() {
        // What OpenAI-compatible providers really send: a role-only event, a blank line
        // between events, then the text.
        const CHAT: &str = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
                            data: {\"choices\":[{\"delta\":{\"content\":\"hello back\"}}]}\n\n\
                            data: [DONE]\n\n";
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, requests) = provider(CHAT, MODELS);
        let mut settings: serde_json::Map<String, Value> = [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": base_url, "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect();
        let mut plugin = AiAgent::default();
        let ctx = egui::Context::default();
        plugin.draft = "hello".to_owned();
        // The composer panel settles its height on the first frame.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let send = rows
            .iter()
            .find(|(_, _, text)| text == "Send")
            .cloned()
            .expect("the composer has a Send button");
        let pos = egui::Pos2::new(send.1 as f32 + 4.0, send.0 as f32 + 4.0);
        frame(
            &mut plugin,
            &mut settings,
            &ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
        );

        // The worker posts the completion as soon as the pane is drawn again.
        let request = requests
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the provider was never asked");
        assert!(request.contains("\"model\": \"test-model\""), "{request}");
        assert!(request.contains("\"content\": \"hello\""), "{request}");

        // And the answer lands in the transcript.
        let answered = |plugin: &AiAgent| {
            plugin.agent.turns.iter().any(
                |turn| matches!(turn, Turn::Assistant { text, .. } if text.contains("hello back")),
            )
        };
        for _ in 0..50 {
            frame(&mut plugin, &mut settings, &ctx, Vec::new());
            if answered(&plugin) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            answered(&plugin),
            "the answer never reached the transcript: {:?}",
            plugin.agent.turns
        );
    }

    #[test]
    fn an_interrupted_run_is_picked_up_by_continue() {
        const CHAT: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"resumed\"}}]}\n\n\
                            data: [DONE]\n\n";
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, requests) = provider(CHAT, MODELS);
        let mut settings: serde_json::Map<String, Value> = [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": base_url, "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect();
        let mut plugin = AiAgent::default();
        let ctx = egui::Context::default();
        // A run cut off after the prompt went out: no reply ever came back.
        assert!(plugin.agent.push_user("hello", UserMeta::default()));
        plugin.agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: true,
        });

        // The composer panel settles its height on the first frame.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let resume = rows
            .iter()
            .find(|(_, _, text)| text == "Continue")
            .cloned()
            .unwrap_or_else(|| panic!("nothing offers to continue the run: {rows:?}"));
        let pos = egui::Pos2::new(resume.1 as f32 + 4.0, resume.0 as f32 + 4.0);
        frame(
            &mut plugin,
            &mut settings,
            &ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
        );

        let request = requests
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the cut-off run was never resumed");
        assert!(request.contains("\"content\": \"hello\""), "{request}");
    }

    #[test]
    fn the_pane_opens_on_the_chat_list_with_a_hint() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        // Nothing is configured yet, so the pane says where to add a provider.
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(
            texts.contains(&"No model providers configured — add one in Options."),
            "the missing-provider hint is not shown: {texts:?}"
        );
        assert_eq!(plugin.view, ChatView::List, "the list opens when no chat does");
        assert!(
            texts.contains(&CHAT_LIST_HEADING),
            "the chat list is not shown: {texts:?}"
        );
        assert!(
            texts.contains(&CHAT_LIST_EMPTY),
            "an empty list says so: {texts:?}"
        );
        // An idle pane says nothing in the header: the title sits in the body.
        assert_eq!(plugin.header_status(), None);
        assert!(texts.contains(&"Send"), "composer missing: {texts:?}");
        assert!(
            texts.contains(&ATTACH_GLYPH),
            "attach button missing: {texts:?}"
        );
        assert!(
            texts.contains(&TERMINAL_ATTACH_GLYPH),
            "terminal-context toggle missing: {texts:?}"
        );
    }

    /// A chat stored far over its model's window, on a provider at `base_url`.
    fn chat_over_its_window(
        base_url: &str,
        auto_compact: bool,
    ) -> (serde_json::Map<String, Value>, AiAgent) {
        let mut settings: serde_json::Map<String, Value> = [
            (
                PROVIDERS_KEY.to_owned(),
                json!([{ "id": "local", "name": "Local", "protocol": "openai",
                         "baseUrl": base_url, "models": ["test-model"] }]),
            ),
            // A window small enough that the stored conversation is already over it.
            (
                SETTING_MODEL_CONTEXTS.to_owned(),
                json!([{ "model": "test-model", "tokens": 9_000 }]),
            ),
            (SETTING_AUTO_COMPACT.to_owned(), json!(auto_compact)),
        ]
        .into_iter()
        .collect();

        let filler = "detail ".repeat(400);
        let mut history = Vec::new();
        let mut turns = Vec::new();
        for step in 0..5 {
            history.push(json!({ "role": "user", "content": format!("question {step} {filler}") }));
            history.push(json!({
                "role": "assistant",
                "content": format!("answer {step} {filler}"),
                "toolCalls": []
            }));
            turns.push(json!({ "role": "user", "text": format!("question {step}") }));
            turns.push(json!({ "role": "assistant", "text": format!("answer {step}") }));
        }
        settings.insert(
            CONVERSATION_KEY.to_owned(),
            json!({
                "version": DATA_VERSION,
                "title": "Compaction test",
                "model": "test-model",
                "provider": "local",
                "cwd": "/srv/app",
                "turns": turns,
                "history": history
            }),
        );
        (settings, AiAgent::default())
    }

    #[test]
    fn a_conversation_over_the_model_window_is_compacted_into_a_summary() {
        const SUMMARY: &str = "{\"choices\":[{\"message\":{\"content\":\"Goal: ship the daemon\"}}]}";
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, requests) = answering("application/json", SUMMARY, MODELS);
        let (mut settings, mut plugin) = chat_over_its_window(&base_url, true);
        let filler = "detail ".repeat(400);
        let ctx = egui::Context::default();
        for _ in 0..50 {
            frame(&mut plugin, &mut settings, &ctx, Vec::new());
            if plugin.agent.compacted.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(
            plugin.agent.compacted.as_deref(),
            Some("Goal: ship the daemon"),
            "the summary never came back: error {:?}, pending {}, history {}",
            plugin.error,
            plugin.summary.is_some(),
            plugin.agent.history.len()
        );
        assert_eq!(
            plugin.agent.history.len(),
            COMPACT_KEEP_MESSAGES,
            "the recent messages stay in the history"
        );
        let first = plugin.agent.history.first().cloned();
        assert_eq!(
            first,
            Some(crate::ai_agent::protocol::ApiMessage::User {
                content: format!("question 2 {filler}"),
                images: Vec::new(),
            }),
            "the history keeps the messages after the fold point"
        );
        assert!(
            plugin.agent.turns.iter().any(
                |turn| matches!(turn, Turn::Notice(text) if text.starts_with(COMPACTED_NOTICE))
            ),
            "the transcript says the chat was compacted: {:?}",
            plugin.agent.turns
        );
        let request = requests
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the summariser was called");
        assert!(
            request.contains("Summarise the conversation above"),
            "the model was asked for a summary: {request}"
        );
        assert!(
            !request.contains("question 4"),
            "only what is being folded goes to the summariser: {request}"
        );
    }

    #[test]
    fn switching_compaction_off_leaves_a_long_chat_alone() {
        const SUMMARY: &str = "{\"choices\":[{\"message\":{\"content\":\"Goal: ship the daemon\"}}]}";
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, requests) = answering("application/json", SUMMARY, MODELS);
        let (mut settings, mut plugin) = chat_over_its_window(&base_url, false);
        let ctx = egui::Context::default();
        for _ in 0..5 {
            frame(&mut plugin, &mut settings, &ctx, Vec::new());
        }

        assert_eq!(plugin.agent.compacted, None, "the chat is kept whole");
        assert_eq!(plugin.agent.history.len(), 10);
        assert!(plugin.summary.is_none(), "nothing was sent to summarise");
        assert!(
            requests.try_recv().is_err(),
            "the provider was not asked for a summary"
        );
    }

    #[test]
    fn the_context_window_comes_from_the_models_own_row() {
        let mut settings = serde_json::Map::new();
        assert_eq!(
            context_window(&settings, "anything"),
            DEFAULT_CONTEXT_WINDOW_TOKENS,
            "a model with no row gets the default window"
        );
        settings.insert(
            SETTING_MODEL_CONTEXTS.to_owned(),
            json!([
                { "model": "claude", "tokens": 200_000 },
                { "model": "llama3.1", "tokens": 8_192 },
                { "model": "no-window", "tokens": 0 }
            ]),
        );
        assert_eq!(context_window(&settings, "claude-sonnet-4-5"), 200_000);
        assert_eq!(context_window(&settings, "llama3.1:8b"), 8_192);
        assert_eq!(
            context_window(&settings, "no-window"),
            DEFAULT_CONTEXT_WINDOW_TOKENS,
            "a row without a window is no row"
        );
        assert_eq!(context_window(&settings, "gpt-4o"), DEFAULT_CONTEXT_WINDOW_TOKENS);
    }

    #[test]
    fn a_message_is_parsed_once_and_only_again_when_its_text_changes() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "**bold** prompt".to_owned(),
            meta: UserMeta::default(),
        });
        plugin.parse_transcript(TranscriptSource::Main);
        assert_eq!(plugin.parsed.len(), 1);
        assert!(
            !plugin.parsed[0].blocks.is_empty(),
            "the message was parsed: {:?}",
            plugin.parsed[0].blocks
        );

        // An unchanged message keeps what was parsed, even if that is nonsense.
        plugin.parsed[0].blocks.clear();
        plugin.parse_transcript(TranscriptSource::Main);
        assert!(
            plugin.parsed[0].blocks.is_empty(),
            "an unchanged message is not parsed again"
        );

        // A growing reply is parsed again on the next frame.
        plugin.agent.turns.push(Turn::Assistant {
            text: "one".to_owned(),
            stopped: false,
        });
        plugin.parse_transcript(TranscriptSource::Main);
        assert_eq!(plugin.parsed.len(), 2);
        plugin.agent.turns[1] = Turn::Assistant {
            text: "one two".to_owned(),
            stopped: false,
        };
        plugin.parse_transcript(TranscriptSource::Main);
        assert_eq!(
            plugin.parsed[1].blocks,
            crate::ai_agent::markdown::blocks("one two")
        );
    }

    #[test]
    fn a_stored_conversation_comes_back_and_the_title_follows_it() {
        let mut settings = serde_json::Map::new();
        settings.insert(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "ollama", "name": "Ollama", "protocol": "openai",
                     "baseUrl": "http://127.0.0.1:11434/v1", "models": ["llama3.1", "qwen3"] }]),
        );
        settings.insert(
            CONVERSATION_KEY.to_owned(),
            json!({
                "version": DATA_VERSION,
                "model": "llama3.1",
                "provider": "ollama",
                "cwd": "/srv/app",
                "turns": [
                    { "role": "user", "text": "how much disk is left" },
                    { "role": "assistant", "text": "about 4 GB" }
                ],
                "history": [
                    { "role": "user", "content": "how much disk is left" },
                    { "role": "assistant", "content": "about 4 GB", "toolCalls": [] }
                ]
            }),
        );
        let mut plugin = AiAgent::default();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(
            texts.contains(&"llama3.1"),
            "the model dropdown does not show the stored model: {texts:?}"
        );
        assert!(
            texts.contains(&"how much disk is left"),
            "stored question missing: {texts:?}"
        );
        assert_eq!(plugin.model, "llama3.1");
        assert_eq!(plugin.agent.cwd, "/srv/app");
        assert_eq!(plugin.agent.history.len(), 2);
        assert_eq!(plugin.view, ChatView::Chat, "a stored chat reopens in its transcript");
    }

    #[test]
    fn the_footer_picks_the_provider_and_one_of_its_models() {
        let mut settings = serde_json::Map::new();
        settings.insert(
            PROVIDERS_KEY.to_owned(),
            json!([
                { "id": "one", "name": "First", "protocol": "openai",
                  "baseUrl": "https://one/v1", "models": ["a", "b"] },
                { "id": "two", "name": "Second", "protocol": "anthropic",
                  "baseUrl": "https://two", "models": ["c"] }
            ]),
        );
        let mut plugin = AiAgent::default();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        // First provider and its first model are what a new conversation starts on.
        assert!(
            texts.contains(&"First"),
            "provider dropdown missing: {texts:?}"
        );
        assert!(texts.contains(&"a"), "model dropdown missing: {texts:?}");
        assert_eq!(plugin.provider, 0);
        assert_eq!(plugin.model, "a");
    }

    #[test]
    fn finished_command_output_is_collapsed_by_default() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::Tool {
            name: "run_command".to_owned(),
            label: "$ uptime".to_owned(),
            output: "up 3 days".to_owned(),
            failed: false,
        });
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"$ uptime"), "label missing: {texts:?}");
        assert!(texts.contains(&"SSH"), "request type missing: {texts:?}");
        assert!(!texts.contains(&"up 3 days"), "output is not collapsed: {texts:?}");
        assert!(texts.contains(&COPY_GLYPH), "a command cannot be copied: {texts:?}");
    }

    #[test]
    fn only_shell_cards_offer_a_copy_button() {
        for (name, copies) in [
            ("web_fetch", false),
            ("remote_fs_read_file", false),
            (super::super::protocol::TOOL_RUN_SUDO, true),
            (super::super::protocol::TOOL_LOCAL_RUN, true),
        ] {
            // A fresh map each time: the pane keeps the transcript in the settings.
            let mut settings = serde_json::Map::new();
            let mut plugin = AiAgent::default();
            plugin.agent.turns.push(Turn::Tool {
                name: name.to_owned(),
                label: name.to_owned(),
                output: "some output".to_owned(),
                failed: false,
            });
            let painted = draw(&mut plugin, &mut settings);
            let has_button = painted.iter().any(|text| text == COPY_GLYPH);
            assert_eq!(has_button, copies, "{name}: {painted:?}");
        }
    }

    #[test]
    fn clicking_the_header_button_copies_the_command() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::Tool {
            name: super::super::protocol::TOOL_RUN_COMMAND.to_owned(),
            label: "$ uptime".to_owned(),
            output: "up 3 days".to_owned(),
            failed: false,
        });
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        // The transcript settles on its scroll position before the button has a rect.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let button = rows
            .iter()
            .find(|(_, _, text)| text == COPY_GLYPH)
            .cloned()
            .unwrap_or_else(|| panic!("no copy button on the card: {rows:?}"));
        let pos = egui::Pos2::new(button.1 as f32 + 4.0, button.0 as f32 + 4.0);
        let (_, commands) = frame_commands(
            &mut plugin,
            &mut settings,
            &ctx,
            click_events(pos),
            WIDE_PANE,
        );

        assert!(
            commands
                .iter()
                .any(|command| matches!(command, egui::OutputCommand::CopyText(text)
                    if text == "uptime")),
            "the command was not copied: {commands:?}"
        );
        // The confirmation is painted from the next frame on.
        let (after, _) = frame_commands(&mut plugin, &mut settings, &ctx, Vec::new(), WIDE_PANE);
        assert!(
            after.iter().any(|(_, _, text)| text == COPIED_GLYPH),
            "the button did not confirm: {after:?}"
        );
    }

    #[test]
    fn the_output_has_its_own_copy_button_on_the_next_row() {
        /// Painted text with its position, and what a copy button asked for.
        type CardFrame = (Vec<(i32, i32, String)>, Option<(CopyTarget, String)>);
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let card_id = std::cell::Cell::new(None);
        let time = std::cell::Cell::new(0.0_f64);
        let draw = |events: Vec<egui::Event>| -> CardFrame {
            let now = time.get() + 0.1;
            time.set(now);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(WIDE_PANE, 240.0),
                )),
                time: Some(now),
                events,
                ..Default::default()
            };
            let mut reply = None;
            let mut output = ctx.run_ui(input, |ui| {
                card_id.set(Some(ui.make_persistent_id(("ai-command-output", 7))));
                reply = Some(tool_card(
                    ui,
                    &palette,
                    super::super::protocol::TOOL_RUN_COMMAND,
                    "$ uptime",
                    "up 3 days",
                    ToolCardState {
                        failed: false,
                        running: false,
                        turn_index: 7,
                        copied: None,
                    },
                ));
            });
            let mut rows: Vec<_> = std::mem::take(&mut output.shapes)
                .into_iter()
                .filter_map(|clipped| match clipped.shape {
                    egui::Shape::Text(text) => Some((
                        text.pos.y as i32,
                        text.pos.x as i32,
                        text.galley.text().to_owned(),
                    )),
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            rows.sort();
            (rows, reply.and_then(|reply| reply.copy))
        };
        draw(Vec::new());
        // The card starts folded to its header: open it before looking for the output row.
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            &ctx,
            card_id.get().expect("the card's collapsing id"),
            false,
        );
        state.set_open(true);
        state.store(&ctx);
        // The body slides open: let the animation finish before clicking into it.
        draw(Vec::new());
        let (rows, _) = draw(Vec::new());
        let buttons: Vec<_> = rows
            .iter()
            .filter(|(_, _, text)| text == COPY_GLYPH)
            .cloned()
            .collect();
        assert_eq!(
            buttons.len(),
            2,
            "the card needs a command and an output button: {rows:?}"
        );
        let output_text = rows
            .iter()
            .find(|(_, _, text)| text == "up 3 days")
            .cloned()
            .expect("the output text");
        let output_button = &buttons[1];
        assert!(
            (output_button.0 - output_text.0).abs() <= 8,
            "the output button is not level with the first output line: {rows:?}"
        );

        let (_, copied) = draw(click_events(egui::Pos2::new(
            output_button.1 as f32 + 4.0,
            output_button.0 as f32 + 4.0,
        )));
        assert_eq!(
            copied,
            Some((CopyTarget::Output, "up 3 days".to_owned())),
            "the output button did not copy the output"
        );
    }

    #[test]
    fn a_shell_card_copies_the_command_without_its_prompt() {
        assert_eq!(super::command_text("$ uptime"), "uptime");
        assert_eq!(super::command_text("$ sudo apt update"), "sudo apt update");
        assert_eq!(super::command_text("local $ ls -la"), "ls -la");
        // A command that happens to start with the Local marker keeps it.
        assert_eq!(super::command_text("$ local echo hi"), "local echo hi");
    }

    #[test]
    fn the_permission_card_offers_a_session_wide_allow() {
        let mut plugin = AiAgent::default();
        plugin.agent.phase = Some(Phase::Allowing {
            call: super::super::protocol::ApiToolCall {
                id: "c1".to_owned(),
                name: super::super::protocol::TOOL_TERMINAL_WRITE.to_owned(),
                arguments: r#"{"text":"ls"}"#.to_owned(),
            },
            label: "terminal_write".to_owned(),
            turn: 0,
        });
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"Approve action?"), "card missing: {texts:?}");
        assert!(
            texts.contains(&"Allow all this session"),
            "no session-wide allow: {texts:?}"
        );
    }

    #[test]
    fn the_approval_card_names_the_command_and_offers_scopes() {
        let mut plugin = AiAgent::default();
        plugin.agent.phase = Some(Phase::Approving {
            call: super::super::protocol::ApiToolCall {
                id: "c1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"systemctl restart nginx"}"#.to_owned(),
            },
            command: "systemctl restart nginx".to_owned(),
            scopes: super::super::rules::approval_scopes("systemctl restart nginx"),
        });
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(
            texts.contains(&"Run this command?"),
            "card missing: {texts:?}"
        );
        assert!(
            texts.contains(&"systemctl restart nginx"),
            "command missing: {texts:?}"
        );
        assert!(texts.contains(&"Allow"), "allow missing: {texts:?}");
        assert!(texts.contains(&"Deny"), "deny missing: {texts:?}");
        assert!(
            texts.contains(&"Allow all this session"),
            "allow-all missing: {texts:?}"
        );
    }

    #[test]
    fn permission_actions_keep_the_remembered_choices_on_their_own_line() {
        let mut plugin = AiAgent::default();
        plugin.agent.phase = Some(Phase::Allowing {
            call: super::super::protocol::ApiToolCall {
                id: "permission-row".to_owned(),
                name: super::super::protocol::TOOL_TERMINAL_WRITE.to_owned(),
                arguments: r#"{"text":"ls"}"#.to_owned(),
            },
            label: "terminal_write".to_owned(),
            turn: 0,
        });
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        let permission_rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert_actions_share_row(&permission_rows, &["Approve once", "Deny once"]);
        assert_actions_share_row(&permission_rows, &["Always allow", "Always deny"]);
        let once = row_y(&permission_rows, "Approve once");
        let always = row_y(&permission_rows, "Always allow");
        let session = row_y(&permission_rows, "Allow all this session");
        assert!(always > once, "the remembered choices are below the once ones");
        assert!(
            session > always,
            "the session-wide allow is on its own line: {permission_rows:?}"
        );

        plugin.agent.phase = Some(Phase::Approving {
            call: super::super::protocol::ApiToolCall {
                id: "approval-row".to_owned(),
                name: super::super::protocol::TOOL_RUN_COMMAND.to_owned(),
                arguments: r#"{"command":"systemctl restart nginx"}"#.to_owned(),
            },
            command: "systemctl restart nginx".to_owned(),
            scopes: super::super::rules::approval_scopes("systemctl restart nginx"),
        });
        let approval_rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert_actions_share_row(&approval_rows, &["Allow", "Deny"]);
        assert_eq!(row_y(&approval_rows, "Allow"), row_y(&approval_rows, "Deny"));
        assert!(
            row_y(&approval_rows, "Allow all this session") > row_y(&approval_rows, "Allow"),
            "the session-wide allow is on its own line: {approval_rows:?}"
        );
        assert_actions_stack_below(&approval_rows, &["Allow", "Deny", "Allow all this session"]);
    }

    /// The y of the painted `label`.
    fn row_y(rows: &[(i32, i32, String)], label: &str) -> i32 {
        rows.iter()
            .find(|(_, _, text)| text == label)
            .unwrap_or_else(|| panic!("{label} missing from {rows:?}"))
            .0
    }

    fn assert_actions_share_row(rows: &[(i32, i32, String)], labels: &[&str]) {
        let positions: Vec<i32> = labels
            .iter()
            .map(|label| {
                rows.iter()
                    .find(|(_, _, text)| text == label)
                    .unwrap_or_else(|| panic!("{label} missing from {rows:?}"))
                    .0
            })
            .collect();
        let top = *positions.iter().min().expect("at least one action");
        let bottom = *positions.iter().max().expect("at least one action");
        assert!(bottom - top <= 3, "actions span multiple rows: {rows:?}");
    }

    fn assert_actions_stack_below(rows: &[(i32, i32, String)], row_labels: &[&str]) {
        let row_bottom = row_labels
            .iter()
            .filter_map(|label| rows.iter().find(|(_, _, text)| text == label))
            .map(|(y, _, _)| *y)
            .max()
            .expect("at least one row action");
        let stacked: Vec<_> = rows
            .iter()
            .filter(|(_, _, text)| text.starts_with("Always allow"))
            .collect();
        assert!(!stacked.is_empty(), "no scope actions: {rows:?}");
        assert!(
            stacked.iter().all(|(y, _, _)| *y > row_bottom),
            "scope actions are not below the main row: {rows:?}"
        );
        assert!(
            stacked.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "scope actions are not vertically stacked: {rows:?}"
        );
    }

    #[test]
    fn a_prompt_card_stays_a_bounded_popup() {
        // A model can ask to run a whole script. The card must stay a card: it is drawn on
        // its own background above the composer, and the command is capped instead of
        // pushing the conversation and the send button out of the pane.
        let mut plugin = AiAgent::default();
        let script = format!("cd ~/app && python3 - <<'PY'\n{}\nPY", "print(1)\n".repeat(120));
        let scopes = super::super::rules::approval_scopes(&script);
        plugin.agent.phase = Some(Phase::Approving {
            call: super::super::protocol::ApiToolCall {
                id: "c1".to_owned(),
                name: "run_command".to_owned(),
                arguments: json!({ "command": script }).to_string(),
            },
            command: script.clone(),
            scopes: scopes.clone(),
        });
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(WIDE_PANE, 760.0),
            )),
            ..Default::default()
        };
        let mut context = PluginContext {
            palette: Palette::dark(),
            host_label: "dev".to_owned(),
            environment: "a test shell".to_owned(),
            status: SessionStatus::Connected,
            settings: &mut settings,
            scope: "host-1".to_owned(),
            session: None,
            events: &[],
            pending_input: Vec::new(),
            repaint_after: None,
            secrets: &EMPTY_SECRETS,
            secret_writes: Vec::new(),
        };
        let mut texts = Vec::new();
        let mut cards = Vec::new();
        let mut request_viewport = None;
        let mut request_position = None;
        let mut request_title_position = None;
        // The bottom panel settles its height over a few frames.
        for _ in 0..4 {
            let mut output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            texts.clear();
            cards.clear();
            request_viewport = None;
            request_position = None;
            request_title_position = None;
            for clipped in std::mem::take(&mut output.shapes) {
                let clip_rect = clipped.clip_rect;
                match clipped.shape {
                    egui::Shape::Text(text) => {
                        let label = text.galley.text().to_owned();
                        if label == "Run this command?" {
                            request_title_position = Some(text.pos);
                        }
                        if label.starts_with("cd ~/app") {
                            request_viewport = Some(clip_rect);
                            request_position = Some(text.pos);
                        }
                        texts.push((text.pos, label));
                    }
                    egui::Shape::Rect(rect)
                        if rect.fill == to_color32(Palette::dark().bg_panel_2) =>
                    {
                        cards.push(rect.rect);
                    }
                    _ => {}
                }
            }
            output.drop_without_applying_deltas();
        }

        let title = texts
            .iter()
            .find(|(_, text)| text == "Run this command?")
            .map(|(pos, _)| *pos)
            .expect("the approval card is drawn");
        let card = cards
            .iter()
            .find(|card| card.contains(title))
            .copied()
            .expect("the card has no background of its own");

        // The composer is still on screen, well under the 760-point pane.
        let stop = texts
            .iter()
            .find(|(_, text)| text == "Stop")
            .map(|(pos, _)| *pos)
            .expect("the stop button is drawn");
        assert!(stop.y < 760.0, "the card pushed the composer away");
        // The card itself stays a card, not a whole screen of script.
        for card in &cards {
            assert!(card.height() < 760.0 / 2.0, "the card grew to {card:?}");
        }
        assert!(
            card.height() <= PROMPT_CARD_MAX_HEIGHT,
            "the card grew past its cap: {card:?}"
        );

        // The command scrolls vertically; the single-row action strip scrolls horizontally.
        let button = |label: &str| {
            texts
                .iter()
                .find(|(_, text)| text == label)
                .map(|(pos, _)| *pos)
                .unwrap_or_else(|| panic!("{label} was scrolled out of the card"))
        };
        let allow = button("Allow");
        let deny = button("Deny");
        let allow_session = button("Allow all this session");
        let always_allow: Vec<_> = texts
            .iter()
            .filter(|(_, text)| text.starts_with("Always allow"))
            .collect();
        assert_eq!(
            always_allow.len(),
            scopes.len(),
            "not all approval scopes are visible: {texts:?}"
        );
        let request_viewport = request_viewport.expect("the request has a scroll viewport");
        assert!(
            card.contains(request_viewport.min) && card.contains(request_viewport.max),
            "the request viewport left the card: {request_viewport:?} in {card:?}"
        );
        let request_position = request_position.expect("the request is drawn");
        let request_title_position =
            request_title_position.expect("the request title is drawn");
        assert!(
            request_title_position.y >= request_viewport.top()
                && request_title_position.y
                    <= request_viewport.top() + PROMPT_SCROLL_TOP_TOLERANCE,
            "request title is not at the top of its viewport: {request_title_position:?} in {request_viewport:?}"
        );
        assert!(
            request_position.y > request_title_position.y,
            "request title must precede the command: {request_title_position:?}, {request_position:?}"
        );
        let action_positions: Vec<_> = std::iter::once(&allow)
            .chain(std::iter::once(&deny))
            .chain(std::iter::once(&allow_session))
            .chain(always_allow.iter().map(|(position, _)| position))
            .collect();
        for position in action_positions {
            assert!(
                position.y >= card.top() && position.y <= card.bottom(),
                "a control left the card: {position:?} in {card:?}"
            );
        }
    }

    #[test]
    fn the_question_card_asks_and_takes_an_answer() {
        let mut plugin = AiAgent::default();
        plugin.agent.phase = Some(Phase::Asking {
            call: super::super::protocol::ApiToolCall {
                id: "c1".to_owned(),
                name: "ask_user".to_owned(),
                arguments: r#"{"text":"Which file?"}"#.to_owned(),
            },
            questions: vec![super::super::agent::AgentQuestion {
                text: "Which file?".to_owned(),
                options: Vec::new(),
            }],
        });
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(
            texts.contains(&"Which file?"),
            "question missing: {texts:?}"
        );
        assert!(
            texts.contains(&"Your answer…"),
            "answer box missing: {texts:?}"
        );
        assert!(texts.contains(&"Answer"), "answer button missing: {texts:?}");
    }

    #[test]
    fn a_question_card_preserves_composer_focus() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let (y, x, _) = rows
            .iter()
            .find(|(_, _, text)| text == "Ask the agent…")
            .expect("composer placeholder");
        let position = egui::Pos2::new(*x as f32 + 5.0, *y as f32 + 5.0);
        frame(
            &mut plugin,
            &mut settings,
            &ctx,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
        );
        let composer_focus = ctx.memory(|memory| memory.focused());
        assert!(composer_focus.is_some(), "the composer did not gain focus");

        plugin.agent.phase = Some(Phase::Asking {
            call: super::super::protocol::ApiToolCall {
                id: "focus-test".to_owned(),
                name: "ask_user".to_owned(),
                arguments: r#"{"text":"Which file?"}"#.to_owned(),
            },
            questions: vec![super::super::agent::AgentQuestion {
                text: "Which file?".to_owned(),
                options: Vec::new(),
            }],
        });
        frame(&mut plugin, &mut settings, &ctx, Vec::new());

        assert_eq!(ctx.memory(|memory| memory.focused()), composer_focus);
    }

    #[test]
    fn a_single_line_question_card_stays_snug() {
        // The card is capped, not padded: a question with no options must not open a
        // popup half a pane tall with empty space under it.
        let mut plugin = AiAgent::default();
        plugin.agent.phase = Some(Phase::Asking {
            call: super::super::protocol::ApiToolCall {
                id: "c1".to_owned(),
                name: "ask_user".to_owned(),
                arguments: r#"{"text":"Which file?"}"#.to_owned(),
            },
            questions: vec![super::super::agent::AgentQuestion {
                text: "Which file?".to_owned(),
                options: Vec::new(),
            }],
        });
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(WIDE_PANE, 760.0),
            )),
            ..Default::default()
        };
        let mut title = None;
        let mut cards = Vec::new();
        // The bottom panel settles its height over a few frames.
        for _ in 0..4 {
            let mut context = PluginContext {
                palette: Palette::dark(),
                host_label: "dev".to_owned(),
                environment: "a test shell".to_owned(),
                status: SessionStatus::Connected,
                settings: &mut settings,
                scope: "host-1".to_owned(),
                session: None,
                events: &[],
                pending_input: Vec::new(),
                repaint_after: None,
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let mut output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            title = None;
            cards.clear();
            for clipped in std::mem::take(&mut output.shapes) {
                match clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Which file?" => {
                        title = Some(text.pos);
                    }
                    egui::Shape::Rect(rect)
                        if rect.fill == to_color32(Palette::dark().bg_panel_2) =>
                    {
                        cards.push(rect.rect);
                    }
                    _ => {}
                }
            }
            output.drop_without_applying_deltas();
        }

        let title = title.expect("the question card is drawn");
        let card = *cards
            .iter()
            .find(|card| card.contains(title))
            .expect("the card has no background of its own");
        assert!(
            card.height() < PROMPT_CARD_MIN_HEIGHT,
            "the card padded itself out to {card:?}"
        );
    }

    /// The painted row whose text is `text`.
    fn row_of(rows: &[(i32, i32, String)], text: &str) -> (i32, i32, String) {
        rows.iter()
            .find(|(_, _, painted)| painted == text)
            .unwrap_or_else(|| panic!("{text} is not painted: {rows:?}"))
            .clone()
    }

    /// A bare key press.
    fn key(key: egui::Key) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    /// A pane holding two prompts, each with its reply.
    fn two_prompts() -> AiAgent {
        let mut plugin = AiAgent::default();
        for (prompt, reply) in [("first prompt", "a reply"), ("second prompt", "another reply")] {
            plugin.agent.turns.push(Turn::User {
                text: prompt.to_owned(),
                meta: UserMeta::default(),
            });
            plugin.agent.turns.push(Turn::Assistant {
                text: reply.to_owned(),
                stopped: false,
            });
        }
        plugin
    }

    #[test]
    fn clicking_a_prompt_opens_it_for_editing() {
        let mut plugin = two_prompts();
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        // The bottom panel and the scroll area settle over the first frames.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());

        frame(&mut plugin, &mut settings, &ctx, click(row_of(&rows, "second prompt")));
        assert_eq!(plugin.editing.as_ref().map(|edit| edit.index), Some(2));

        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let texts: Vec<&str> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        assert!(
            texts.contains(&EDITOR_HINT),
            "the editor says which keys it answers to: {texts:?}"
        );
        assert!(
            texts.contains(&"second prompt"),
            "the prompt is in the field: {texts:?}"
        );

        // Escape puts the message back as it was.
        frame(&mut plugin, &mut settings, &ctx, key(egui::Key::Escape));
        assert!(plugin.editing.is_none());
        assert_eq!(plugin.agent.turns.len(), 4);
    }

    #[test]
    fn sending_an_edited_prompt_restarts_the_chat_from_it() {
        let mut plugin = two_prompts();
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        frame(&mut plugin, &mut settings, &ctx, click(row_of(&rows, "second prompt")));
        let edit = plugin.editing.as_mut().expect("the prompt is being edited");
        edit.text = "rewritten prompt".to_owned();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());

        // Enter, in the focused field, sends it and drops what followed.
        frame(&mut plugin, &mut settings, &ctx, key(egui::Key::Enter));
        assert!(plugin.editing.is_none(), "the editor closed");
        assert_eq!(plugin.agent.turns.len(), 3, "{:?}", plugin.agent.turns);
        assert!(matches!(
            &plugin.agent.turns[1],
            Turn::Assistant { text, .. } if text == "a reply"
        ));
        assert!(matches!(
            &plugin.agent.turns[2],
            Turn::User { text, .. } if text == "rewritten prompt"
        ));
    }

    /// Click the painted `text` and draw the frame that shows the result.
    fn click_and_draw(
        plugin: &mut AiAgent,
        settings: &mut serde_json::Map<String, Value>,
        ctx: &egui::Context,
        rows: &[(i32, i32, String)],
        text: &str,
    ) -> Vec<(i32, i32, String)> {
        frame(plugin, settings, ctx, click(row_of(rows, text)));
        frame(plugin, settings, ctx, Vec::new())
    }

    #[test]
    fn a_question_card_advances_on_a_pick_and_only_the_last_one_submits() {
        let mut plugin = AiAgent::default();
        plugin.agent.phase = Some(Phase::Asking {
            call: super::super::protocol::ApiToolCall {
                id: "c1".to_owned(),
                name: "ask_user".to_owned(),
                arguments: String::new(),
            },
            questions: vec![
                super::super::agent::AgentQuestion {
                    text: "Which language?".to_owned(),
                    options: vec!["Rust".to_owned(), "Go".to_owned()],
                },
                super::super::agent::AgentQuestion {
                    text: "Which file?".to_owned(),
                    options: vec!["main.rs".to_owned(), "lib.rs".to_owned()],
                },
            ],
        });
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        // The bottom panel settles its height over the first frames.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let texts: Vec<&str> = rows.iter().map(|(_, _, text)| text.as_str()).collect();

        for expected in [
            "Question 1 of 2",
            "Which language?",
            "Rust",
            "Go",
            "Your answer…",
            "Previous",
            "Next",
            "Stop",
        ] {
            assert!(texts.contains(&expected), "{expected} missing: {texts:?}");
        }
        assert!(!texts.contains(&"Answer"), "not on the final question");
        assert!(
            !texts.contains(&"Other (write your own)"),
            "the text field is the free answer: {texts:?}"
        );
        // Picking an option is the answer: the card moves on by itself.
        let rows = click_and_draw(&mut plugin, &mut settings, &ctx, &rows, "Go");
        let texts: Vec<&str> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        assert!(texts.contains(&"Question 2 of 2"), "{texts:?}");
        assert!(texts.contains(&"Which file?"), "{texts:?}");
        assert!(
            texts.contains(&"Answer"),
            "the last question submits: {texts:?}"
        );

        // Previous goes back with the answer still chosen, and the pick can change.
        let rows = click_and_draw(&mut plugin, &mut settings, &ctx, &rows, "Previous");
        assert_eq!(plugin.questions.page, 0);
        assert_eq!(plugin.questions.drafts[0].chosen, Some(1));
        let rows = click_and_draw(&mut plugin, &mut settings, &ctx, &rows, "Rust");
        assert_eq!(plugin.questions.page, 1);
        assert_eq!(plugin.questions.drafts[0].chosen, Some(0));

        // The last question only records the pick; the Answer button submits.
        let rows = click_and_draw(&mut plugin, &mut settings, &ctx, &rows, "lib.rs");
        assert!(plugin.agent.asking(), "a pick on the last question waits");
        assert_eq!(plugin.questions.drafts[1].chosen, Some(1));
        click_and_draw(&mut plugin, &mut settings, &ctx, &rows, "Answer");

        assert!(!plugin.agent.asking());
        let answers: Vec<&str> = plugin
            .agent
            .turns
            .iter()
            .filter_map(|turn| match turn {
                Turn::Question { answer, .. } => answer.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(answers, ["Rust", "lib.rs"]);

        plugin.agent.phase = Some(Phase::Asking {
            call: super::super::protocol::ApiToolCall {
                id: "c2".to_owned(),
                name: "ask_user".to_owned(),
                arguments: String::new(),
            },
            questions: vec![super::super::agent::AgentQuestion {
                text: "Stop this?".to_owned(),
                options: vec!["Yes".to_owned()],
            }],
        });
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        plugin.agent.stop();
        assert!(
            !plugin.agent.asking(),
            "Stop ends the question prompt: asking={}, blocked={}, busy={}",
            plugin.agent.asking(),
            plugin.agent.blocked(),
            plugin.agent.busy()
        );
    }

    #[test]
    fn the_footer_switches_the_chat_between_agent_and_plan() {
        let mut settings = serde_json::Map::new();
        let mut plugin = AiAgent::default();
        let ctx = egui::Context::default();
        // The composer panel settles its height on the first frame.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let mode = rows
            .iter()
            .find(|(_, _, text)| text == MODE_AGENT_LABEL)
            .cloned()
            .unwrap_or_else(|| panic!("the footer offers the mode: {rows:?}"));
        assert!(!plugin.agent.plan, "a chat starts in Agent mode");
        frame(&mut plugin, &mut settings, &ctx, click(mode));
        assert!(plugin.agent.plan, "the mode button did not switch to Plan");
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            rows.iter().any(|(_, _, text)| text == MODE_PLAN_LABEL),
            "the button names the mode the chat is in: {rows:?}"
        );
        // And the pane keeps it with the conversation.
        assert_eq!(settings[CONVERSATION_KEY]["planMode"], json!(true));
    }

    #[test]
    fn a_narrow_footer_drops_the_mode_and_compact_buttons_first() {
        let mut settings = serde_json::Map::new();
        let mut plugin = AiAgent::default();
        let ctx = egui::Context::default();
        frame_at(&mut plugin, &mut settings, &ctx, Vec::new(), NARROW_PANE);
        let rows = frame_at(&mut plugin, &mut settings, &ctx, Vec::new(), NARROW_PANE);
        assert!(
            !rows.iter().any(|(_, _, text)| text == COMPACT_LABEL),
            "a 120-point pane has no room for the buttons: {rows:?}"
        );
    }

    #[test]
    fn the_compact_command_is_the_only_way_to_fold_when_automatic_compaction_is_off() {
        let (base_url, _requests) = provider("", "{\"data\":[]}");
        let mut settings: serde_json::Map<String, Value> = [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": base_url, "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect();
        let mut plugin = AiAgent {
            model: "test-model".to_owned(),
            ..AiAgent::default()
        };
        plugin.agent.cwd = "/srv".to_owned();
        // One frame loads the provider and keys the settings hold, as the pane does.
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        plugin.model = "test-model".to_owned();
        for step in 0..8 {
            assert!(plugin.agent.push_user(&format!("question {step}"), UserMeta::default()));
        }

        // Automatic compaction is off by default, however long the chat is.
        assert!(
            !settings
                .get(SETTING_AUTO_COMPACT)
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
        plugin.compact_if_needed(&settings, "dev");
        assert!(plugin.summary.is_none(), "compaction ran on its own");

        // Asking for it folds the older part, and says so when there is nothing to fold.
        plugin.run_command(COMPACT_COMMAND, &settings, "dev");
        assert!(plugin.summary.is_some(), "/compact did not start a summary");
        plugin.summary = None;
        let mut fresh = AiAgent {
            providers: plugin.providers.clone(),
            model: plugin.model.clone(),
            ..AiAgent::default()
        };
        assert!(fresh.agent.push_user("only question", UserMeta::default()));
        fresh.run_command(COMPACT_COMMAND, &settings, "dev");
        assert!(fresh.summary.is_none(), "a short chat was folded anyway");
        assert!(
            fresh
                .agent
                .turns
                .iter()
                .any(|turn| matches!(turn, Turn::Notice(text) if text == COMPACT_EMPTY)),
            "the pane does not say why nothing happened: {:?}",
            fresh.agent.turns
        );
    }

    #[test]
    fn the_mode_commands_switch_the_chat() {
        let settings = serde_json::Map::new();
        let mut plugin = AiAgent::default();
        assert!(!plugin.agent.plan, "a chat starts in Agent mode");
        plugin.run_command(PLAN_COMMAND, &settings, "dev");
        assert!(plugin.agent.plan, "/plan did not switch the chat");
        assert!(plugin.draft.is_empty(), "the command stayed in the composer");
        plugin.run_command(AGENT_COMMAND, &settings, "dev");
        assert!(!plugin.agent.plan, "/agent did not switch it back");
    }

    #[test]
    fn slash_commands_parse_like_the_reference() {
        let exact = parse_slash_input("/new").unwrap();
        assert_eq!(exact.exact.map(|command| command.name), Some("new"));
        let compact = parse_slash_input("/compact").unwrap();
        assert_eq!(compact.exact.map(|command| command.name), Some("compact"));
        let plan = parse_slash_input("/plan").unwrap();
        assert_eq!(plan.exact.map(|command| command.name), Some("plan"));
        let agent = parse_slash_input("/agent").unwrap();
        assert_eq!(agent.exact.map(|command| command.name), Some("agent"));
        let partial = parse_slash_input("/sum").unwrap();
        assert_eq!(partial.prefix, "sum");
        assert_eq!(partial.matches.len(), 1);
        assert_eq!(partial.matches[0].name, "summary");
        // Anything with a space, or not starting with a slash, is a message.
        assert!(parse_slash_input("/summary please").is_none());
        assert!(parse_slash_input("hello").is_none());
        assert!(
            parse_slash_input("/nonexistent")
                .unwrap()
                .matches
                .is_empty()
        );
    }

    #[test]
    fn an_image_attachment_is_sent_as_an_image() {
        let dir = std::env::temp_dir().join(format!("wassh-ai-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp folder");
        let path = dir.join("dot.png");
        std::fs::write(
            &path,
            [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0],
        )
        .expect("the image");
        let mut plugin = AiAgent::default();
        plugin.attach(path.to_str().expect("a path"));
        let images = plugin.attached_images();
        assert_eq!(images.len(), 1, "{images:?}");
        assert_eq!(images[0].media_type, "image/png");
        let message = plugin.message_with_attachments("look", None);
        assert!(
            message.starts_with("[Attached image dot.png]"),
            "{message}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn attachments_and_the_terminal_tail_are_folded_into_the_message() {
        let mut plugin = AiAgent::default();
        plugin.attachments.push(Attachment {
            path: "/tmp/notes.txt".to_owned(),
            text: "line one".to_owned(),
            image: None,
        });
        let message =
            plugin.message_with_attachments("summarise this", Some("$ uptime\nup 3 days"));
        assert!(
            message.starts_with("[Attached file notes.txt]\nline one"),
            "{message}"
        );
        assert!(message.contains("summarise this"), "{message}");
        assert!(
            message.ends_with("[Recent terminal output]\n$ uptime\nup 3 days"),
            "terminal context missing: {message}"
        );
        assert!(
            plugin
                .message_with_attachments("plain", None)
                .contains("Attached")
        );

        // A blank terminal still says so, like the reference.
        let empty = plugin.message_with_attachments("plain", Some("  "));
        assert!(empty.ends_with("[Recent terminal output]\n[No terminal output captured yet.]"));
    }

    /// The bubble is right-aligned, but its blocks must still stack: laid out in the
    /// right-to-left parent, the tags, the prompt and the terminal tail shared one row,
    /// and the tail was wrapped into the sliver left beside them.
    #[test]
    fn the_terminal_tail_wraps_inside_the_bubble() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(WIDE_PANE, 760.0),
            )),
            ..Default::default()
        };
        let plugin = AiAgent::default();
        let message = plugin.message_with_attachments(
            "look at this",
            Some("aya@Luna ~/Documents/dev/ayalabrainchild\n$ ls -la\nfile1 file2"),
        );
        let mut texts: Vec<(i32, f32, String)> = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                user_bubble(
                    ui,
                    &Palette::dark(),
                    &crate::ai_agent::markdown::blocks(&message),
                    UserMeta {
                        attachments: 0,
                        terminal: true,
                    },
                );
            });
            let shapes = std::mem::take(&mut output.shapes);
            output.drop_without_applying_deltas();
            texts = shapes
                .into_iter()
                .filter_map(|clipped| match clipped.shape {
                    egui::Shape::Text(text) => Some((
                        text.pos.y as i32,
                        text.galley.rect.width(),
                        text.galley.text().to_owned(),
                    )),
                    _ => None,
                })
                .collect();
        }

        let (_, width, _) = texts
            .iter()
            .find(|(_, _, text)| text.contains(TERMINAL_CONTEXT_MARKER))
            .expect("the tail is drawn");
        assert!(
            *width > WIDE_PANE / 2.0,
            "the tail must use the bubble's width, not a sliver: {texts:?}"
        );
        let rows: Vec<i32> = texts.iter().map(|(y, _, _)| *y).collect();
        assert!(
            rows.windows(2).all(|pair| pair[0] < pair[1]),
            "the bubble must stack its blocks: {texts:?}"
        );
    }

    #[test]
    fn chat_bubbles_fit_long_text_inside_the_pane() {
        let palette = Palette::dark();
        let user_fill = to_color32(palette.accent);
        for width in [NARROW_PANE, WIDE_PANE] {
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 3000.0))),
                ..Default::default()
            };
            let prompt = format!("Please inspect this:\n\n```\n{}\n```", "long-path".repeat(30));
            let terminal = format!("terminal output {}", "long-line".repeat(30));
            let mut right = 0.0;
            let mut shapes = Vec::new();
            for _ in 0..2 {
                let mut output = ctx.run_ui(input.clone(), |ui| {
                    right = ui.max_rect().right();
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        user_bubble(
                            ui,
                            &palette,
                            &crate::ai_agent::markdown::blocks(&prompt),
                            UserMeta::default(),
                        );
                        tool_card(ui, &palette, "run_command", &"long-command".repeat(30), &terminal, ToolCardState {
                            failed: false, running: false, turn_index: 0, copied: None,
                        });
                        // A folded card hides its body, so the long output is painted on its own.
                        tool_output(ui, &palette, &terminal, true, false);
                        tool_card(ui, &palette, "run_command", &"long-command".repeat(30), &terminal, ToolCardState {
                            failed: false, running: false, turn_index: 1, copied: None,
                        });
                    });
                });
                shapes = std::mem::take(&mut output.shapes);
                output.drop_without_applying_deltas();
            }
            let mut found = 0;
            for clipped in shapes {
                match clipped.shape {
                    egui::Shape::Rect(rect) if rect.fill == user_fill || rect.fill == to_color32(palette.bg_input) => {
                        found += 1;
                        let limit = right.min(clipped.clip_rect.right());
                        assert!(rect.rect.right() <= limit, "bubble exceeds pane: {:?} > {limit}", rect.rect);
                    }
                    egui::Shape::Text(text) if text.galley.text().contains("long-path") || text.galley.text().contains("long-line") => {
                        found += 1;
                        let limit = right.min(clipped.clip_rect.right());
                        assert!(text.pos.x + text.galley.size().x <= limit, "text exceeds pane: {:?} > {limit}", text.pos.x + text.galley.size().x);
                    }
                    _ => {}
                }
            }
            assert!(found >= 5, "expected the bubbles and their contents, found {found}");
        }
    }

    #[test]
    fn chat_transcript_stays_inside_the_plugin_pane() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let body = egui::Rect::from_min_size(egui::pos2(75.0, 20.0), egui::vec2(WIDE_PANE, 700.0));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 760.0))),
            ..Default::default()
        };
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: format!("Please inspect {}", "long-path".repeat(30)),
            meta: UserMeta::default(),
        });
        plugin.agent.turns.push(Turn::Tool {
            name: "web_fetch".to_owned(),
            label: "long-command".repeat(30),
            output: format!("terminal output {}", "long-line".repeat(30)),
            failed: false,
        });
        plugin.agent.turns.push(Turn::Tool {
            name: super::super::protocol::TOOL_RUN_COMMAND.to_owned(),
            label: "long-command".repeat(30),
            output: "finished".to_owned(),
            failed: false,
        });
        let mut settings = serde_json::Map::new();
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut context = PluginContext {
                palette, host_label: "dev".to_owned(), environment: "test".to_owned(),
                status: SessionStatus::Connected, settings: &mut settings, scope: "host-1".to_owned(),
                session: None, events: &[], pending_input: Vec::new(), repaint_after: None,
                secrets: &EMPTY_SECRETS, secret_writes: Vec::new(),
            };
            let mut output = ctx.run_ui(input.clone(), |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
                child.set_clip_rect(body);
                plugin.show(&mut child, &mut context);
            });
            shapes = std::mem::take(&mut output.shapes);
            output.drop_without_applying_deltas();
        }
        let mut found = 0;
        for clipped in shapes {
            if let egui::Shape::Rect(rect) = &clipped.shape
                && rect.fill == to_color32(palette.bg_input)
            {
                assert!(rect.rect.right() <= body.right().min(clipped.clip_rect.right()), "tool card exceeds pane: {:?} > {:?}", rect.rect, clipped.clip_rect);
            }
            if let egui::Shape::Text(text) = clipped.shape
                && (text.galley.text().contains("long-path")
                    || text.galley.text().contains("long-line")
                    || text.galley.text().contains("long-command"))
            {
                found += 1;
                let limit = body.right().min(clipped.clip_rect.right());
                assert!(text.pos.x + text.galley.size().x <= limit, "text exceeds pane: {} > {limit}", text.pos.x + text.galley.size().x);
            }
        }
        assert!(found >= 2, "the prompt and the cards stay inside: {found}");
    }

    #[test]
    fn an_assistant_response_uses_a_distinct_left_aligned_bubble() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(WIDE_PANE, 200.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            assistant_bubble(
                ui,
                &palette,
                &crate::ai_agent::markdown::blocks("Agent response"),
            );
        });
        let mut bubble = None;
        let mut response = None;
        for clipped in std::mem::take(&mut output.shapes) {
            match clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == to_color32(palette.bg_panel_2) => {
                    bubble = Some(rect.rect);
                }
                egui::Shape::Text(text) if text.galley.text() == "Agent response" => {
                    response = Some(text.pos);
                }
                _ => {}
            }
        }
        output.drop_without_applying_deltas();

        let bubble = bubble.expect("the assistant bubble has a neutral panel fill");
        let response = response.expect("the assistant response is drawn");
        assert_ne!(to_color32(palette.bg_panel_2), to_color32(palette.accent));
        assert!(bubble.left() < WIDE_PANE / 2.0, "bubble should start on the left");
        assert!(
            response.x >= bubble.left() && response.x < bubble.center().x,
            "response text should be left-aligned inside its bubble: {response:?} in {bubble:?}"
        );
    }

    #[test]
    fn a_user_bubble_is_an_accent_surface_with_white_ink() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(WIDE_PANE, 200.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            user_bubble(
                ui,
                &palette,
                &crate::ai_agent::markdown::blocks("hello"),
                UserMeta::default(),
            );
        });
        let mut fill = None;
        let mut ink = None;
        for clipped in std::mem::take(&mut output.shapes) {
            match clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == to_color32(palette.accent) => {
                    fill = Some(rect.rect);
                }
                egui::Shape::Text(text) if text.galley.text() == "hello" => {
                    ink = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map(|section| section.format.color);
                }
                _ => {}
            }
        }
        output.drop_without_applying_deltas();

        assert!(fill.is_some(), "the bubble takes the accent, like a primary button");
        assert_eq!(ink, Some(egui::Color32::WHITE), "its text is white");
    }

    /// A frame of the pinned copy: which message pinned, where each painted text landed,
    /// and the bubble's backdrop.
    fn pinned_frame(
        plugin: &mut AiAgent,
        viewport: egui::Rect,
        bubbles: &[(usize, egui::Rect)],
    ) -> (Option<usize>, Vec<(f32, String)>, Option<egui::Rect>) {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(800.0, 400.0),
            )),
            ..Default::default()
        };
        let mut pinned = None;
        let mut output = ctx.run_ui(input, |ui| {
            pinned = plugin
                .pinned_bubble(ui, &palette, viewport, bubbles)
                .map(|pinned| pinned.index);
        });
        let mut texts = Vec::new();
        let mut backdrop = None;
        for clipped in std::mem::take(&mut output.shapes) {
            match clipped.shape {
                egui::Shape::Text(text) => texts.push((text.pos.y, text.galley.text().to_owned())),
                egui::Shape::Rect(rect) if rect.fill == to_color32(palette.bg_panel) => {
                    backdrop = Some(rect.rect);
                }
                _ => {}
            }
        }
        output.drop_without_applying_deltas();
        (pinned, texts, backdrop)
    }

    #[test]
    fn the_newest_message_scrolled_past_the_top_is_pinned_there() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "first question".to_owned(),
            meta: UserMeta::default(),
        });
        plugin.agent.turns.push(Turn::User {
            text: "second question".to_owned(),
            meta: UserMeta::default(),
        });
        let viewport =
            egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(WIDE_PANE, 200.0));
        let first = egui::Rect::from_min_size(egui::pos2(0.0, -200.0), egui::vec2(120.0, 30.0));
        let second = egui::Rect::from_min_size(egui::pos2(0.0, 10.0), egui::vec2(120.0, 30.0));

        // Both are above the viewport: the newest one is the one that pins.
        let (pinned, texts, backdrop) = pinned_frame(&mut plugin, viewport, &[(0, first), (1, second)]);
        assert_eq!(pinned, Some(1));
        let (y, text) = texts.first().expect("the pinned copy is drawn");
        assert_eq!(text, "second question");
        assert!(
            *y >= viewport.top() && *y < viewport.top() + second.height(),
            "the copy sits at the top of the transcript: {y} in {viewport:?}"
        );
        let backdrop = backdrop.expect("the pinned copy has an opaque backdrop");
        assert_eq!(
            backdrop.width(),
            viewport.width(),
            "the backdrop covers the whole transcript width"
        );

        // The newest came back into view: the message before it takes over.
        let visible = egui::Rect::from_min_size(egui::pos2(0.0, 150.0), egui::vec2(120.0, 30.0));
        let inside = egui::Rect::from_min_size(egui::pos2(0.0, 250.0), egui::vec2(120.0, 30.0));
        let (pinned, texts, _) = pinned_frame(&mut plugin, viewport, &[(0, first), (1, visible)]);
        assert_eq!(pinned, Some(0));
        assert_eq!(
            texts.first().map(|(_, text)| text.as_str()),
            Some("first question")
        );

        // Nothing scrolled past the top: nothing is pinned.
        let (pinned, texts, _) = pinned_frame(&mut plugin, viewport, &[(0, visible), (1, inside)]);
        assert_eq!(pinned, None);
        assert!(texts.is_empty(), "nothing is drawn: {texts:?}");
    }

    #[test]
    fn a_long_reply_keeps_the_last_message_pinned_at_the_top() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "first question".to_owned(),
            meta: UserMeta::default(),
        });
        plugin.agent.turns.push(Turn::Assistant {
            text: format!("answer one {}", "filler ".repeat(120)),
            stopped: false,
        });
        plugin.agent.turns.push(Turn::User {
            text: "second question".to_owned(),
            meta: UserMeta::default(),
        });
        plugin.agent.turns.push(Turn::Assistant {
            text: format!("answer two {}", "filler ".repeat(400)),
            stopped: false,
        });
        let ctx = egui::Context::default();
        let mut settings = serde_json::Map::new();
        let mut rows = Vec::new();
        for frame in 0..6 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(WIDE_PANE, 420.0),
                )),
                // Time steps instead of the wall clock, so the scroll settles on a frame
                // of its own instead of racing the test.
                time: Some(frame as f64),
                ..Default::default()
            };
            let mut context = PluginContext {
                palette: Palette::dark(),
                host_label: "dev".to_owned(),
                environment: "a test shell".to_owned(),
                status: SessionStatus::Connected,
                settings: &mut settings,
                scope: "host-1".to_owned(),
                session: None,
                events: &[],
                pending_input: Vec::new(),
                repaint_after: None,
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let mut output = ctx.run_ui(input, |ui| plugin.show(ui, &mut context));
            let shapes = std::mem::take(&mut output.shapes);
            output.drop_without_applying_deltas();
            rows = shapes
                .into_iter()
                .filter_map(|clipped| match clipped.shape {
                    egui::Shape::Text(text) => {
                        Some((text.pos.y, text.galley.text().to_owned()))
                    }
                    _ => None,
                })
                .collect();
        }

        let pinned = rows
            .iter()
            .filter(|(_, text)| text == "second question")
            .map(|(y, _)| *y)
            .fold(f32::INFINITY, f32::min);
        assert!(
            pinned < PINNED_TOP_MAX,
            "the newest message is pinned at the top of the transcript: {rows:?}"
        );
        // The chat is named by its digest, so the first question shows nowhere: its own
        // bubble has scrolled away and the title row no longer repeats it.
        assert_eq!(
            rows.iter()
                .filter(|(_, text)| text == "first question")
                .count(),
            0,
            "the prompt is not repeated as the chat's title: {rows:?}"
        );
    }

    #[test]
    fn a_reply_with_no_words_draws_no_bubble() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::Assistant {
            text: "  \n".to_owned(),
            stopped: false,
        });
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(WIDE_PANE, 200.0),
            )),
            ..Default::default()
        };
        let muted = to_color32(palette.text_muted);
        let mut session: Option<&mut dyn crate::plugin::SessionAccess> = None;
        let mut output = ctx.run_ui(input, |ui| {
            plugin.transcript(ui, &palette, muted, &mut session, TranscriptSource::Main);
        });
        let mut bubbles = Vec::new();
        for clipped in std::mem::take(&mut output.shapes) {
            if let egui::Shape::Rect(rect) = clipped.shape
                && rect.fill == to_color32(palette.bg_panel_2)
            {
                bubbles.push(rect.rect);
            }
        }
        output.drop_without_applying_deltas();
        assert!(bubbles.is_empty(), "an empty reply drew a bubble: {bubbles:?}");
    }

    #[test]
    fn the_running_tool_card_offers_a_stop_button() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let painted = |running: bool| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(WIDE_PANE, 200.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                tool_card(
                    ui,
                    &palette,
                    super::super::protocol::TOOL_RUN_COMMAND,
                    "$ sleep 100",
                    "",
                    ToolCardState {
                        failed: false,
                        running,
                        turn_index: 0,
                        copied: None,
                    },
                );
            });
            let mut texts = Vec::new();
            for clipped in std::mem::take(&mut output.shapes) {
                if let egui::Shape::Text(text) = clipped.shape {
                    texts.push(text.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
            texts
        };
        assert!(
            painted(true).contains(&"Stop".to_owned()),
            "a running command cannot be stopped from its card"
        );
        assert!(
            !painted(false).contains(&"Stop".to_owned()),
            "a finished command still offers Stop"
        );
    }

    #[test]
    fn a_completed_command_card_collapses_to_one_expandable_line() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut output_id = None;
        let draw = |ctx: &egui::Context, output_id: &mut Option<egui::Id>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(WIDE_PANE, 200.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                *output_id = Some(ui.make_persistent_id(("ai-command-output", 4)));
                tool_card(
                    ui,
                    &palette,
                    super::super::protocol::TOOL_RUN_COMMAND,
                    "$ echo ready",
                    "captured output line",
                    ToolCardState {
                        failed: false,
                        running: false,
                        turn_index: 4,
                        copied: None,
                    },
                );
            });
            let mut texts = Vec::new();
            for clipped in std::mem::take(&mut output.shapes) {
                if let egui::Shape::Text(text) = clipped.shape {
                    texts.push((text.galley.text().to_owned(), text.pos));
                }
            }
            output.drop_without_applying_deltas();
            texts
        };
        let collapsed = draw(&ctx, &mut output_id);
        let badge = collapsed
            .iter()
            .find(|(text, _)| text == "SSH")
            .expect("request type");
        let command = collapsed
            .iter()
            .find(|(text, _)| text == "$ echo ready")
            .expect("command label");
        let status = collapsed
            .iter()
            .find(|(text, _)| text == "ok")
            .expect("command status");
        assert!((badge.1.y - command.1.y).abs() <= 2.0, "{collapsed:?}");
        assert!((status.1.y - command.1.y).abs() <= 2.0, "{collapsed:?}");
        assert!(
            !collapsed
                .iter()
                .any(|(text, _)| text == "captured output line"),
            "collapsed output is visible: {collapsed:?}"
        );

        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            &ctx,
            output_id.expect("collapsing header id"),
            false,
        );
        state.set_open(true);
        state.store(&ctx);
        let expanded = draw(&ctx, &mut output_id);
        assert!(
            expanded
                .iter()
                .any(|(text, _)| text == "captured output line"),
            "expanded output is missing: {expanded:?}"
        );
    }

    #[test]
    fn the_terminal_tail_keeps_the_last_characters() {
        assert_eq!(tail_chars("abcdef", 2), "ef");
        assert_eq!(tail_chars("abc", 10), "abc");
        assert_eq!(tail_chars("abc", 0), "");
        // Multi-byte characters are cut on their boundary.
        assert_eq!(tail_chars("hällo", 2), "lo");
    }

    #[test]
    fn a_fetched_model_list_reaches_the_app_settings() {
        let mut plugin = AiAgent {
            providers: vec![ProviderConfig {
                id: "one".to_owned(),
                name: "First".to_owned(),
                protocol: Protocol::Openai,
                base_url: "https://one/v1".to_owned(),
                models: vec!["old".to_owned()],
                embedding_model: None,
            }],
            model: "old".to_owned(),
            ..AiAgent::default()
        };
        let (sender, events) = std::sync::mpsc::channel();
        plugin.fetch = Some((0, events));
        sender
            .send(Ok(vec!["c".to_owned(), "A".to_owned()]))
            .expect("send");

        assert!(plugin.poll_models(), "the list should count as changed");
        assert_eq!(
            plugin.providers[0].models,
            ["A", "c"],
            "sorted, case-insensitive"
        );
        assert_eq!(plugin.model, "old", "the chosen model is left alone");

        let writes = plugin.take_app_settings();
        assert_eq!(writes.len(), 1, "one app-wide write");
        assert_eq!(writes[0].0, PROVIDERS_KEY);
        assert_eq!(writes[0].1[0]["models"], json!(["A", "c"]));
    }

    #[test]
    fn a_failed_model_fetch_keeps_the_configured_list() {
        let mut plugin = AiAgent::default();
        plugin.providers.push(ProviderConfig {
            id: "one".to_owned(),
            name: "First".to_owned(),
            protocol: Protocol::Openai,
            base_url: "https://one/v1".to_owned(),
            models: vec!["kept".to_owned()],
            embedding_model: None,
        });
        let (sender, events) = std::sync::mpsc::channel();
        plugin.fetch = Some((0, events));
        sender.send(Err("no route".to_owned())).expect("send");

        assert!(!plugin.poll_models());
        assert_eq!(plugin.providers[0].models, ["kept"]);
        assert!(plugin.take_app_settings().is_empty());
    }

    #[test]
    fn script_mode_puts_the_tool_catalog_in_the_prompt() {
        let mut plugin = AiAgent::default();
        plugin.agent.script_mode = true;
        let prompt =
            AiAgent::system_prompt(&plugin.agent, &serde_json::Map::new(), "dev", "a test shell");
        assert!(prompt.contains("Call tools from a script."), "{prompt}");
        // The whole point of the mode: many tool calls per script, not one round per call.
        assert!(
            prompt.contains("Run multiple tool calls in one go when it makes sense"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Do the processing and the decisions in the script"),
            "{prompt}"
        );
        // The script is where output is cut down before it costs prompt room.
        assert!(
            prompt.contains("Spend real effort on shrinking what you return"),
            "{prompt}"
        );
        // The limits a script runs under come from the sandbox, not from a copy in the prompt.
        assert!(
            prompt.contains(&format!(
                "{} seconds of JavaScript",
                super::super::script::MAX_EXEC_MS / 1_000
            )),
            "{prompt}"
        );
        assert!(
            prompt.contains(&format!(
                "up to {} tool calls",
                super::super::script::MAX_CALLS
            )),
            "{prompt}"
        );
        assert!(
            prompt.contains(&format!(
                "not answered within {} seconds fails with a timeout",
                super::super::script::MAX_CALL_MS / 1_000
            )),
            "{prompt}"
        );
        assert!(
            prompt.contains("each round re-sends the whole prompt"),
            "{prompt}"
        );
        assert!(
            prompt.contains("- run_command(command: string"),
            "{prompt}"
        );
        // The root's tools are the only ones loaded, so nothing of a scope shows up.
        assert!(!prompt.contains("- dev_grep_search("), "{prompt}");
    }

    #[test]
    fn the_prompt_names_the_loaded_scopes_and_what_can_be_loaded_next() {
        use super::super::permissions::{GROUP_LOCAL_FS, GROUP_REMOTE_FS, Permissions, group_key};
        let prompt_for = |groups: Vec<&str>, load: &[&str]| {
            let settings: serde_json::Map<String, Value> = groups
                .into_iter()
                .map(|group| (group_key(group), json!(true)))
                .collect();
            let mut plugin = AiAgent::default();
            plugin.agent.permissions = Permissions::from_settings(&settings);
            for id in load {
                plugin.agent.scopes.load(super::scopes::find(id).unwrap());
            }
            AiAgent::system_prompt(
                &plugin.agent,
                &serde_json::Map::new(),
                "dev",
                "a test shell",
            )
        };

        // Nothing loaded beyond the root: the prompt offers the next stops and names no
        // tool of a scope that is not loaded.
        let both = prompt_for(vec![GROUP_REMOTE_FS, GROUP_LOCAL_FS], &[]);
        assert!(both.contains("Environment: a test shell."), "{both}");
        assert!(
            both.contains(
                "- core: run_command, run_command_background, run_script, run_sudo, ask_user, \
                 spawn_subagent, wait_subagent, get_current_time"
            ),
            "{both}"
        );
        assert!(both.contains("- host: "), "{both}");
        assert!(both.contains("- local: "), "{both}");
        assert!(!both.contains("remote_fs_read_file"), "{both}");
        assert!(!both.contains("local_fs_grep_search"), "{both}");

        // Loading a scope names its tools and the guidance for them.
        let loaded = prompt_for(vec![GROUP_REMOTE_FS, GROUP_LOCAL_FS], &["host", "host-files"]);
        assert!(
            loaded.contains("- host-files: remote_fs_read_file"),
            "{loaded}"
        );
        assert!(loaded.contains("dev_grep_search"), "{loaded}");
        assert!(loaded.contains("Files on the connected host:"), "{loaded}");
        assert!(
            loaded.contains("sed, awk or shell redirection"),
            "{loaded}"
        );

        // One stop earlier: the file tools are not loaded, so the prompt names the scope
        // that holds them instead of leaving the agent to guess with the shell.
        let one_step = prompt_for(vec![GROUP_REMOTE_FS, GROUP_LOCAL_FS], &["host"]);
        assert!(
            one_step.contains("which the 'host-files' scope holds"),
            "{one_step}"
        );
        assert!(one_step.contains("dev_view_file"), "{one_step}");
        assert!(!one_step.contains("Files on the connected host:"), "{one_step}");

        // A capability the host may not use never shows up, not even as a scope.
        let local_only = prompt_for(vec![GROUP_LOCAL_FS], &[]);
        assert!(local_only.contains("- local: "), "{local_only}");
        assert!(!local_only.contains("- host-files: "), "{local_only}");
        assert!(!local_only.contains("dev_view_file"), "{local_only}");

        // A host with nothing switched on keeps the commands and loses the file scopes.
        let bare = prompt_for(Vec::new(), &[]);
        assert!(bare.contains("- core: run_command"), "{bare}");
        assert!(!bare.contains("- local: "), "{bare}");
        assert!(!bare.contains("local_fs_read_file"), "{bare}");
    }

    #[test]
    fn the_header_names_the_chat_by_its_digest_not_by_its_prompt() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "x".repeat(200),
            meta: UserMeta::default(),
        });
        assert_eq!(
            plugin.title(),
            DEFAULT_CHAT_TITLE,
            "a prompt is not a title"
        );
        plugin.generated_title = Some("Short digest".to_owned());
        assert_eq!(plugin.title(), "Short digest");
    }

    #[test]
    fn the_conversation_is_written_back_for_the_store() {
        let mut settings = serde_json::Map::new();
        settings.insert(
            CONVERSATION_KEY.to_owned(),
            json!({
                "model": "llama3.1",
                "turns": [
                    { "role": "user", "text": "hello" },
                    { "role": "tool", "name": "run_command", "label": "$ uptime",
                      "output": "up 3 days", "failed": false }
                ],
                "history": [
                    { "role": "user", "content": "hello" },
                    { "role": "assistant", "content": "", "toolCalls": [
                        { "id": "c1", "name": "run_command", "arguments": "{\"command\":\"uptime\"}" }
                    ] },
                    { "role": "tool", "toolCallId": "c1", "content": "up 3 days" }
                ]
            }),
        );
        let mut plugin = AiAgent {
            generated_title: Some("Check the host".to_owned()),
            ..AiAgent::default()
        };
        draw(&mut plugin, &mut settings);
        let stored = &settings[CONVERSATION_KEY];
        assert_eq!(stored["version"], DATA_VERSION);
        assert_eq!(stored["title"], "Check the host");
        assert_eq!(stored["turns"].as_array().map(Vec::len), Some(2));
        assert_eq!(stored["history"].as_array().map(Vec::len), Some(3));
        assert_eq!(stored["turns"][1]["label"], "$ uptime");
        // Providers belong to Options, so the pane never writes them back.
        assert!(!settings.contains_key(PROVIDERS_KEY));
        assert!(!settings.contains_key(API_KEYS_KEY));
    }

    #[test]
    fn providers_come_from_the_app_wide_settings_and_keys_from_the_vault() {
        let mut settings = serde_json::Map::new();
        settings.insert(
            PROVIDERS_KEY.to_owned(),
            json!([
                { "id": "one", "name": "First", "protocol": "openai",
                  "baseUrl": "https://one/v1", "models": ["m1"] },
                { "id": "two", "name": "Second", "protocol": "anthropic",
                  "baseUrl": "https://two", "models": ["m2"] }
            ]),
        );
        let mut secrets = std::collections::BTreeMap::new();
        secrets.insert(provider_secret_field("two"), "secret".to_owned());
        let mut plugin = AiAgent::default();
        draw_with_secrets(&mut plugin, &mut settings, &secrets);

        let names: Vec<&str> = plugin
            .providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect();
        assert_eq!(names, ["First", "Second"]);
        assert_eq!(plugin.keys.get("two").map(String::as_str), Some("secret"));
        assert_eq!(plugin.keys.get("one"), None, "no key stored for one");
    }

    #[test]
    fn a_new_chat_files_the_old_one_and_reopens_it() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "first question".to_owned(),
            meta: UserMeta::default(),
        });
        plugin
            .agent
            .history
            .push(super::super::protocol::ApiMessage::User {
                content: "first question".to_owned(),
                images: Vec::new(),
            });
        plugin.generated_title = Some("First digest".to_owned());
        plugin.begin_new_chat();
        assert!(plugin.agent.turns.is_empty(), "the live chat was cleared");
        assert_eq!(plugin.chats.len(), 1);
        assert_eq!(plugin.chats[0].title, "First digest");

        // Opening it back restores the transcript and history.
        plugin.open_chat(0);
        assert_eq!(plugin.agent.turns.len(), 1);
        assert_eq!(plugin.agent.history.len(), 1);
        assert!(plugin.chats.is_empty(), "the empty chat is not kept");
    }

    #[test]
    fn the_back_button_lists_the_chats_and_a_row_opens_one() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "first question".to_owned(),
            meta: UserMeta::default(),
        });
        plugin.generated_title = Some("First digest".to_owned());
        plugin.begin_new_chat();
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        frame(&mut plugin, &mut settings, &ctx, click_back());
        assert_eq!(plugin.view, ChatView::List, "back leaves the chat");

        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            rows.iter().any(|(_, _, text)| text == CHAT_LIST_HEADING),
            "the chat list is not shown: {rows:?}"
        );
        let row = rows
            .iter()
            .find(|(_, _, text)| text == "First digest")
            .cloned()
            .unwrap_or_else(|| panic!("the filed chat is not listed: {rows:?}"));
        frame(&mut plugin, &mut settings, &ctx, click(row));
        assert_eq!(plugin.view, ChatView::Chat, "a row opens its chat");
        assert_eq!(plugin.agent.turns.len(), 1, "the chat came back");
    }

    #[test]
    fn a_prompt_typed_on_the_chat_list_starts_a_new_chat() {
        const CHAT: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
                            data: [DONE]\n\n";
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, _requests) = provider(CHAT, MODELS);
        let mut settings: serde_json::Map<String, Value> = [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": base_url, "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect();
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "earlier question".to_owned(),
            meta: UserMeta::default(),
        });
        plugin.generated_title = Some("Earlier digest".to_owned());
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        frame(&mut plugin, &mut settings, &ctx, click_back());
        assert_eq!(plugin.view, ChatView::List);

        plugin.draft = "hello".to_owned();
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let send = rows
            .iter()
            .find(|(_, _, text)| text == "Send")
            .cloned()
            .expect("the list has a composer");
        frame(&mut plugin, &mut settings, &ctx, click(send));
        assert_eq!(plugin.view, ChatView::Chat, "the new chat is entered");
        assert_eq!(plugin.chats.len(), 1, "the chat left behind is filed");
        assert_eq!(plugin.chats[0].title, "Earlier digest");
        assert!(
            plugin
                .agent
                .turns
                .iter()
                .any(|turn| matches!(turn, Turn::User { text, .. } if text == "hello")),
            "the prompt starts the new chat: {:?}",
            plugin.agent.turns
        );
    }

    #[test]
    fn the_header_starts_a_new_chat_from_the_chat_and_from_the_list() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "first question".to_owned(),
            meta: UserMeta::default(),
        });
        plugin.generated_title = Some("First digest".to_owned());
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();

        // The open chat files itself and starts empty.
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let button = rows
            .iter()
            .find(|(_, _, text)| text == NEW_CHAT_LABEL)
            .cloned()
            .unwrap_or_else(|| panic!("the chat header has no new chat button: {rows:?}"));
        assert!(
            button.1 > (WIDE_PANE / 2.0) as i32,
            "the button belongs at the right edge of the header, not at {}",
            button.1
        );
        frame(&mut plugin, &mut settings, &ctx, click(button));
        assert_eq!(plugin.view, ChatView::Chat, "the new chat is entered");
        assert_eq!(plugin.chats.len(), 1, "the chat left behind is filed");
        assert_eq!(plugin.chats[0].title, "First digest");
        assert!(plugin.agent.turns.is_empty(), "the new chat starts empty");

        // The list offers the same button, and it enters a fresh chat.
        frame(&mut plugin, &mut settings, &ctx, click_back());
        assert_eq!(plugin.view, ChatView::List);
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let button = rows
            .iter()
            .find(|(_, _, text)| text == NEW_CHAT_LABEL)
            .cloned()
            .unwrap_or_else(|| panic!("the chat list has no new chat button: {rows:?}"));
        frame(&mut plugin, &mut settings, &ctx, click(button));
        assert_eq!(plugin.view, ChatView::Chat, "the list's button enters a chat");
        assert_eq!(plugin.chats.len(), 1, "an empty conversation is not filed");
    }

    #[test]
    fn a_filed_chat_is_titled_by_the_model() {
        const TITLE: &str = "{\"choices\":[{\"message\":{\"content\":\"Check the pump\"}}]}";
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, _requests) = answering("application/json", TITLE, MODELS);
        let mut settings: serde_json::Map<String, Value> = [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": base_url, "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect();
        let mut plugin = AiAgent::default();
        plugin.agent.now_ms = 7;
        plugin.agent.turns.push(Turn::User {
            text: "what is the pump doing".to_owned(),
            meta: UserMeta::default(),
        });
        plugin.title_prompt = Some("what is the pump doing".to_owned());
        // Filed while the digest is still to be written: the work goes with the chat.
        plugin.begin_new_chat();
        assert_eq!(plugin.chats[0].title, DEFAULT_CHAT_TITLE);

        let ctx = egui::Context::default();
        for _ in 0..50 {
            frame(&mut plugin, &mut settings, &ctx, Vec::new());
            if plugin.chats[0].title != DEFAULT_CHAT_TITLE {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            plugin.chats[0].title, "Check the pump",
            "the model names the filed chat: error {:?}",
            plugin.error
        );
    }

    #[test]
    fn a_shallow_map_starts_a_mapping_pass_with_its_budget() {
        const CHAT: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
                            data: [DONE]\n\n";
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, _requests) = provider(CHAT, MODELS);
        let mut settings: serde_json::Map<String, Value> = [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": base_url, "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect();
        let mut plugin = AiAgent::default();
        let ctx = egui::Context::default();
        // The first frame reads what was stored, so the map is set after it.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        plugin.agent.map = super::super::map::EnvMap::parse(
            "/srv/app",
            1,
            "10 5 /srv/app/main.rs\n__wassh_symbols__\n",
        );
        plugin.map_deep_wanted = true;
        frame(&mut plugin, &mut settings, &ctx, Vec::new());

        let run = plugin
            .map_deep
            .as_ref()
            .unwrap_or_else(|| panic!("the mapping pass did not start: {:?}", plugin.error));
        let prompt = &plugin.agent.subagents[run.subagent].prompt;
        assert!(prompt.contains("at most 100 of them"), "{prompt}");
        assert!(prompt.contains("/srv/app"), "{prompt}");
    }

    #[test]
    fn a_mapping_answer_describes_the_files_in_the_map() {
        const ANSWER: &str = "```json\n{\"files\": [\
            {\"path\": \"main.rs\", \"summary\": \"Entry point.\", \"connects\": [\"lib.rs\"]}]}\n```";
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        // The first frame reads what was stored, so the map is set after it.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        plugin.agent.map = super::super::map::EnvMap::parse(
            "/srv/app",
            1,
            "10 5 /srv/app/main.rs\n__wassh_symbols__\n",
        );
        let index = plugin.agent.begin_subagent("map the project", "map it");
        plugin.agent.subagents[index]
            .agent
            .turns
            .push(Turn::Assistant {
                text: ANSWER.to_owned(),
                stopped: false,
            });
        plugin.agent.subagents[index].end = Some(super::super::agent::SubagentEnd::Done);
        plugin.map_deep = Some(MapRun {
            subagent: index,
            root: "/srv/app".to_owned(),
            started: 0,
            attempts: MAP_ATTEMPTS,
        });

        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let node = &plugin.agent.map.nodes["/srv/app/main.rs"];
        assert_eq!(node.summary, "Entry point.");
        assert_eq!(node.links, ["lib.rs"]);
        assert!(
            plugin.agent.turns.iter().any(
                |turn| matches!(turn, Turn::Notice(text) if text.contains("Map ready"))
            ),
            "the pane says the map is ready: {:?}",
            plugin.agent.turns
        );
        assert!(plugin.map_deep.is_none(), "the pass is done");
    }

    #[test]
    fn the_graph_popup_says_what_the_mapping_agent_is_doing() {
        let mut settings = serde_json::Map::new();
        settings.insert(MAP_KEY.to_owned(), a_scanned_map().to_json());
        let mut plugin = AiAgent::default();
        plugin
            .agent
            .scopes
            .load(super::super::scopes::find("host").unwrap());
        plugin
            .agent
            .scopes
            .load(super::super::scopes::find("env").unwrap());
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let index = plugin.agent.begin_subagent("map the project", "map it");
        {
            let sub = &mut plugin.agent.subagents[index];
            sub.agent.turns.push(Turn::Tool {
                name: "run_command".to_owned(),
                label: "read /srv/app/main.rs".to_owned(),
                output: String::new(),
                failed: false,
            });
            sub.agent.phase = Some(Phase::Running {
                call: crate::ai_agent::ApiToolCall {
                    id: "call-1".to_owned(),
                    name: "run_command".to_owned(),
                    arguments: "{}".to_owned(),
                },
                turn: sub.agent.turns.len() - 1,
                waiting: crate::ai_agent::Waiting::List {
                    path: "/srv/app".to_owned(),
                },
            });
        }
        plugin.map_deep = Some(MapRun {
            subagent: index,
            root: "/srv/app".to_owned(),
            started: plugin.agent.now_ms,
            attempts: MAP_ATTEMPTS,
        });
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let tag = rows
            .iter()
            .find(|(_, _, text)| text == GRAPH_SCOPE_LABEL)
            .cloned()
            .expect("the Graph scope has a tag");
        frame(&mut plugin, &mut settings, &ctx, click(tag));
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            rows.iter()
                .any(|(_, _, text)| text.contains("main.rs") && text.contains("Describing")),
            "the popup says what the mapping agent is doing: {rows:?}"
        );
        let more = rows
            .iter()
            .find(|(_, _, text)| text == "More")
            .cloned()
            .expect("a running pass offers more about itself");
        frame(&mut plugin, &mut settings, &ctx, click(more));
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            rows.iter()
                .any(|(_, _, text)| text.starts_with("1 call in")),
            "the popup counts the calls the pass made: {rows:?}"
        );
        assert!(
            rows.iter().any(|(_, _, text)| text == "read /srv/app/main.rs"),
            "the popup lists the calls themselves: {rows:?}"
        );
        assert!(
            rows.iter().any(|(_, _, text)| text == "Less"),
            "the detail view can be closed again: {rows:?}"
        );
    }

    #[test]
    fn a_mapping_pass_says_how_long_it_has_been_running() {
        assert_eq!(elapsed_text(0), "0 s");
        assert_eq!(elapsed_text(9_500), "9 s");
        assert_eq!(elapsed_text(90_000), "1 min");
    }

    #[test]
    fn the_graph_popup_scans_the_root_the_host_finds() {
        let mut settings = serde_json::Map::new();
        settings.insert(MAP_KEY.to_owned(), a_scanned_map().to_json());
        let mut plugin = AiAgent::default();
        plugin
            .agent
            .scopes
            .load(super::super::scopes::find("host").unwrap());
        plugin
            .agent
            .scopes
            .load(super::super::scopes::find("env").unwrap());
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let tag = rows
            .iter()
            .find(|(_, _, text)| text == GRAPH_SCOPE_LABEL)
            .cloned()
            .expect("the Graph scope has a tag");
        frame(&mut plugin, &mut settings, &ctx, click(tag));
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let scan = rows
            .iter()
            .find(|(_, _, text)| text == "Scan")
            .cloned()
            .unwrap_or_else(|| panic!("the map popup offers Scan: {rows:?}"));
        // Scanning needs no folder: the host finds the project root itself, and a pane
        // without a session says so instead of asking for one.
        frame(&mut plugin, &mut settings, &ctx, click(scan));
        // The click arms the rebuild; the frame after it runs the scan.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert_eq!(
            plugin.error.as_deref(),
            Some("The session is not connected.")
        );
        assert!(
            !settings.contains_key("envMapFolder"),
            "the pane keeps no folder of its own"
        );
    }

    #[test]
    fn a_mapping_answer_without_files_says_what_it_said() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        plugin.agent.map = super::super::map::EnvMap::parse(
            "/srv/app",
            1,
            "10 5 /srv/app/main.rs\n__wassh_symbols__\n",
        );
        let index = plugin.agent.begin_subagent("map the project", "map it");
        plugin.agent.subagents[index]
            .agent
            .turns
            .push(Turn::Assistant {
                text: "I had a look around.".to_owned(),
                stopped: false,
            });
        plugin.agent.subagents[index].end = Some(super::super::agent::SubagentEnd::Done);
        plugin.map_deep = Some(MapRun {
            subagent: index,
            root: "/srv/app".to_owned(),
            started: 0,
            attempts: MAP_ATTEMPTS,
        });
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            plugin.agent.turns.iter().any(|turn| matches!(
                turn,
                Turn::Notice(text)
                    if text.contains("described no files") && text.contains("I had a look around")
            )),
            "the pane says the pass described nothing, and what it said: {:?}",
            plugin.agent.turns
        );
        assert!(plugin.map_deep.is_none(), "the pass is over");
    }

    #[test]
    fn a_mapping_answer_without_files_is_asked_for_again() {
        const CHAT: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
                            data: [DONE]\n\n";
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, _requests) = provider(CHAT, MODELS);
        let mut settings: serde_json::Map<String, Value> = [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": base_url, "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect();
        let mut plugin = AiAgent::default();
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        plugin.agent.map = super::super::map::EnvMap::parse(
            "/srv/app",
            1,
            "10 5 /srv/app/main.rs\n__wassh_symbols__\n",
        );
        let index = plugin.agent.begin_subagent("map the project", "map it");
        plugin.agent.subagents[index]
            .agent
            .turns
            .push(Turn::Assistant {
                text: "I had a look around.".to_owned(),
                stopped: false,
            });
        plugin.agent.subagents[index].end = Some(super::super::agent::SubagentEnd::Done);
        plugin.map_deep = Some(MapRun {
            subagent: index,
            root: "/srv/app".to_owned(),
            started: 0,
            attempts: 1,
        });
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let run = plugin
            .map_deep
            .as_ref()
            .unwrap_or_else(|| panic!("the pass gave up: {:?}", plugin.agent.turns));
        assert_eq!(run.attempts, 2, "the answer is asked for once more");
        assert!(
            plugin.agent.subagents[index].agent.phase.is_some(),
            "the model is asked again"
        );
        assert!(
            plugin
                .agent
                .subagents[index]
                .agent
                .turns
                .iter()
                .any(|turn| matches!(turn, Turn::User { text, .. } if text.contains("JSON object"))),
            "the sub-agent is told what was missing"
        );
    }

    #[test]
    fn the_oldest_chats_are_dropped_at_the_cap() {
        let mut plugin = AiAgent::default();
        for index in 0..(CONVERSATIONS_PER_HOST_MAX + 3) {
            plugin.agent.turns.push(Turn::User {
                text: format!("question {index}"),
                meta: UserMeta::default(),
            });
            // The chat that follows starts a moment later, so creation order is total.
            plugin.agent.now_ms = index as u64 + 1;
            plugin.begin_new_chat();
        }
        assert_eq!(plugin.chats.len(), CONVERSATIONS_PER_HOST_MAX);
        // The three oldest were dropped; the rest keep the order they were started in.
        assert_eq!(plugin.chats[0].created, 3);
        assert_eq!(
            plugin.chats.last().map(|chat| chat.created),
            Some((CONVERSATIONS_PER_HOST_MAX + 2) as u64)
        );
    }

    #[test]
    fn the_list_keeps_creation_order_when_a_chat_is_opened_again() {
        let mut plugin = AiAgent::default();
        for index in 0..3u64 {
            plugin.agent.turns.push(Turn::User {
                text: format!("question {index}"),
                meta: UserMeta::default(),
            });
            plugin.generated_title = Some(format!("digest {index}"));
            plugin.agent.now_ms = index + 1;
            plugin.begin_new_chat();
        }
        assert_eq!(plugin.chats.len(), 3);
        assert_eq!(plugin.chats[2].title, "digest 2");

        // Reopening the oldest and filing it again leaves it where it was created.
        plugin.open_chat(0);
        plugin.begin_new_chat();
        let titles: Vec<&str> = chat_order(&plugin.chats)
            .iter()
            .map(|&index| plugin.chats[index].title.as_str())
            .collect();
        assert_eq!(
            titles,
            ["digest 2", "digest 1", "digest 0"],
            "newest on top, and reopening a chat does not move it"
        );
    }

    #[test]
    fn a_title_taken_from_the_prompt_is_asked_for_again() {
        let mut settings = serde_json::Map::new();
        settings.insert(
            CONVERSATION_KEY.to_owned(),
            json!({
                "version": DATA_VERSION,
                "title": "New chat",
                "chats": [{
                    "title": "what is on the line",
                    "turns": [{ "role": "user", "text": "what is on the line" }],
                    "history": [{ "role": "user", "content": "what is on the line" }]
                }]
            }),
        );
        let mut plugin = AiAgent::default();
        draw(&mut plugin, &mut settings);
        assert_eq!(plugin.chats[0].title, DEFAULT_CHAT_TITLE);
        assert_eq!(
            plugin.chats[0].title_prompt.as_deref(),
            Some("what is on the line"),
            "the prompt is queued for a digest"
        );
    }

    /// A map with two files, as a scan would leave it.
    fn a_scanned_map() -> super::super::map::EnvMap {
        super::super::map::EnvMap::parse(
            "/srv/app",
            1_700_000_000_000,
            "1200 1700000000 /srv/app/src/main.rs\n\
             430 1700000001 /srv/app/src/lib.rs\n\
             __wassh_symbols__\n\
             /srv/app/src/lib.rs:4:pub fn run() {\n",
        )
    }

    #[test]
    fn the_prompt_carries_the_map_once_its_scope_is_loaded() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        plugin.agent.map = a_scanned_map();
        plugin
            .agent
            .scopes
            .load(super::super::scopes::find("host").unwrap());
        let without = AiAgent::system_prompt(&plugin.agent, &settings, "dev", "a test shell");
        assert!(
            !without.contains(MAP_HEADING),
            "the map is not the conversation's until its scope is loaded: {without}"
        );

        plugin
            .agent
            .scopes
            .load(super::super::scopes::find("env").unwrap());
        let with = AiAgent::system_prompt(&plugin.agent, &settings, "dev", "a test shell");
        assert!(with.contains(MAP_HEADING), "{with}");
        assert!(with.contains("/srv/app (2 files"), "{with}");
        assert!(with.contains("src/  2"), "{with}");
        assert!(with.contains("env_search ranks files"), "{with}");

        // An empty map says nothing rather than costing prompt room.
        plugin.agent.map.clear();
        let empty = AiAgent::system_prompt(&plugin.agent, &settings, "dev", "a test shell");
        assert!(!empty.contains(MAP_HEADING), "{empty}");
        settings.clear();
    }

    #[test]
    fn the_prompt_carries_the_host_setup_once_it_has_been_scanned() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        plugin
            .agent
            .scopes
            .load(super::super::scopes::find("host").unwrap());
        plugin
            .agent
            .scopes
            .load(super::super::scopes::find("env").unwrap());
        let without = AiAgent::system_prompt(&plugin.agent, &settings, "dev", "a test shell");
        assert!(
            !without.contains(SYSTEM_HEADING),
            "nothing scanned, nothing claimed: {without}"
        );

        plugin.agent.system = super::super::map::SystemFacts::parse(
            1,
            "__wassh_os__\nLinux 6.1\n__wassh_ports__\n:8080 nginx\n__wassh_units__\nnginx.service\n",
        );
        let with = AiAgent::system_prompt(&plugin.agent, &settings, "dev", "a test shell");
        assert!(with.contains(SYSTEM_HEADING), "{with}");
        assert!(with.contains(":8080 nginx"), "{with}");
        assert!(with.contains("nginx.service"), "{with}");
        settings.clear();
    }

    #[test]
    fn the_map_is_host_state_and_survives_a_reload() {
        let mut settings = serde_json::Map::new();
        settings.insert(MAP_KEY.to_owned(), a_scanned_map().to_json());
        let mut plugin = AiAgent::default();
        draw(&mut plugin, &mut settings);
        assert_eq!(plugin.agent.map.len(), 2, "the host's map came back");
        assert_eq!(plugin.agent.map.root, "/srv/app");

        // Forgetting a path is written back on the next frame, once.
        plugin.agent.map.forget("/srv/app/src/main.rs");
        draw(&mut plugin, &mut settings);
        assert_eq!(
            settings[MAP_KEY]["nodes"].as_array().map(Vec::len),
            Some(1),
            "the shorter map was stored"
        );
    }

    #[test]
    fn the_map_scope_popup_clears_the_map() {
        let mut settings = serde_json::Map::new();
        settings.insert(MAP_KEY.to_owned(), a_scanned_map().to_json());
        let mut plugin = AiAgent::default();
        plugin.agent.scopes.load(super::super::scopes::find("host").unwrap());
        plugin.agent.scopes.load(super::super::scopes::find("env").unwrap());
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let tag = rows
            .iter()
            .find(|(_, _, text)| text == GRAPH_SCOPE_LABEL)
            .cloned()
            .expect("the Graph scope has a tag");
        assert!(
            rows.iter().any(|(_, _, text)| text == "host"),
            "the host scope stays visible alongside Graph: {rows:?}"
        );
        frame(&mut plugin, &mut settings, &ctx, click(tag));
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            rows.iter().any(|(_, _, text)| text.contains("files under /srv/app")),
            "the popup does not describe the map: {rows:?}"
        );
        let clear = rows
            .iter()
            .find(|(_, _, text)| text == "Clear")
            .cloned()
            .expect("the map popup offers Clear");
        frame(&mut plugin, &mut settings, &ctx, click(clear));
        assert!(plugin.agent.map.is_empty(), "Clear did not empty the map");
        assert!(
            plugin.agent.scopes.contains(ENV_SCOPE),
            "clearing is not unloading"
        );
    }

    #[test]
    fn stored_chats_survive_a_reload() {
        let mut settings = serde_json::Map::new();
        settings.insert(
            CONVERSATION_KEY.to_owned(),
            json!({
                "model": "llama3.1",
                "turns": [],
                "history": [],
                "chats": [
                    {
                        "title": "old chat",
                        "cwd": "/srv",
                        "model": "llama3.1",
                        "provider": "ollama",
                        "turns": [{ "role": "user", "text": "old chat" }],
                        "history": [{ "role": "user", "content": "old chat" }]
                    }
                ]
            }),
        );
        let mut plugin = AiAgent::default();
        draw(&mut plugin, &mut settings);
        assert_eq!(plugin.chats.len(), 1);
        assert_eq!(plugin.chats[0].title, "New chat", "a prompt is not a title");
        assert_eq!(plugin.chats[0].cwd, "/srv");
        // The picker is painted.
        let painted = draw(&mut plugin, &mut settings);
        assert!(painted.iter().any(|text| text == "Chats"), "picker missing");
        // And it is written back.
        assert_eq!(
            settings[CONVERSATION_KEY]["chats"].as_array().map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn loaded_scopes_are_written_back_and_read_again() {
        let mut settings = serde_json::Map::new();
        settings.insert(
            CONVERSATION_KEY.to_owned(),
            json!({
                "model": "llama3.1",
                "turns": [],
                "history": [],
                "scopes": [scopes::ROOT, "web", "not-a-scope"]
            }),
        );
        let mut plugin = AiAgent::default();
        draw(&mut plugin, &mut settings);
        assert!(plugin.agent.scopes.contains("web"));
        assert!(!plugin.agent.scopes.contains("not-a-scope"));
        assert!(plugin.agent.scopes.contains(scopes::ROOT));
        // Written back in graph order, with the unknown id dropped.
        assert_eq!(settings[CONVERSATION_KEY]["scopes"], json!([scopes::ROOT, "web"]));
    }

    #[test]
    fn a_scope_tag_opens_its_popup_and_turning_off_drops_it() {
        let mut plugin = AiAgent::default();
        plugin.agent.scopes.load(super::scopes::find("host").unwrap());
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        // The composer panel settles its height on the first frame.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            rows.iter().any(|(_, _, text)| text == SCOPE_LABEL),
            "{rows:?}"
        );
        let tag = rows
            .iter()
            .find(|(_, _, text)| text == "host")
            .cloned()
            .unwrap_or_else(|| panic!("no tag for the loaded scope: {rows:?}"));
        frame(&mut plugin, &mut settings, &ctx, click(tag));
        assert!(
            plugin.agent.scopes.contains("host"),
            "clicking a tag asks, it does not unload"
        );

        // The popup says what the scope is and offers to turn it off.
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let scope = super::scopes::find("host").unwrap();
        assert!(
            rows.iter().any(|(_, _, text)| text == scope.summary),
            "the popup does not describe the scope: {rows:?}"
        );
        let turn_off = rows
            .iter()
            .find(|(_, _, text)| text == "Turn off")
            .cloned()
            .unwrap_or_else(|| panic!("the popup offers no Turn off: {rows:?}"));
        frame(&mut plugin, &mut settings, &ctx, click(turn_off));
        assert!(!plugin.agent.scopes.contains("host"), "Turn off did nothing");
        assert!(plugin.agent.scopes.contains(scopes::ROOT));
        assert!(plugin.scope_popup.is_none(), "the popup closed with the action");
    }

    #[test]
    fn the_include_buttons_stack_beside_the_prompt() {
        // Enough lines that the prompt box is taller than the button column: the column
        // must stay pinned to the box's top edge instead of being centred against it.
        let mut plugin = AiAgent {
            draft: "one\ntwo\nthree\nfour".to_owned(),
            ..AiAgent::default()
        };
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        // The composer panel settles its height on the first frame.
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let find = |want: &str| {
            rows.iter()
                .find(|(_, _, text)| text == want)
                .cloned()
                .unwrap_or_else(|| panic!("{want} is missing: {rows:?}"))
        };
        let attach = find(ATTACH_GLYPH);
        let terminal = find(TERMINAL_ATTACH_GLYPH);
        assert_eq!(
            attach.1, terminal.1,
            "the two include tools share one column: {attach:?} vs {terminal:?}"
        );
        assert!(
            terminal.0 > attach.0,
            "the terminal toggle is the lower one: {attach:?} vs {terminal:?}"
        );
        // The top button starts level with the box's first line of text.
        let prompt = find("one\ntwo\nthree\nfour");
        assert!(
            (attach.0 - prompt.0).abs() < 12,
            "the column is level with the prompt top: {attach:?} vs {prompt:?}"
        );
        assert!(
            (attach.1 - prompt.1).abs() < 60,
            "the column sits beside the prompt: {attach:?} vs {prompt:?}"
        );
    }

    #[test]
    fn a_narrow_pane_crops_the_dropdowns_instead_of_the_send_button() {
        let mut settings: serde_json::Map<String, Value> = [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": "http://127.0.0.1:1", "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect();
        let mut plugin = AiAgent {
            draft: "hi".to_owned(),
            ..AiAgent::default()
        };
        let ctx = egui::Context::default();
        // The composer panel settles its height on the first frame.
        frame_at(&mut plugin, &mut settings, &ctx, Vec::new(), NARROW_PANE);
        let rows = frame_at(&mut plugin, &mut settings, &ctx, Vec::new(), NARROW_PANE);

        let send = rows
            .iter()
            .find(|(_, _, text)| text == "Send")
            .cloned()
            .expect("the narrow row still offers Send");
        assert!(
            (send.1 as f32) < NARROW_PANE,
            "Send was pushed out of the pane: {send:?}"
        );
        let provider = rows
            .iter()
            .find(|(_, _, text)| text == "Local")
            .cloned()
            .expect("the provider dropdown is still there");
        assert!(
            provider.1 < send.1,
            "Send stays to the right of the dropdowns: {provider:?} vs {send:?}"
        );
    }

    #[test]
    fn the_sudo_card_names_the_command_and_takes_a_password() {
        let mut plugin = AiAgent::default();
        plugin.agent.phase = Some(Phase::Sudo {
            call: super::super::protocol::ApiToolCall {
                id: "c1".to_owned(),
                name: super::super::protocol::TOOL_RUN_SUDO.to_owned(),
                arguments: r#"{"command":"apt-get update"}"#.to_owned(),
            },
            command: "apt-get update".to_owned(),
            reason: Some("refresh the package lists".to_owned()),
        });
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(
            texts.contains(&"Sudo password required"),
            "the card is missing: {texts:?}"
        );
        assert!(
            texts.contains(&"apt-get update"),
            "the command is not shown: {texts:?}"
        );
        assert!(
            texts.contains(&"refresh the package lists"),
            "the reason is not shown: {texts:?}"
        );
        assert!(texts.contains(&"Continue"), "no way to go on: {texts:?}");
        assert!(texts.contains(&"Cancel"), "no way out: {texts:?}");
    }

    #[test]
    fn the_web_and_knowledge_settings_reach_the_tools() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        settings.insert(SETTING_WEB_SEARCH_PROVIDER.to_owned(), json!("duckduckgo"));
        // A key left in the settings map is ignored: the vault is the only
        // place a secret is read from.
        settings.insert(SETTING_WEB_SEARCH_KEY.to_owned(), json!("plain-text"));
        settings.insert(SETTING_RAG_FOLDER.to_owned(), json!("/docs"));
        settings.insert(SETTING_HOST_RAG_FOLDER.to_owned(), json!("/srv/docs"));
        let secrets = [(SETTING_WEB_SEARCH_KEY.to_owned(), "secret".to_owned())]
            .into_iter()
            .collect();
        draw_with_secrets(&mut plugin, &mut settings, &secrets);
        assert_eq!(
            plugin.agent.tools.web_provider.as_deref(),
            Some("duckduckgo")
        );
        assert_eq!(plugin.agent.tools.web_key.as_deref(), Some("secret"));
        assert_eq!(plugin.agent.tools.rag_folders, ["/docs", "/srv/docs"]);
        assert!(
            plugin.agent.tools.embed.is_none(),
            "no embedding model configured"
        );
    }

    #[test]
    fn a_message_typed_during_a_run_joins_the_conversation_instead_of_queueing() {
        let mut settings = local_provider();
        let (mut plugin, _worker) = busy_pane();
        let ctx = egui::Context::default();
        // A typed message turns the footer's Stop into Send.
        plugin.draft = "and check /etc".to_owned();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            !rows.iter().any(|(_, _, text)| text == "Stop"),
            "Stop has no room beside a message: {rows:?}"
        );
        let send = rows
            .iter()
            .find(|(_, _, text)| text == "Send")
            .cloned()
            .unwrap_or_else(|| panic!("the composer has no Send button: {rows:?}"));

        frame(&mut plugin, &mut settings, &ctx, click(send));

        // The message joined the running conversation: it is a turn, and the provider
        // history waits for the reply that is still coming.
        assert!(
            matches!(
                plugin.agent.turns.last(),
                Some(Turn::User { text, .. }) if text == "and check /etc"
            ),
            "the message was dropped: {:?}",
            plugin.agent.turns
        );
        assert!(plugin.queued.is_none(), "Enter and Send inject, they queue");
        assert_eq!(
            plugin.agent.history.len(),
            1,
            "history: {:?}",
            plugin.agent.history
        );
        assert!(plugin.draft.is_empty(), "the composer is ready for the next");
    }

    #[test]
    fn alt_enter_holds_the_message_back_until_the_run_is_over() {
        let mut settings = local_provider();
        let (mut plugin, _worker) = busy_pane();
        let ctx = egui::Context::default();
        // The composer settles its height on the first frame; clicking the empty prompt
        // is what focuses the text area for the key press.
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let hint = rows
            .iter()
            .find(|(_, _, text)| text == "Ask the agent…")
            .cloned()
            .unwrap_or_else(|| panic!("the prompt has no hint: {rows:?}"));
        frame(&mut plugin, &mut settings, &ctx, click(hint));
        plugin.draft = "second opinion".to_owned();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());

        let alt = egui::Modifiers {
            alt: true,
            ..Default::default()
        };
        frame(
            &mut plugin,
            &mut settings,
            &ctx,
            vec![
                egui::Event::ModifiersChanged(alt),
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: alt,
                },
            ],
        );

        let queued = plugin
            .queued
            .as_ref()
            .unwrap_or_else(|| panic!("Alt+Enter queues the message: {:?}", plugin.agent.turns));
        assert_eq!(queued.text, "second opinion");
        assert!(
            plugin.agent.turns.iter().all(
                |turn| !matches!(turn, Turn::User { text, .. } if text.contains("second opinion"))
            ),
            "a queued message does not join the conversation yet: {:?}",
            plugin.agent.turns
        );
        assert!(
            frame(&mut plugin, &mut settings, &ctx, Vec::new())
                .iter()
                .any(|(_, _, text)| text == "Queued"),
            "nothing shows what waits its turn"
        );
    }

    /// The settings of a pane with one OpenAI-compatible provider that never answers.
    fn local_provider() -> serde_json::Map<String, Value> {
        const MODELS: &str = "{\"data\":[{\"id\":\"test-model\"}]}";
        let (base_url, _requests) = provider("", MODELS);
        [(
            PROVIDERS_KEY.to_owned(),
            json!([{ "id": "local", "name": "Local", "protocol": "openai",
                     "baseUrl": base_url, "models": ["test-model"] }]),
        )]
        .into_iter()
        .collect()
    }

    /// A pane whose model is mid-answer; the returned sender keeps the run going.
    fn busy_pane() -> (AiAgent, std::sync::mpsc::Sender<crate::ai_agent::StreamEvent>) {
        let mut plugin = AiAgent::default();
        assert!(plugin.agent.push_user("first", UserMeta::default()));
        plugin.agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        let (worker, events) = std::sync::mpsc::channel();
        plugin.agent.phase = Some(Phase::Streaming { turn: 1, events });
        (plugin, worker)
    }

    /// A click in the middle of the pane's back button, which is drawn rather than typed.
    fn click_back() -> Vec<egui::Event> {
        click_events(egui::Pos2::new(
            BACK_BUTTON_SIZE / 2.0,
            BACK_BUTTON_SIZE / 2.0,
        ))
    }

    /// A click in the middle of a painted text.
    fn click(row: (i32, i32, String)) -> Vec<egui::Event> {
        let pos = egui::Pos2::new(row.1 as f32 + 4.0, row.0 as f32 + 4.0);
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ]
    }

    #[test]
    fn the_allow_rules_default_to_the_shipped_safe_list() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        draw(&mut plugin, &mut settings);
        assert_eq!(plugin.agent.app_allow.len(), SAFE_RULES.len());
        assert!(plugin.agent.app_allow.contains(&"ls*".to_owned()));
        // A configured list replaces the defaults.
        settings.insert(
            SETTING_DEFAULT_ALLOW_RULES.to_owned(),
            json!(["uptime", "date"]),
        );
        let mut plugin = AiAgent::default();
        draw(&mut plugin, &mut settings);
        assert_eq!(plugin.agent.app_allow, ["uptime", "date"]);
    }

    #[test]
    fn the_pane_turns_a_spawn_into_a_running_subagent() {
        let mut plugin = AiAgent::default();
        plugin.providers.push(ProviderConfig {
            id: "one".to_owned(),
            name: "First".to_owned(),
            protocol: Protocol::Openai,
            base_url: "https://one/v1".to_owned(),
            models: vec!["m".to_owned()],
            embedding_model: None,
        });
        plugin.model = "m".to_owned();
        plugin.agent.turns.push(Turn::User {
            text: "delegate the log check".to_owned(),
            meta: UserMeta::default(),
        });
        let call = super::super::protocol::ApiToolCall {
            id: "c1".to_owned(),
            name: super::super::protocol::TOOL_SPAWN_SUBAGENT.to_owned(),
            arguments: r#"{"title":"check the logs","prompt":"Find the failing unit."}"#.to_owned(),
        };
        // The model asks for a spawn in its reply.
        plugin.agent.turns.push(Turn::Assistant {
            text: String::new(),
            stopped: false,
        });
        let (sender, events) = std::sync::mpsc::channel();
        plugin.agent.phase = Some(Phase::Streaming { turn: 1, events });
        sender
            .send(crate::ai_agent::StreamEvent::Done(
                super::super::protocol::Completion {
                    text: String::new(),
                    tool_calls: vec![call],
                    ..super::super::protocol::Completion::default()
                },
            ))
            .expect("send");
        let mut session: Option<&mut dyn crate::plugin::SessionAccess> = None;
        plugin.agent.poll(&mut session);
        assert!(plugin.agent.spawning(), "the pane has to build the request");

        let settings = serde_json::Map::new();
        plugin.launch_spawn(&mut session, &settings, "dev", "a test shell");

        assert_eq!(plugin.agent.subagents.len(), 1);
        let sub = &plugin.agent.subagents[0];
        assert_eq!(sub.id, "sub-1");
        assert!(sub.running());
        assert!(
            matches!(sub.agent.phase, Some(Phase::Streaming { .. })),
            "the sub-agent is answering on its own"
        );
        assert!(!plugin.agent.spawning());
        let answered = plugin
            .agent
            .turns
            .iter()
            .find_map(|turn| match turn {
                Turn::Tool { output, failed, .. } => Some((output.clone(), *failed)),
                _ => None,
            })
            .expect("the spawn call is answered");
        assert!(!answered.1, "{}", answered.0);
        assert!(answered.0.contains("sub-1"), "{}", answered.0);
        assert!(plugin.agent.needs_reply, "the main run goes on");
    }

    /// A pane whose only chat is filed, with one finished sub-agent under it.
    fn filed_chat_with_a_subagent() -> AiAgent {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "delegate the log check".to_owned(),
            meta: UserMeta::default(),
        });
        let index = plugin
            .agent
            .begin_subagent("check the logs", "Find the failing unit.");
        plugin.agent.subagents[index]
            .agent
            .turns
            .push(Turn::Assistant {
                text: "api.service is failing".to_owned(),
                stopped: false,
            });
        plugin.agent.subagents[index].end = Some(SubagentEnd::Done);
        plugin.begin_new_chat();
        plugin
    }

    #[test]
    fn a_filed_chats_subagents_open_from_its_note() {
        let mut plugin = filed_chat_with_a_subagent();
        let mut settings = serde_json::Map::new();
        let ctx = egui::Context::default();
        frame(&mut plugin, &mut settings, &ctx, Vec::new());
        frame(&mut plugin, &mut settings, &ctx, click_back());
        assert_eq!(plugin.view, ChatView::List);

        // The card says how many sub-agents there are and lists none of them yet.
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            !rows
                .iter()
                .any(|(_, _, text)| text == "\u{21b3} check the logs"),
            "a sub-agent card is listed before it is asked for: {rows:?}"
        );
        let note = rows
            .iter()
            .find(|(_, _, text)| text == "1 sub-agent")
            .cloned()
            .unwrap_or_else(|| panic!("the chat does not name its sub-agents: {rows:?}"));
        frame(&mut plugin, &mut settings, &ctx, click(note));
        assert_eq!(
            plugin.view,
            ChatView::List,
            "the note expands the card instead of opening the chat"
        );

        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let row = rows
            .iter()
            .find(|(_, _, text)| text == "\u{21b3} check the logs")
            .cloned()
            .unwrap_or_else(|| panic!("the sub-agent is not listed after the note: {rows:?}"));
        frame(&mut plugin, &mut settings, &ctx, click(row));
        assert_eq!(
            plugin.view,
            ChatView::Subagent(0),
            "the row brings the sub-agent forward"
        );

        let drawn = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let texts: Vec<&str> = drawn.iter().map(|(_, _, text)| text.as_str()).collect();
        assert!(texts.contains(&SUBAGENT_VIEW_NOTICE), "{texts:?}");
        assert!(
            texts
                .iter()
                .any(|text| text.contains("api.service is failing")),
            "the sub-agent's transcript is shown: {texts:?}"
        );

        // Back returns to the chat that owns it rather than to the list.
        frame(&mut plugin, &mut settings, &ctx, click_back());
        assert_eq!(plugin.view, ChatView::Chat);
        frame(&mut plugin, &mut settings, &ctx, click_back());
        assert_eq!(plugin.view, ChatView::List);

        // The note folds them away again, and the card still opens the chat.
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        let note = rows
            .iter()
            .find(|(_, _, text)| text == "1 sub-agent")
            .cloned()
            .expect("the note is still there");
        frame(&mut plugin, &mut settings, &ctx, click(note));
        let rows = frame(&mut plugin, &mut settings, &ctx, Vec::new());
        assert!(
            !rows
                .iter()
                .any(|(_, _, text)| text == "\u{21b3} check the logs"),
            "the note did not hide the sub-agent cards: {rows:?}"
        );
        let stats = rows
            .iter()
            .find(|(_, _, text)| text.starts_with("1 turn"))
            .cloned()
            .expect("the card keeps its stats line");
        frame(&mut plugin, &mut settings, &ctx, click(stats));
        assert_eq!(
            plugin.view,
            ChatView::Chat,
            "the card opens the chat it belongs to"
        );
    }

    #[test]
    fn stop_takes_the_main_run_first_and_the_subagents_second() {
        let (mut plugin, _worker) = busy_pane();
        assert_eq!(plugin.action_state(false), ActionState::StopMain);
        let index = plugin.agent.begin_subagent("check", "do it");
        // A stream still open keeps the sub-agent working after the main run stops.
        let (_sender, events) = std::sync::mpsc::channel();
        plugin.agent.subagents[index]
            .agent
            .turns
            .push(Turn::Assistant {
                text: String::new(),
                stopped: false,
            });
        plugin.agent.subagents[index].agent.phase = Some(Phase::Streaming { turn: 1, events });

        plugin.agent.stop();
        assert_eq!(
            plugin.action_state(false),
            ActionState::StopSubagents,
            "the button stays on to stop what is left"
        );
        assert_eq!(plugin.agent.stop_subagents(), 1);
        assert_eq!(plugin.action_state(false), ActionState::Send);
        // A typed message sends again.
        assert_eq!(plugin.action_state(true), ActionState::Send);
    }

    #[test]
    fn subagents_survive_a_reload_with_their_transcript() {
        let mut settings = serde_json::Map::new();
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "delegate the log check".to_owned(),
            meta: UserMeta::default(),
        });
        let index = plugin
            .agent
            .begin_subagent("check the logs", "Find the failing unit.");
        plugin.agent.subagents[index]
            .agent
            .turns
            .push(Turn::Assistant {
                text: "api.service is failing".to_owned(),
                stopped: false,
            });
        plugin.agent.subagents[index].end = Some(SubagentEnd::Done);
        draw(&mut plugin, &mut settings);
        assert_eq!(
            settings[CONVERSATION_KEY]["subagents"][0]["title"],
            "check the logs"
        );

        let mut reloaded = AiAgent::default();
        draw(&mut reloaded, &mut settings);
        assert_eq!(reloaded.agent.subagents.len(), 1);
        assert_eq!(reloaded.agent.subagents[0].title, "check the logs");
        assert_eq!(reloaded.agent.subagents[0].state(), SubagentState::Done);
        assert!(reloaded.agent.subagents[0].answer().contains("api.service"));
    }

    #[test]
    fn a_subagent_that_was_running_comes_back_stopped() {
        let mut settings = serde_json::Map::new();
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "delegate".to_owned(),
            meta: UserMeta::default(),
        });
        let index = plugin.agent.begin_subagent("check", "do it");
        let (_sender, events) = std::sync::mpsc::channel();
        plugin.agent.subagents[index]
            .agent
            .turns
            .push(Turn::Assistant {
                text: String::new(),
                stopped: false,
            });
        plugin.agent.subagents[index].agent.phase = Some(Phase::Streaming { turn: 1, events });
        draw(&mut plugin, &mut settings);

        let mut reloaded = AiAgent::default();
        draw(&mut reloaded, &mut settings);
        assert_eq!(reloaded.agent.subagents.len(), 1);
        assert_eq!(reloaded.agent.subagents[0].state(), SubagentState::Stopped);
    }

    #[test]
    fn the_header_counts_the_subagents_of_the_open_chat() {
        let (mut plugin, _worker) = busy_pane();
        let index = plugin.agent.begin_subagent("check", "do it");
        let (_sender, events) = std::sync::mpsc::channel();
        plugin.agent.subagents[index]
            .agent
            .turns
            .push(Turn::Assistant {
                text: String::new(),
                stopped: false,
            });
        plugin.agent.subagents[index].agent.phase = Some(Phase::Streaming { turn: 1, events });
        plugin.caption_for = None;
        let mut settings = serde_json::Map::new();
        draw(&mut plugin, &mut settings);
        assert_eq!(
            plugin.header_status().expect("a caption").0,
            "1 sub-agent running"
        );

        // One waiting for an answer is said in the header too.
        plugin.agent.subagents[index].agent.phase = Some(Phase::Allowing {
            call: super::super::protocol::ApiToolCall {
                id: "c1".to_owned(),
                name: "terminal_write".to_owned(),
                arguments: "{}".to_owned(),
            },
            turn: 1,
            label: "type rm".to_owned(),
        });
        plugin.caption_for = None;
        draw(&mut plugin, &mut settings);
        assert_eq!(
            plugin.header_status().expect("a caption").0,
            "A sub-agent needs an answer"
        );
    }
}
