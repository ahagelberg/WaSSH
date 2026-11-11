//! The AI agent pane.
//!
//! A chat with an LLM that can act on the session: run commands, read and write
//! files, drive the terminal, search the web or the knowledge base. Provider and
//! tool work runs off the UI thread, so typing, scrolling and Stop stay
//! responsive while a turn is in flight.

use std::sync::mpsc::Receiver;

use egui::Ui;
use serde_json::{Value, json};
use wassh_core::{Palette, SessionStatus};
use wassh_core::HeaderHealth;
use wassh_ui::widgets::{
    Button, ButtonKind, password_input, select_str, select_width, small_button,
    small_button_enabled, text_area, text_area_with_id,
};

use super::agent::{
    Agent, Phase, SUMMARY_SYSTEM_PROMPT, ToolSettings, Turn, UserMeta, history_from_json,
    history_to_json, turns_from_json, turns_to_json,
};
use super::permissions::{GROUP_KEY_PREFIX, METHOD_KEY_PREFIX, Permissions, setting_key};
use super::protocol::{
    CONVERSATIONS_PER_HOST_MAX, DATA_VERSION, DEFAULT_CHAT_TITLE, MAX_ATTACHMENT_BYTES,
    MAX_ATTACHMENT_CHARS, MAX_ATTACHMENTS, Protocol, ProviderConfig,
    SAFE_RULES, SETTING_DEFAULT_ALLOW_RULES, SETTING_DEFAULT_DENY_RULES, SETTING_GLOBAL_PROMPT,
    SETTING_HOST_ALLOW_RULES, SETTING_HOST_DENY_RULES, SETTING_HOST_PROMPT,
    SETTING_HOST_RAG_FOLDER, SETTING_RAG_FOLDER, SETTING_RAG_MODEL, SETTING_RAG_PROVIDER,
    SETTING_WEB_SEARCH_KEY, SETTING_WEB_SEARCH_PROVIDER, TITLE_MAX_CHARS,
};
use super::providers::{CompletionRequest, EmbedConfig, complete_blocking, provider_models};
use super::settings_ui::{PROVIDERS_KEY, provider_secret_field};
use super::tools;
use wassh_ui::to_color32;
use crate::plugin::{Plugin, PluginContext};
use crate::{PluginId, PluginManifest};

/// Key the conversation is stored under.
const CONVERSATION_KEY: &str = "conversation";
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
];
/// Tokens a reply may use.
const MAX_TOKENS: u32 = 4096;
/// Gap between the provider and model dropdowns under the composer.
const FOOTER_GAP: f32 = 6.0;
/// Type size of the two dropdowns.
const FOOTER_TEXT_SIZE: f32 = 11.0;
/// Narrowest the composer text area gets.
const MIN_COMPOSER_WIDTH: f32 = 80.0;
/// Gap between two entries of the transcript.
const TURN_GAP: f32 = 8.0;
/// Share of the pane a user message may take, like the reference's bubble.
/// Share of the pane a chat message may take.
const MESSAGE_BUBBLE_SHARE: f32 = 0.92;
/// Share of the accent mixed into the user bubble's background.
const USER_BUBBLE_TINT: f32 = 0.20;
/// Share of the accent mixed into the user bubble's border.
const USER_BUBBLE_BORDER_TINT: f32 = 0.40;
/// Share of a tool badge's colour mixed into its pill.
const BADGE_TINT: f32 = 0.25;
/// Characters kept from one tool's output.
const TOOL_OUTPUT_MAX_CHARS: usize = 4_000;
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
/// Padding inside a prompt card.
const PROMPT_CARD_PADDING: i8 = 10;

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
    /// Its text.
    pub text: String,
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
}

/// A model-list fetch running on its own thread: the provider index and the worker.
type ModelFetch = (usize, Receiver<Result<Vec<String>, String>>);

/// A previous conversation, kept so the user can go back to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredChat {
    /// First question, for the picker.
    pub title: String,
    /// The transcript.
    pub turns: Vec<Turn>,
    /// The provider history.
    pub history: Vec<super::protocol::ApiMessage>,
    /// Working directory it ended in.
    pub cwd: String,
    /// Model it used.
    pub model: String,
    /// Provider id it used.
    pub provider: String,
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
    /// A summary running on its own thread.
    summary: Option<(SummaryMode, Receiver<Result<String, String>>)>,
    /// The question composer, while `ask_user` waits.
    answer: String,
    /// Selected answer option for the current question.
    answer_choice: Option<usize>,
    /// The password being typed into the sudo card.
    sudo: String,
    /// Whether the stored conversation has been read back.
    loaded: bool,
    /// Caption and health for the pane header.
    caption: (String, HeaderHealth),
    /// Last problem reported to the user.
    error: Option<String>,
    /// Provider list and keys [`AiAgent::sync_providers`] last derived its state from.
    synced_providers: Option<Value>,
    synced_secrets: std::collections::BTreeMap<String, String>,
    /// Setting values [`AiAgent::apply_settings`] last derived the agent's rules from;
    /// `None` until it has run once.
    applied: Option<serde_json::Map<String, Value>>,
    /// Search key the agent's tools were last given.
    applied_web_key: Option<String>,
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
            answer: String::new(),
            answer_choice: None,
            sudo: String::new(),
            loaded: false,
            caption: (String::new(), HeaderHealth::Ok),
            error: None,
            synced_providers: None,
            synced_secrets: std::collections::BTreeMap::new(),
            applied: None,
            applied_web_key: None,
        }
    }
}

/// Whether `key` is a setting the agent's rules and tools are derived from.
fn derived_setting(key: &str) -> bool {
    DERIVED_SETTINGS.contains(&key)
        || key.starts_with(GROUP_KEY_PREFIX)
        || key.starts_with(METHOD_KEY_PREFIX)
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
    /// Summarise the live conversation for the picker.
    fn stored_chat(&self) -> StoredChat {
        StoredChat {
            title: self.title(),
            turns: self.agent.turns.clone(),
            history: self.agent.history.clone(),
            cwd: self.agent.cwd.clone(),
            model: self.model.clone(),
            provider: self
                .active()
                .map(|provider| provider.id.clone())
                .unwrap_or_default(),
        }
    }

    /// Set the live conversation aside and start an empty one.
    fn begin_new_chat(&mut self) {
        if !self.agent.turns.is_empty() {
            self.chats.push(self.stored_chat());
            let over = self.chats.len().saturating_sub(CONVERSATIONS_PER_HOST_MAX);
            if over > 0 {
                self.chats.drain(..over);
            }
        }
        self.agent.clear();
        self.attachments.clear();
        self.error = None;
        self.answer.clear();
    }

    /// Swap a stored conversation back into the pane.
    fn open_chat(&mut self, index: usize) {
        let Some(chat) = self.chats.get(index).cloned() else {
            return;
        };
        let current = self.stored_chat();
        if !self.agent.turns.is_empty() {
            self.chats[index] = current;
        } else {
            self.chats.remove(index);
        }
        self.agent.clear();
        self.agent.turns = chat.turns;
        self.agent.history = chat.history;
        self.agent.cwd = chat.cwd;
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
    }

    /// The provider in use.
    fn active(&self) -> Option<&ProviderConfig> {
        self.providers.get(self.provider)
    }

    /// The system prompt: what the agent is connected to, the tools this session really
    /// offers, and how to choose between them.
    fn system_prompt(
        &self,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) -> String {
        let mut lines = vec![
            format!(
                "You are an AI development agent connected to the active WaSSH session \"{host}\"."
            ),
            format!("Environment: {environment}."),
            format!("Current working directory: {}", self.agent.cwd),
            String::new(),
            "Available tools:".to_owned(),
        ];
        // Only what the request actually sends: naming a tool this host may not call just
        // invites the model to try it and fall back to the shell.
        for spec in tools::TOOL_SPECS
            .iter()
            .filter(|spec| self.agent.offers_tool(spec.name))
        {
            lines.push(format!("- {}: {}", spec.name, spec.description));
        }
        lines.push(String::new());
        lines.push("Guidelines:".to_owned());
        lines.extend(self.tool_guidance());
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
        lines.join("\n")
    }

    /// How to choose between this session's tools, named as they are offered here.
    ///
    /// A family the host has switched off is not mentioned, so the agent never reads about
    /// a capability it cannot use; a file tool that is missing is reported as such rather
    /// than worked around with a shell command.
    fn tool_guidance(&self) -> Vec<String> {
        let offered = |prefix: &str| -> Vec<&'static str> {
            tools::TOOL_SPECS
                .iter()
                .map(|spec| spec.name)
                .filter(|name| name.starts_with(prefix) && self.agent.offers_tool(name))
                .collect()
        };
        let mut lines = Vec::new();
        // The developer tools are the richer set; the plain remote ones cover what they do
        // not, such as deleting.
        let mut host_files = offered("dev_");
        host_files.extend(offered("remote_fs_"));
        if !host_files.is_empty() {
            lines.push(format!(
                "- Files on the connected host: {}. Read, search and change them with these \
                 tools. Do not reach for cat, head, tail, ls, find, grep/rg, sed or awk through a \
                 shell command: the tools are quicker, they cannot hang on a pager, and they show \
                 the user what changed.",
                host_files.join(", ")
            ));
        }
        let local_files = offered("local_fs_");
        if !local_files.is_empty() {
            lines.push(format!(
                "- Files on this computer: {}.",
                local_files.join(", ")
            ));
        }
        let shells: Vec<&str> = [
            super::protocol::TOOL_RUN_COMMAND,
            super::protocol::TOOL_RUN_SUDO,
            super::protocol::TOOL_LOCAL_RUN,
        ]
        .into_iter()
        .filter(|name| self.agent.offers_tool(name))
        .collect();
        if !shells.is_empty() {
            lines.push(format!(
                "- {} {} for work no tool covers: builds, tests, services, package managers and \
                 version control. Not for reading or editing files.",
                shells.join(", "),
                if shells.len() == 1 { "is" } else { "are" }
            ));
        }
        if self.agent.offers_tool(super::protocol::TOOL_TERMINAL_READ) {
            lines.push(
                "- The terminal_* tools drive the live shell the user is watching; use them for \
                 what only an interactive program can do."
                    .to_owned(),
            );
        }
        if self.agent.offers_tool(super::protocol::TOOL_RUN_SUDO) {
            lines.push(
                "- When a task needs root, call run_sudo with the same command: it asks the user \
                 for their sudo password before running. Plain run_command cannot answer sudo's \
                 prompt, and never pipe a password to sudo yourself."
                    .to_owned(),
            );
        }
        lines.push(
            "- If a file tool you need is not in the list above, tell the user which capability \
             is switched off instead of doing the same job with a shell command."
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
        self.agent.repair_history();
        let provider = self.active()?;
        Some(CompletionRequest {
            base_url: provider.base_url.clone(),
            api_key: self.keys.get(&provider.id).cloned().unwrap_or_default(),
            protocol: provider.protocol,
            model: self.model.trim().to_owned(),
            system: self.system_prompt(settings, host, environment),
            messages: self.agent.history.clone(),
            tools: tools::specs(&self.agent.permissions, self.agent.allow_all),
            max_tokens: MAX_TOKENS,
        })
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

    /// Hand one message to the agent: it becomes a turn and its reply starts.
    fn send_message(
        &mut self,
        message: &str,
        meta: UserMeta,
        settings: &serde_json::Map<String, Value>,
        host: &str,
        environment: &str,
    ) {
        if !self.agent.push_user(message, meta) {
            return;
        }
        self.error = None;
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
            self.error = Some(error.to_owned());
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
        let Some(stored) = settings.get(CONVERSATION_KEY) else {
            return;
        };
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
        if let Some(history) = stored.get("history").and_then(Value::as_array) {
            self.agent.history = history_from_json(history);
        }
        if let Some(chats) = stored.get("chats").and_then(Value::as_array) {
            self.chats = chats
                .iter()
                .map(|chat| StoredChat {
                    title: chat["title"]
                        .as_str()
                        .unwrap_or(DEFAULT_CHAT_TITLE)
                        .to_owned(),
                    cwd: chat["cwd"].as_str().unwrap_or("/").to_owned(),
                    model: chat["model"].as_str().unwrap_or_default().to_owned(),
                    provider: chat["provider"].as_str().unwrap_or_default().to_owned(),
                    turns: chat["turns"]
                        .as_array()
                        .map(|turns| turns_from_json(turns))
                        .unwrap_or_default(),
                    history: chat["history"]
                        .as_array()
                        .map(|history| history_from_json(history))
                        .unwrap_or_default(),
                })
                .collect();
        }
    }

    /// Title for the header and the stored conversation.
    fn title(&self) -> String {
        let first = self.agent.turns.iter().find_map(|turn| match turn {
            Turn::User { text, .. } => Some(text.clone()),
            _ => None,
        });
        match first {
            Some(text) if !text.trim().is_empty() => {
                let trimmed = text.trim();
                let mut title: String = trimmed.chars().take(TITLE_MAX_CHARS).collect();
                if trimmed.chars().count() > TITLE_MAX_CHARS {
                    title.push('…');
                }
                title
            }
            _ => DEFAULT_CHAT_TITLE.to_owned(),
        }
    }

    /// Start a summary of the conversation on a worker thread.
    fn start_summary(
        &mut self,
        mode: SummaryMode,
        settings: &serde_json::Map<String, Value>,
        host: &str,
    ) {
        if self.summary.is_some() || self.agent.turns.is_empty() {
            return;
        }
        let Some(provider) = self.active().cloned() else {
            self.error = Some("Add a provider first.".to_owned());
            return;
        };
        let transcript = self.agent.transcript_text();
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
                }],
                tools: Vec::new(),
                max_tokens: 1_024,
            };
            let result =
                complete_blocking(&request).map(|completion| completion.text.trim().to_owned());
            let _ = sender.send(result);
        });
        self.summary = Some((mode, events));
    }

    /// Collect a finished summary.
    fn poll_summary(&mut self) {
        let Some((mode, events)) = &self.summary else {
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
        self.summary = None;
        match result {
            Err(error) => self.error = Some(error),
            Ok(summary) if summary.is_empty() => {
                self.error = Some("The provider returned an empty summary.".to_owned());
            }
            Ok(summary) => {
                if mode == SummaryMode::Transfer {
                    self.begin_new_chat();
                    self.agent.turns.push(Turn::Notice(format!(
                        "Chat transferred. Carried over:\n{summary}"
                    )));
                    self.agent.history.push(super::protocol::ApiMessage::User {
                        content: format!(
                            "[Context carried over from the previous chat]\n{summary}"
                        ),
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
            self.error = Some(format!("At most {MAX_ATTACHMENTS} files can be attached."));
            return;
        }
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_ATTACHMENT_BYTES => {
                self.error = Some(format!(
                    "{} is larger than {} bytes.",
                    path, MAX_ATTACHMENT_BYTES
                ));
                return;
            }
            Ok(_) => {}
            Err(error) => {
                self.error = Some(format!("Cannot read {path}: {error}"));
                return;
            }
        }
        match std::fs::read(path) {
            Ok(bytes) if bytes.iter().take(2048).any(|byte| *byte == 0) => {
                self.error = Some(format!("{path} looks like a binary file."));
            }
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let text: String = text.chars().take(MAX_ATTACHMENT_CHARS).collect();
                self.attachments.push(Attachment {
                    path: path.to_owned(),
                    text,
                });
            }
            Err(error) => self.error = Some(format!("Cannot read {path}: {error}")),
        }
    }

    /// The user message with attachments and the terminal tail folded in.
    fn message_with_attachments(&self, text: &str, terminal: Option<&str>) -> String {
        let mut message = String::new();
        for attachment in &self.attachments {
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
            "summary" | "context" => self.start_summary(SummaryMode::Summary, settings, host),
            "transfer" => self.start_summary(SummaryMode::Transfer, settings, host),
            _ => {}
        }
    }

    /// Paint the transcript.
    fn transcript(
        &mut self,
        ui: &mut Ui,
        palette: &wassh_core::Palette,
        muted: egui::Color32,
        session: &mut Option<&mut dyn crate::plugin::SessionAccess>,
    ) {
        if self.agent.turns.is_empty() {
            let empty = if self.providers.is_empty() {
                "No model providers configured yet. Configure one in Options."
            } else {
                "Ask the agent to help with this session. It can use terminal tools when you approve them."
            };
            ui.add(egui::Label::new(egui::RichText::new(empty).size(12.0).color(muted)).wrap());
        }
        let mut rewind = None;
        let mut stop_command = false;
        let running = self.agent.running_command();
        let last_user = self
            .agent
            .turns
            .iter()
            .rposition(|turn| matches!(turn, Turn::User { .. }));
        let turn_finished = self.agent.phase.is_none();
        for (index, turn) in self.agent.turns.iter().enumerate() {
            match turn {
                Turn::User { text, meta } => {
                    if user_bubble(ui, palette, text, *meta) {
                        rewind = Some(index);
                    }
                }
                Turn::Assistant { text, stopped } => {
                    // A reply waiting for its first token, or one that only asked for
                    // tools, has no words to show: no bubble for it.
                    if !text.trim().is_empty() {
                        assistant_bubble(ui, palette, text);
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
                } => stop_command |= tool_card(
                    ui,
                    palette,
                    name,
                    label,
                    output,
                    ToolCardState {
                        failed: *failed,
                        running: running == Some(index),
                        turn_index: index,
                        collapsed: tool_output_is_collapsed(
                            name,
                            output,
                            index,
                            last_user,
                            turn_finished,
                        ),
                    },
                ),
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
        if self.waiting_for_the_first_token() {
            let step = (self.agent.now_ms / THINKING_DOT_MS) as usize;
            let dots: String = ".".repeat(1 + step % THINKING_DOT_COUNT);
            ui.label(
                egui::RichText::new(format!("Thinking{dots}"))
                    .size(12.0)
                    .italics()
                    .color(muted),
            );
        }
        // Clicking a message takes the conversation back to it, like the reference.
        if let Some(text) = rewind.and_then(|index| self.agent.rewind_to(index)) {
            self.draft = text;
            self.attachments.clear();
            self.attach_terminal = false;
            self.queued = None;
        }
        if stop_command {
            self.agent.stop_command(session);
        }
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

/// A user message: a bubble against the right edge, tinted with the accent like the
/// reference's `.ai-agent-user-text`. Clicking it rewinds the conversation to that point.
fn user_bubble(ui: &mut Ui, palette: &Palette, text: &str, meta: UserMeta) -> bool {
    let room = ui.available_width() * MESSAGE_BUBBLE_SHARE;
    let fill = to_color32(wassh_core::colors::tint(
        palette.accent,
        palette.bg_panel_2,
        USER_BUBBLE_TINT,
    ));
    let border = to_color32(wassh_core::colors::tint(
        palette.accent,
        palette.border,
        USER_BUBBLE_BORDER_TINT,
    ));
    let response = ui
        .with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            egui::Frame::new()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, border))
                .corner_radius(wassh_core::RADIUS)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_max_width(room);
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
                                    .color(to_color32(palette.text_muted)),
                            );
                        }
                        super::markdown::render(ui, palette, text);
                    });
                })
                .response
        })
        .inner;
    response
        .interact(egui::Sense::click())
        .on_hover_text("Click to rewind to this message")
        .clicked()
}

/// An assistant response: Markdown in a neutral bubble against the left edge.
fn assistant_bubble(ui: &mut Ui, palette: &Palette, text: &str) {
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
                    super::markdown::render(ui, palette, text);
                });
            });
    });
}

/// One tool call: the reference's `.ai-agent-tool` card — a badge, the call, its outcome
/// and the output. Returns whether its Stop button was clicked.
struct ToolCardState {
    failed: bool,
    running: bool,
    turn_index: usize,
    collapsed: bool,
}

fn tool_card(
    ui: &mut Ui,
    palette: &Palette,
    name: &str,
    label: &str,
    output: &str,
    state: ToolCardState,
) -> bool {
    let ToolCardState {
        failed,
        running,
        turn_index,
        collapsed,
    } = state;
    let (badge, colour) = tool_badge(name, palette);
    let muted = to_color32(palette.text_muted);
    let output_id = ui.make_persistent_id(("ai-command-output", turn_index));
    let mut stop = false;
    egui::Frame::new()
        .fill(to_color32(palette.bg_input))
        .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
        .corner_radius(wassh_core::RADIUS)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let shown = if output.chars().count() > TOOL_OUTPUT_MAX_CHARS {
                let head: String = output.chars().take(TOOL_OUTPUT_MAX_CHARS).collect();
                format!("{head}\n…")
            } else {
                output.to_owned()
            };
            let (outcome, outcome_colour) = if running {
                ("running", muted)
            } else if failed {
                ("failed", to_color32(palette.danger))
            } else {
                ("ok", to_color32(palette.ok))
            };
            if collapsed {
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    output_id,
                    false,
                )
                    .show_header(ui, |ui| {
                        badge_pill(ui, palette, badge, colour);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                        ui.label(
                            egui::RichText::new(if output.is_empty() {
                                "(no output)"
                            } else {
                                &shown
                            })
                            .size(11.0)
                            .monospace()
                            .color(muted),
                        );
                    });
            } else {
                ui.horizontal(|ui| {
                    badge_pill(ui, palette, badge, colour);
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
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(outcome)
                            .size(10.0)
                            .strong()
                            .color(outcome_colour),
                    );
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
                });
                ui.label(
                    egui::RichText::new(if output.is_empty() {
                        "(no output)"
                    } else {
                        &shown
                    })
                    .size(11.0)
                    .monospace()
                    .color(muted),
                );
            }
        });
    stop
}

fn tool_output_is_collapsed(
    name: &str,
    output: &str,
    index: usize,
    last_user: Option<usize>,
    turn_finished: bool,
) -> bool {
    let collapse_by_tool = matches!(
        name,
        super::protocol::TOOL_RUN_COMMAND
            | super::protocol::TOOL_RUN_SUDO
            | super::protocol::TOOL_LOCAL_RUN
    ) || name.starts_with("remote_fs_");
    let has_lots_of_output = output.chars().count() > TOOL_OUTPUT_MAX_CHARS;
    (collapse_by_tool || has_lots_of_output)
        && (turn_finished || last_user.is_some_and(|last_user| index < last_user))
}

#[test]
fn tool_output_collapse_state_tracks_tool_and_output_size() {
    use super::protocol::{TOOL_LOCAL_RUN, TOOL_RUN_COMMAND, TOOL_RUN_SUDO};

    let small_output = "short output";
    let large_output = format!("{}x", "x".repeat(TOOL_OUTPUT_MAX_CHARS));
    assert!(tool_output_is_collapsed(TOOL_RUN_COMMAND, small_output, 3, None, true));
    assert!(tool_output_is_collapsed(TOOL_RUN_SUDO, small_output, 3, Some(5), false));
    assert!(tool_output_is_collapsed(TOOL_LOCAL_RUN, small_output, 3, Some(5), false));
    assert!(!tool_output_is_collapsed(TOOL_RUN_COMMAND, small_output, 6, Some(5), false));
    assert!(tool_output_is_collapsed("remote_fs_read_file", small_output, 3, None, true));
    assert!(tool_output_is_collapsed("web_fetch", &large_output, 3, None, true));
    assert!(!tool_output_is_collapsed("web_fetch", small_output, 3, None, true));
    assert!(!tool_output_is_collapsed("web_fetch", &large_output, 3, None, false));
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

/// What the user did with the prompt cards this frame.
#[derive(Default)]
struct PromptAnswers {
    /// Approval card: (allow, scope to remember, allow every command).
    approval: Option<(bool, Option<usize>, bool)>,
    /// Capability card: (allow, remember the answer).
    permission: Option<(bool, bool)>,
    /// Capability card: allow every prompt for the rest of the session.
    allow_session: bool,
    /// The question card's answer.
    answer: Option<String>,
    /// The sudo card.
    sudo: Option<SudoAnswer>,
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
    ) -> PromptAnswers {
        let mut answers = PromptAnswers::default();

        // The approval card: the command the model wants to run, and how far to allow it.
        let approval = match &self.agent.phase {
            Some(Phase::Approving { command, scopes, .. }) => {
                Some((command.clone(), scopes.clone()))
            }
            _ => None,
        };
        if let Some((command, scopes)) = approval {
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
                    prompt_command(ui, palette, &command);
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
                        if Button::new("Allow all this session")
                            .compact()
                            .show(ui, palette)
                            .clicked()
                        {
                            everything = true;
                        }
                    });
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
        let asking = match &self.agent.phase {
            Some(Phase::Allowing { label, .. }) => Some(label.clone()),
            _ => None,
        };
        if let Some(label) = asking {
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
                    prompt_command(ui, palette, &label);
                },
                |ui| {
                    prompt_action_row(ui, "permission", |ui| {
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
                        if Button::new("Allow all this session")
                            .compact()
                            .show(ui, palette)
                            .clicked()
                        {
                            answers.allow_session = true;
                        }
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
                },
            );
        }

        // The question card.
        let question = match &self.agent.phase {
            Some(Phase::Asking {
                questions,
                current,
                ..
            }) => questions
                .get(*current)
                .cloned()
                .map(|question| (question, *current, questions.len())),
            _ => None,
        };
        if let Some((question, current, question_count)) = question {
            prompt_card(
                ui,
                palette,
                "question",
                |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Question {} of {}",
                            current + QUESTION_INDEX_OFFSET,
                            question_count
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
                    let free_text_index = question.options.len();
                    let is_last = current + QUESTION_INDEX_OFFSET >= question_count;
                    let mut answer_on_enter = false;
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(room, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(room);
                            if !question.options.is_empty() {
                                for (index, option) in question.options.iter().enumerate() {
                                    ui.radio_value(
                                        &mut self.answer_choice,
                                        Some(index),
                                        option,
                                    );
                                }
                                ui.radio_value(
                                    &mut self.answer_choice,
                                    Some(free_text_index),
                                    "Other (write your own)",
                                );
                            }
                            let free_text_selected = question.options.is_empty()
                                || self.answer_choice == Some(free_text_index);
                            if free_text_selected {
                                let response =
                                    text_area(ui, palette, &mut self.answer, 2, "Your answer…");
                                answer_on_enter = response.has_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                                    && !ui.input(|input| input.modifiers.shift);
                            }
                        },
                    );
                    let answer = if question.options.is_empty()
                        || self.answer_choice == Some(free_text_index)
                    {
                        self.answer.trim().to_owned()
                    } else {
                        self.answer_choice
                            .and_then(|index| question.options.get(index))
                            .cloned()
                            .unwrap_or_default()
                    };
                    if !answer.is_empty() && answer_on_enter {
                        answers.answer = Some(answer);
                    } else {
                        let label = if is_last { "Continue" } else { "Next" };
                        if small_button_enabled(ui, palette, label, !answer.is_empty()) {
                            answers.answer = Some(answer);
                        }
                    }
                },
            );
        }

        // The sudo card: the command that needs root, and the password for it.
        let sudo = self
            .agent
            .sudo_prompt()
            .map(|(command, reason)| (command.to_owned(), reason.map(str::to_owned)));
        if let Some((command, reason)) = sudo {
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
                    prompt_command(ui, palette, &command);
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
        self.apply_settings(context.settings, context.secret(SETTING_WEB_SEARCH_KEY));
        self.poll_models();
        self.refresh_models(context);
        self.agent.now_ms = wassh_core::time::now_ms();
        self.poll_summary();
        self.poll_picker();

        // Drain workers, then answer any tool result it produced.
        self.agent.poll(&mut session);
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
                        context.settings,
                        &context.host_label,
                        &context.environment,
                    );
                }
                Err(error) => {
                    self.error = Some(error.to_owned());
                    self.queued = Some(queued);
                }
            }
        }
        self.caption = if self.agent.busy() || self.summary.is_some() {
            ("Thinking…".to_owned(), HeaderHealth::Warn)
        } else if context.status == SessionStatus::Connected {
            (self.title(), HeaderHealth::Ok)
        } else {
            ("Session not connected".to_owned(), HeaderHealth::Warn)
        };

        let busy = self.agent.busy();

        // The pane controls; provider and model live under the composer.
        ui.horizontal(|ui| {
            if small_button_enabled(
                ui,
                palette,
                "New chat",
                !busy && !self.agent.turns.is_empty(),
            ) {
                self.begin_new_chat();
            }
        });

        // Previous conversations.
        if !self.chats.is_empty() {
            let chosen = {
                let titles: Vec<&str> = self.chats.iter().map(|chat| chat.title.as_str()).collect();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Chats").size(11.0).color(muted));
                    select_str(ui, "ai-chats", "", &titles)
                })
                .inner
            };
            if let Some(chosen) = chosen
                && let Some(index) = self.chats.iter().position(|chat| chat.title == chosen)
            {
                self.open_chat(index);
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
        let slash = parse_slash_input(&self.draft);
        egui::Panel::bottom("ai-composer")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .show(ui, |ui| {
                // A request the agent is blocked on comes first: it is the one thing the
                // user has to answer before the run can go on.
                prompts = self.prompt_cards(ui, palette, muted);

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
                let (chosen_provider, chosen_model, action_clicked) = {
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
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = FOOTER_GAP;
                        // The reference footer is set in the same small type as the rest.
                        ui.style_mut().text_styles.insert(
                            egui::TextStyle::Button,
                            egui::FontId::proportional(FOOTER_TEXT_SIZE),
                        );
                        let can_send =
                            !self.providers.is_empty() && !self.draft.trim().is_empty();
                        let action = if busy && !can_send {
                            Button::new("Stop").compact().kind(ButtonKind::Danger)
                        } else {
                            Button::new("Send").compact().enabled(can_send)
                        };
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
                            return (None, None, action.show(ui, palette).clicked());
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
                        (provider, model, action.show(ui, palette).clicked())
                    })
                    .inner
                };
                if action_clicked {
                    if busy && self.draft.trim().is_empty() {
                        self.agent.stop();
                    } else {
                        submit = true;
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

        // Transcript.
        egui::ScrollArea::vertical()
            .id_salt("ai-transcript")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                self.transcript(ui, palette, muted, &mut session);

                if let Some(error) = &self.error {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("⚠ {error}"))
                                .size(11.0)
                                .color(to_color32(palette.danger)),
                        )
                        .wrap(),
                    );
                }
            });

        if let Some((allow, remember, everything)) = prompts.approval {
            if everything {
                self.agent.approve_everything(&mut session);
            } else {
                self.agent.approve(allow, remember, &mut session);
            }
        }
        if prompts.allow_session {
            self.agent.allow_everything(&mut session);
        } else if let Some((allow, always)) = prompts.permission {
            if always
                && let Some(Phase::Allowing { call, .. }) = &self.agent.phase
                && let Some(key) = setting_key(&call.name)
            {
                context.set_text(&key, if allow { "allow" } else { "deny" });
            }
            self.agent.answer_permission(allow, &mut session);
        }
        if let Some(answer) = prompts.answer {
            self.answer.clear();
            self.answer_choice = None;
            self.agent.answer(&answer, &mut session);
        }
        match prompts.sudo {
            Some(SudoAnswer::Run) => {
                let password = std::mem::take(&mut self.sudo);
                self.agent.submit_sudo(&password, &mut session);
            }
            Some(SudoAnswer::Cancel) => self.agent.cancel_sudo(&mut session),
            None => {}
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
                self.error = Some(error.to_owned());
            } else {
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
                self.draft.clear();
                self.attachments.clear();
                self.attach_terminal = false;
                if self.agent.blocked() && queue {
                    // Alt+Enter holds the message back until the run is over.
                    self.queued = Some(Queued {
                        text,
                        message,
                        meta,
                    });
                } else if self.agent.blocked() {
                    // Anything else joins the conversation the agent is in the middle of.
                    self.agent.inject_user(&message, meta);
                } else {
                    self.send_message(
                        &message,
                        meta,
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
                    "cwd": chat.cwd,
                    "model": chat.model,
                    "provider": chat.provider,
                    "turns": turns_to_json(&chat.turns),
                    "history": history_to_json(&chat.history)
                })
            })
            .collect();
        context.set_value(
            CONVERSATION_KEY,
            json!({
                "version": DATA_VERSION,
                "title": self.title(),
                "model": self.model,
                "provider": self.active().map(|provider| provider.id.clone()),
                "cwd": self.agent.cwd,
                "turns": turns_to_json(&self.agent.turns),
                "history": history_to_json(&self.agent.history),
                "chats": chats
            }),
        );
        context.session = session;
        // A run in flight, a summary or an attachment being read all finish outside a frame.
        if self.agent.busy()
            || self.summary.is_some()
            || self.picking.is_some()
            || self.fetch.is_some()
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

    use super::super::settings_ui::{API_KEYS_KEY, provider_secret_field};
    use super::*;
    use wassh_core::Palette;

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
        rows
    }

    /// A provider stand-in: lists `models` and answers one chat call with `chat`, then
    /// reports the chat request it was sent.
    fn provider(
        chat: &'static str,
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
                    http_response("text/event-stream", chat)
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
    fn the_pane_opens_on_a_provider_with_a_hint() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        // Nothing is configured yet, so the pane says where to add a provider.
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(
            texts.contains(&"No model providers configured — add one in Options."),
            "the missing-provider hint is not shown: {texts:?}"
        );
        assert!(
            texts.contains(&"No model providers configured yet. Configure one in Options."),
            "empty hint missing: {texts:?}"
        );
        assert!(texts.contains(&"Send"), "composer missing: {texts:?}");
        assert!(
            texts.contains(&ATTACH_GLYPH),
            "attach button missing: {texts:?}"
        );
        assert!(
            texts.contains(&TERMINAL_ATTACH_GLYPH),
            "terminal-context toggle missing: {texts:?}"
        );
        assert_eq!(
            plugin.header_status(),
            Some((DEFAULT_CHAT_TITLE, HeaderHealth::Ok))
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
        assert_eq!(
            plugin.header_status(),
            Some(("how much disk is left", HeaderHealth::Ok))
        );
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
    fn permission_actions_share_one_row() {
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
        assert_actions_share_row(
            &permission_rows,
            &[
                "Approve once",
                "Deny once",
                "Allow all this session",
                "Always allow",
                "Always deny",
            ],
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
        assert_actions_share_row(
            &approval_rows,
            &[
                "Allow",
                "Deny",
                "Allow all this session",
            ],
        );
        assert_actions_stack_below(&approval_rows, &["Allow", "Deny", "Allow all this session"]);
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
        let send = texts
            .iter()
            .find(|(_, text)| text == "Send")
            .map(|(pos, _)| *pos)
            .expect("the send button is drawn");
        assert!(send.y < 760.0, "the card pushed the composer away");
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
            current: 0,
            answers: Vec::new(),
        });
        plugin.agent.turns.push(Turn::Question {
            text: "Which file?".to_owned(),
            answer: None,
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
        assert!(texts.contains(&"Continue"), "continue missing: {texts:?}");
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
            current: 0,
            answers: Vec::new(),
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
            current: 0,
            answers: Vec::new(),
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

    #[test]
    fn a_multi_question_card_shows_choices_and_advances_one_at_a_time() {
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
                    options: Vec::new(),
                },
            ],
            current: 0,
            answers: Vec::new(),
        });
        plugin.agent.turns.push(Turn::Question {
            text: "Which language?".to_owned(),
            answer: None,
        });
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();

        for expected in [
            "Question 1 of 2",
            "Which language?",
            "Rust",
            "Go",
            "Other (write your own)",
            "Next",
        ] {
            assert!(texts.contains(&expected), "{expected} missing: {texts:?}");
        }
        assert!(!texts.contains(&"Continue"), "not on the final question");
    }

    #[test]
    fn slash_commands_parse_like_the_reference() {
        let exact = parse_slash_input("/new").unwrap();
        assert_eq!(exact.exact.map(|command| command.name), Some("new"));
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
    fn attachments_and_the_terminal_tail_are_folded_into_the_message() {
        let mut plugin = AiAgent::default();
        plugin.attachments.push(Attachment {
            path: "/tmp/notes.txt".to_owned(),
            text: "line one".to_owned(),
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
                    &message,
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
        let user_fill = to_color32(wassh_core::colors::tint(
            palette.accent, palette.bg_panel_2, USER_BUBBLE_TINT,
        ));
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
                        user_bubble(ui, &palette, &prompt, UserMeta::default());
                        tool_card(ui, &palette, "run_command", &"long-command".repeat(30), &terminal, ToolCardState {
                            failed: false, running: false, turn_index: 0, collapsed: false,
                        });
                        tool_card(ui, &palette, "run_command", &"long-command".repeat(30), &terminal, ToolCardState {
                            failed: false, running: false, turn_index: 1, collapsed: true,
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
                && (text.galley.text().contains("long-path") || text.galley.text().contains("long-line"))
            {
                found += 1;
                let limit = body.right().min(clipped.clip_rect.right());
                assert!(text.pos.x + text.galley.size().x <= limit, "text exceeds pane: {} > {limit}", text.pos.x + text.galley.size().x);
            }
        }
        assert_eq!(found, 2);
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
            assistant_bubble(ui, &palette, "Agent response");
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
        let user_fill = to_color32(wassh_core::colors::tint(
            palette.accent,
            palette.bg_panel_2,
            USER_BUBBLE_TINT,
        ));
        assert_ne!(to_color32(palette.bg_panel_2), user_fill);
        assert!(bubble.left() < WIDE_PANE / 2.0, "bubble should start on the left");
        assert!(
            response.x >= bubble.left() && response.x < bubble.center().x,
            "response text should be left-aligned inside its bubble: {response:?} in {bubble:?}"
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
        let mut output =
            ctx.run_ui(input, |ui| plugin.transcript(ui, &palette, muted, &mut session));
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
                        collapsed: false,
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
                        collapsed: true,
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
    fn the_prompt_offers_only_the_tools_the_host_may_use() {
        use super::super::permissions::{GROUP_LOCAL_FS, GROUP_REMOTE_FS, Permissions, group_key};
        let prompt_for = |groups: Vec<&str>| {
            let settings: serde_json::Map<String, Value> =
                groups.into_iter().map(|group| (group_key(group), json!(true))).collect();
            let mut plugin = AiAgent::default();
            plugin.agent.permissions = Permissions::from_settings(&settings);
            plugin.system_prompt(&serde_json::Map::new(), "dev", "a test shell")
        };

        // With both filesystem groups on, both families are named and the shell is not
        // offered as a way to read files.
        let both = prompt_for(vec![GROUP_REMOTE_FS, GROUP_LOCAL_FS]);
        assert!(both.contains("Environment: a test shell."), "{both}");
        assert!(both.contains("- dev_grep_search:"), "{both}");
        assert!(both.contains("- local_fs_grep_search:"), "{both}");
        assert!(both.contains("Files on the connected host:"), "{both}");
        assert!(both.contains("Files on this computer:"), "{both}");
        assert!(
            both.contains("Do not reach for cat, head, tail, ls, find, grep/rg, sed or awk"),
            "{both}"
        );
        assert!(both.contains("Not for reading or editing files"), "{both}");

        // With only the local group on, the host's tools are not advertised at all.
        let local_only = prompt_for(vec![GROUP_LOCAL_FS]);
        assert!(local_only.contains("- local_fs_grep_search:"), "{local_only}");
        assert!(!local_only.contains("dev_view_file"), "{local_only}");
        assert!(
            !local_only.contains("Files on the connected host:"),
            "{local_only}"
        );
        // …and the agent is told to say a capability is off rather than shell around it.
        assert!(
            local_only.contains("tell the user which capability is switched off"),
            "{local_only}"
        );

        // A host with nothing switched on gets the shells and no file tools.
        let bare = prompt_for(Vec::new());
        assert!(bare.contains("- run_command:"), "{bare}");
        assert!(!bare.contains("local_fs_read_file"), "{bare}");
        assert!(!bare.contains("dev_view_file"), "{bare}");
    }

    #[test]
    fn a_long_question_is_cut_short_in_the_header() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User {
            text: "x".repeat(200),
            meta: UserMeta::default(),
        });
        let caption = plugin.title();
        assert_eq!(caption.chars().count(), TITLE_MAX_CHARS + 1);
        assert!(caption.ends_with('…'));
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
        let mut plugin = AiAgent::default();
        draw(&mut plugin, &mut settings);
        let stored = &settings[CONVERSATION_KEY];
        assert_eq!(stored["version"], DATA_VERSION);
        assert_eq!(stored["title"], "hello");
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
            });
        plugin.begin_new_chat();
        assert!(plugin.agent.turns.is_empty(), "the live chat was cleared");
        assert_eq!(plugin.chats.len(), 1);
        assert_eq!(plugin.chats[0].title, "first question");

        // Opening it back restores the transcript and history.
        plugin.open_chat(0);
        assert_eq!(plugin.agent.turns.len(), 1);
        assert_eq!(plugin.agent.history.len(), 1);
        assert!(plugin.chats.is_empty(), "the empty chat is not kept");
    }

    #[test]
    fn the_oldest_chats_are_dropped_at_the_cap() {
        let mut plugin = AiAgent::default();
        for index in 0..(CONVERSATIONS_PER_HOST_MAX + 3) {
            plugin.agent.turns.push(Turn::User {
                text: format!("question {index}"),
                meta: UserMeta::default(),
            });
            plugin.begin_new_chat();
        }
        assert_eq!(plugin.chats.len(), CONVERSATIONS_PER_HOST_MAX);
        assert_eq!(plugin.chats[0].title, "question 3");
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
        assert_eq!(plugin.chats[0].title, "old chat");
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
}
