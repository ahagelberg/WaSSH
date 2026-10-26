//! The AI agent pane.
//!
//! A chat with an LLM that can act on the session: run commands, read and write
//! files, drive the terminal, search the web or the knowledge base. Provider and
//! tool work runs off the UI thread, so typing, scrolling and Stop stay
//! responsive while a turn is in flight.

use std::sync::mpsc::Receiver;

use egui::Ui;
use serde_json::{json, Value};
use wassh_core::SessionStatus;
use wassh_ui::dock::HeaderHealth;
use wassh_ui::widgets::{
    Button, ButtonKind, chip, select_str, select_width, small_button_enabled, text_area, text_input,
};

use super::agent::{
    history_from_json, history_to_json, turns_from_json, turns_to_json, Agent, Phase, ToolSettings,
    Turn, SUMMARY_SYSTEM_PROMPT,
};
use super::protocol::{
    Protocol, ProviderConfig, CONVERSATIONS_PER_HOST_MAX, DATA_VERSION,
    DEFAULT_CHAT_TITLE, MAX_ATTACHMENTS,
    MAX_ATTACHMENT_CHARS, MAX_ATTACHMENT_BYTES, PROVIDER_OPENROUTER, SAFE_RULES,
    SETTING_DEFAULT_ALLOW_RULES, SETTING_DEFAULT_DENY_RULES, SETTING_GLOBAL_PROMPT,
    SETTING_HOST_ALLOW_RULES, SETTING_HOST_DENY_RULES, SETTING_HOST_PROMPT, SETTING_HOST_RAG_FOLDER,
    SETTING_RAG_FOLDER, SETTING_RAG_MODEL, SETTING_RAG_PROVIDER, SETTING_WEB_SEARCH_KEY,
    SETTING_WEB_SEARCH_PROVIDER, TITLE_MAX_CHARS,
};
use super::providers::{complete_blocking, provider_models, CompletionRequest, EmbedConfig};
use super::settings_ui::{PROVIDERS_KEY, provider_secret_field};
use super::tools;
use crate::color::to_color32;
use crate::plugin::{Plugin, PluginContext};
use crate::{PluginId, PluginManifest};

/// Key the conversation is stored under.
const CONVERSATION_KEY: &str = "conversation";
/// Tokens a reply may use.
const MAX_TOKENS: u32 = 4096;
/// Gap between the provider and model dropdowns under the composer.
const FOOTER_GAP: f32 = 6.0;
/// Narrowest a footer dropdown gets before it stops shrinking.
const FOOTER_SELECT_MIN_WIDTH: f32 = 80.0;
/// Room the provider and model row takes under the composer.
const FOOTER_HEIGHT: f32 = 26.0;
/// Type size of the two dropdowns.
const FOOTER_TEXT_SIZE: f32 = 11.0;
/// Room the composer and the two buttons under it keep free.
const COMPOSER_HEIGHT: f32 = 116.0;
/// The same while the agent waits for an answer to its question.
const COMPOSER_HEIGHT_ASKING: f32 = 150.0;
/// Shortest the transcript gets, whatever the pane height.
const MIN_TRANSCRIPT_HEIGHT: f32 = 80.0;
/// Room kept for the Send / Stop button next to the composer.
const SEND_BUTTON_WIDTH: f32 = 96.0;
/// Narrowest the composer text area gets.
const MIN_COMPOSER_WIDTH: f32 = 80.0;
/// Caption of the file-attach button.
const ATTACH_GLYPH: &str = "📎";
/// Caption of the terminal-context toggle.
const TERMINAL_ATTACH_GLYPH: &str = "⌁";
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
    /// The sudo password field, after a refused command.
    sudo: String,
    /// Whether the stored conversation has been read back.
    loaded: bool,
    /// Caption and health for the pane header.
    caption: (String, HeaderHealth),
    /// Last problem reported to the user.
    error: Option<String>,
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
            attachments: Vec::new(),
            attach_terminal: false,
            picking: None,
            fetch: None,
            refreshed: std::collections::BTreeSet::new(),
            app_writes: Vec::new(),
            summary: None,
            answer: String::new(),
            sudo: String::new(),
            loaded: false,
            caption: (String::new(), HeaderHealth::Ok),
            error: None,
        }
    }
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

/// The wall clock in milliseconds, from the system clock.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
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
            provider: self.active().map(|provider| provider.id.clone()).unwrap_or_default(),
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
        if let Some(position) = self.providers.iter().position(|item| item.id == chat.provider) {
            self.provider = position;
        }
        self.attachments.clear();
        self.error = None;
    }

    /// The provider in use.
    fn active(&self) -> Option<&ProviderConfig> {
        self.providers.get(self.provider)
    }

    /// The system prompt, as the reference builds it.
    fn system_prompt(&self, settings: &serde_json::Map<String, Value>, host: &str) -> String {
        let mut lines = vec![
            format!(
                "You are an AI development agent connected to the active WaSSH session \"{host}\"."
            ),
            format!("Current working directory: {}", self.agent.cwd),
            String::new(),
            "Available tools:".to_owned(),
        ];
        for spec in tools::TOOL_SPECS {
            lines.push(format!("- {}: {}", spec.name, spec.description));
        }
        lines.extend([
            String::new(),
            "Guidelines:".to_owned(),
            "- Use run_command for terminal work on the host and the terminal_* tools to drive the live shell."
                .to_owned(),
            "- Prefer the file tools over shell commands when reading or editing files.".to_owned(),
            "- After each command or tool call you see its actual result. Never invent output.".to_owned(),
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

    /// Rules and tool settings from the plugin data.
    ///
    /// `web_key` is the search provider's API key, which the shell decrypts from
    /// the credential vault instead of keeping it in a settings file.
    fn apply_settings(
        &mut self,
        settings: &serde_json::Map<String, Value>,
        web_key: Option<&str>,
    ) {
        self.agent.app_allow = setting_rules(settings, SETTING_DEFAULT_ALLOW_RULES)
            .unwrap_or_else(|| SAFE_RULES.iter().map(|rule| (*rule).to_owned()).collect());
        self.agent.app_deny =
            setting_rules(settings, SETTING_DEFAULT_DENY_RULES).unwrap_or_default();
        self.agent.host_allow =
            setting_rules(settings, SETTING_HOST_ALLOW_RULES).unwrap_or_default();
        self.agent.host_deny =
            setting_rules(settings, SETTING_HOST_DENY_RULES).unwrap_or_default();

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

    /// The embedding provider from the settings, when one is configured.
    fn embedding_config(
        &self,
        settings: &serde_json::Map<String, Value>,
    ) -> Option<EmbedConfig> {
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
    fn request(
        &self,
        settings: &serde_json::Map<String, Value>,
        host: &str,
    ) -> Option<CompletionRequest> {
        let provider = self.active()?;
        Some(CompletionRequest {
            base_url: provider.base_url.clone(),
            api_key: self.keys.get(&provider.id).cloned().unwrap_or_default(),
            protocol: provider.protocol,
            model: self.model.trim().to_owned(),
            system: self.system_prompt(settings, host),
            messages: self.agent.history.clone(),
            tools: tools::specs(),
            max_tokens: MAX_TOKENS,
        })
    }

    /// Take the provider list and API keys from the app-wide settings and the vault.
    ///
    /// Options owns both, so they are re-read every frame and never written back here;
    /// the current provider and model stay per conversation. Keys arrive decrypted
    /// from the host, because a pane cannot reach the credential vault itself.
    fn sync_providers(
        &mut self,
        settings: &serde_json::Map<String, Value>,
        secrets: &std::collections::BTreeMap<String, String>,
    ) {
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
                    title: chat["title"].as_str().unwrap_or(DEFAULT_CHAT_TITLE).to_owned(),
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
            Turn::User(text) => Some(text.clone()),
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
            let result = complete_blocking(&request).map(|completion| completion.text.trim().to_owned());
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
                        content: format!("[Context carried over from the previous chat]\n{summary}"),
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
    fn run_command(&mut self, command: SlashCommand, settings: &serde_json::Map<String, Value>, host: &str) {
        self.draft.clear();
        match command.name {
            "new" => self.begin_new_chat(),
            "summary" | "context" => self.start_summary(SummaryMode::Summary, settings, host),
            "transfer" => self.start_summary(SummaryMode::Transfer, settings, host),
            _ => {}
        }
    }

    /// Paint the transcript.
    fn transcript(&self, ui: &mut Ui, palette: &wassh_core::Palette, muted: egui::Color32) {
        if self.agent.turns.is_empty() {
            ui.label(
                egui::RichText::new("Ask anything about this session.")
                    .size(12.0)
                    .color(muted),
            );
        }
        for turn in &self.agent.turns {
            match turn {
                Turn::User(text) => {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .size(12.5)
                                .color(to_color32(palette.accent))
                                .strong(),
                        )
                        .wrap(),
                    );
                }
                Turn::Assistant { text, stopped } => {
                    // Answers are Markdown, like the reference's react-markdown.
                    super::markdown::render(ui, palette, text);
                    if *stopped {
                        ui.label(egui::RichText::new("stopped").size(10.0).color(muted));
                    }
                }
                Turn::Tool {
                    label,
                    output,
                    failed,
                    ..
                } => {
                    ui.label(
                        egui::RichText::new(label.as_str())
                            .size(11.5)
                            .monospace()
                            .color(if *failed {
                                to_color32(palette.danger)
                            } else {
                                muted
                            }),
                    );
                    if !output.is_empty() {
                        let shown = if output.chars().count() > 4_000 {
                            let head: String = output.chars().take(4_000).collect();
                            format!("{head}\n…")
                        } else {
                            output.clone()
                        };
                        ui.label(
                            egui::RichText::new(shown)
                                .size(11.0)
                                .monospace()
                                .color(muted),
                        );
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
            ui.add_space(6.0);
        }
    }
}

impl Plugin for AiAgent {
    fn manifest(&self) -> PluginManifest {
        PluginId::AiAgent.manifest()
    }

    fn header_status(&self) -> Option<(String, HeaderHealth)> {
        (!self.caption.0.is_empty()).then(|| self.caption.clone())
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
        self.agent.now_ms = now_ms();
        self.poll_summary();
        self.poll_picker();

        // Drain workers, then answer any tool result it produced.
        self.agent.poll(&mut session);
        if self.agent.take_needs_reply() {
            let request = self.request(context.settings, &context.host_label);
            if let Some(request) = request {
                self.agent.start(request);
            }
        }
        if self.agent.busy() || self.summary.is_some() || self.picking.is_some() || self.fetch.is_some() {
            ui.ctx().request_repaint();
        }

        self.caption = if self.agent.busy() || self.summary.is_some() {
            ("Thinking…".to_owned(), HeaderHealth::Warn)
        } else if context.status == SessionStatus::Connected {
            (self.title(), HeaderHealth::Ok)
        } else {
            ("Session not connected".to_owned(), HeaderHealth::Warn)
        };

        let busy = self.agent.busy();
        let mut sudo_retry = false;

        // The pane controls; provider and model live under the composer.
        ui.horizontal(|ui| {
            if small_button_enabled(ui, palette, "New chat", !busy && !self.agent.turns.is_empty()) {
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

        // A refused sudo password is asked for here, like the reference.
        if self.agent.sudo_waiting {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("sudo password").size(11.0).color(muted));
                let room = (ui.available_width() - 96.0).max(80.0);
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(room, 0.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_width(room);
                        text_input(ui, palette, &mut self.sudo, "password");
                    },
                );
                if small_button_enabled(ui, palette, "Run with sudo", !self.sudo.is_empty()) {
                    sudo_retry = true;
                }
            });
        }
        ui.separator();

        // Transcript.
        let composer_height = if self.agent.asking() {
            COMPOSER_HEIGHT_ASKING
        } else {
            COMPOSER_HEIGHT
        } + FOOTER_HEIGHT;
        let height = (ui.available_height() - composer_height).max(MIN_TRANSCRIPT_HEIGHT);
        let mut approval: Option<(bool, Option<usize>, bool)> = None;
        let mut answer: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt("ai-transcript")
            .max_height(height)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                self.transcript(ui, palette, muted);

                // The approval card.
                let card = match &self.agent.phase {
                    Some(Phase::Approving {
                        command, scopes, ..
                    }) => Some((command.clone(), scopes.clone())),
                    _ => None,
                };
                if let Some((command, scopes)) = card {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Run this command?")
                            .size(12.0)
                            .strong()
                            .color(to_color32(palette.text)),
                    );
                    ui.label(
                        egui::RichText::new(command.as_str())
                            .size(11.5)
                            .monospace()
                            .color(to_color32(palette.accent)),
                    );
                    let mut chosen: Option<(bool, Option<usize>)> = None;
                    let mut everything = false;
                    ui.horizontal(|ui| {
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
                    if small_button_enabled(ui, palette, "Always allow every command", true) {
                        everything = true;
                    }
                    for (index, scope) in scopes.iter().enumerate() {
                        let label = if scope.word_count == 0 {
                            format!("Always allow exactly: {}", scope.text)
                        } else {
                            format!("Always allow commands starting with: {}", scope.text)
                        };
                        if small_button_enabled(ui, palette, &label, true) {
                            chosen = Some((true, Some(index)));
                        }
                    }
                    if let Some((allow, remember)) = chosen {
                        approval = Some((allow, remember, false));
                    } else if everything {
                        approval = Some((true, None, true));
                    }
                }

                // The question card.
                if let Some(Phase::Asking { text, .. }) = &self.agent.phase {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(text.as_str())
                            .size(12.0)
                            .strong()
                            .color(to_color32(palette.text)),
                    );
                    let room = (ui.available_width() - 8.0).max(80.0);
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(room, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(room);
                            let response = text_area(ui, palette, &mut self.answer, 2, "Your answer…");
                            if response.has_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                                && !ui.input(|input| input.modifiers.shift)
                            {
                                answer = Some(std::mem::take(&mut self.answer));
                            }
                        },
                    );
                    if small_button_enabled(ui, palette, "Send answer", !self.answer.trim().is_empty()) {
                        answer = Some(std::mem::take(&mut self.answer));
                    }
                }

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

        if let Some((allow, remember, everything)) = approval {
            if everything {
                self.agent.approve_everything(&mut session);
            } else {
                self.agent.approve(allow, remember, &mut session);
            }
        }
        if let Some(answer) = answer {
            self.agent.answer(&answer, &mut session);
        }
        if sudo_retry {
            let password = std::mem::take(&mut self.sudo);
            self.agent.retry_with_sudo(&password, &mut session);
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
        let slash = parse_slash_input(&self.draft);
        if let Some(slash) = &slash
            && !slash.matches.is_empty()
        {
            for command in &slash.matches {
                ui.label(
                    egui::RichText::new(format!("{} — {}", command.label, command.description))
                        .size(11.0)
                        .color(muted),
                );
            }
        }

        // Composer.
        ui.separator();
        let mut submit = false;
        let mut command: Option<SlashCommand> = None;
        ui.horizontal(|ui| {
            // The attach buttons stack on the left, like the reference composer.
            ui.vertical(|ui| {
                if small_button_enabled(
                    ui,
                    palette,
                    ATTACH_GLYPH,
                    self.picking.is_none() && self.attachments.len() < MAX_ATTACHMENTS,
                ) {
                    self.pick_attachments();
                }
                if chip(ui, palette, TERMINAL_ATTACH_GLYPH, self.attach_terminal) {
                    self.attach_terminal = !self.attach_terminal;
                }
            });
            let room = (ui.available_width() - SEND_BUTTON_WIDTH).max(MIN_COMPOSER_WIDTH);
            ui.allocate_ui_with_layout(
                egui::Vec2::new(room, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(room);
                    let response = text_area(ui, palette, &mut self.draft, 2, "Ask the agent…");
                    if response.has_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                        && !ui.input(|input| input.modifiers.shift)
                    {
                        submit = true;
                    }
                },
            );
            if busy {
                if Button::new("Stop")
                    .compact()
                    .kind(ButtonKind::Danger)
                    .show(ui, palette)
                    .clicked()
                {
                    self.agent.stop();
                }
            } else if small_button_enabled(
                ui,
                palette,
                "Send",
                !self.providers.is_empty() && !self.draft.trim().is_empty(),
            ) {
                submit = true;
            }
        });

        // Provider and model under the composer, like the reference footer.
        let footer_room = ((ui.available_width() - FOOTER_GAP) / 2.0).max(FOOTER_SELECT_MIN_WIDTH);
        if self.providers.is_empty() {
            ui.label(
                egui::RichText::new("No model providers configured — add one in Options.")
                    .size(FOOTER_TEXT_SIZE)
                    .color(muted),
            );
        }
        let (chosen_provider, chosen_model) = {
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
            if self.providers.is_empty() {
                (None, None)
            } else {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = FOOTER_GAP;
                    // The reference footer is set in the same small type as the rest.
                    ui.style_mut().text_styles.insert(
                        egui::TextStyle::Button,
                        egui::FontId::proportional(FOOTER_TEXT_SIZE),
                    );
                    let provider =
                        select_width(ui, "ai-provider", &self.provider, &provider_options, footer_room);
                    let model = select_width(ui, "ai-model", &self.model, &model_options, footer_room);
                    (provider, model)
                })
                .inner
            }
        };
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

        if submit && !busy && let Some(slash) = &slash && let Some(exact) = slash.exact {
            command = Some(exact);
        }
        if let Some(command) = command {
            self.run_command(command, context.settings, &context.host_label);
        } else if submit && !busy && !self.draft.trim().is_empty() {
            // The toggle attaches the live terminal screen to this one message.
            let terminal = self
                .attach_terminal
                .then(|| {
                    session
                        .as_ref()
                        .map(|access| access.screen_text())
                        .unwrap_or_default()
                });
            let message = self.message_with_attachments(self.draft.trim(), terminal.as_deref());
            if self.agent.push_user(&message) {
                self.draft.clear();
                self.attachments.clear();
                self.attach_terminal = false;
                let request = self.request(context.settings, &context.host_label);
                if let Some(request) = request {
                    self.agent.start(request);
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
        let _ = PROVIDER_OPENROUTER;
    }
}

#[cfg(test)]
mod tests {
    /// No secret settings in these tests: panes read them through the context.
    static EMPTY_SECRETS: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

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
                status: SessionStatus::Connected,
                settings,
                scope: "host-1".to_owned(),
                session: None,
                events: &[],
                pending_input: Vec::new(),
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
            texts.contains(&"Ask anything about this session."),
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
            Some((DEFAULT_CHAT_TITLE.to_owned(), HeaderHealth::Ok))
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
            Some(("how much disk is left".to_owned(), HeaderHealth::Ok))
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
        assert!(texts.contains(&"First"), "provider dropdown missing: {texts:?}");
        assert!(texts.contains(&"a"), "model dropdown missing: {texts:?}");
        assert_eq!(plugin.provider, 0);
        assert_eq!(plugin.model, "a");
    }

    #[test]
    fn tool_runs_are_shown_with_their_output() {
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
        assert!(texts.contains(&"up 3 days"), "output missing: {texts:?}");
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
        assert!(texts.contains(&"Run this command?"), "card missing: {texts:?}");
        assert!(
            texts.contains(&"systemctl restart nginx"),
            "command missing: {texts:?}"
        );
        assert!(texts.contains(&"Allow"), "allow missing: {texts:?}");
        assert!(texts.contains(&"Deny"), "deny missing: {texts:?}");
        assert!(
            texts.contains(&"Always allow every command"),
            "allow-all missing: {texts:?}"
        );
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
            text: "Which file?".to_owned(),
        });
        plugin.agent.turns.push(Turn::Question {
            text: "Which file?".to_owned(),
            answer: None,
        });
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"Which file?"), "question missing: {texts:?}");
        assert!(texts.contains(&"Your answer…"), "answer box missing: {texts:?}");
        assert!(texts.contains(&"Send answer"), "send missing: {texts:?}");
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
        assert!(parse_slash_input("/nonexistent").unwrap().matches.is_empty());
    }

    #[test]
    fn attachments_and_the_terminal_tail_are_folded_into_the_message() {
        let mut plugin = AiAgent::default();
        plugin.attachments.push(Attachment {
            path: "/tmp/notes.txt".to_owned(),
            text: "line one".to_owned(),
        });
        let message = plugin.message_with_attachments("summarise this", Some("$ uptime\nup 3 days"));
        assert!(message.starts_with("[Attached file notes.txt]\nline one"), "{message}");
        assert!(message.contains("summarise this"), "{message}");
        assert!(
            message.ends_with("[Recent terminal output]\n$ uptime\nup 3 days"),
            "terminal context missing: {message}"
        );
        assert!(plugin.message_with_attachments("plain", None).contains("Attached"));

        // A blank terminal still says so, like the reference.
        let empty = plugin.message_with_attachments("plain", Some("  "));
        assert!(empty.ends_with("[Recent terminal output]\n[No terminal output captured yet.]"));
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
        assert_eq!(plugin.providers[0].models, ["A", "c"], "sorted, case-insensitive");
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
    fn a_long_question_is_cut_short_in_the_header() {
        let mut plugin = AiAgent::default();
        plugin.agent.turns.push(Turn::User("x".repeat(200)));
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
        plugin.agent.turns.push(Turn::User("first question".to_owned()));
        plugin.agent.history.push(super::super::protocol::ApiMessage::User {
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
            plugin
                .agent
                .turns
                .push(Turn::User(format!("question {index}")));
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
        assert_eq!(settings[CONVERSATION_KEY]["chats"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_refused_sudo_password_is_asked_for_in_the_pane() {
        let mut plugin = AiAgent::default();
        plugin.agent.sudo_waiting = true;
        plugin.agent.last_command = Some("apt-get update".to_owned());
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"sudo password"), "password row missing: {texts:?}");
        assert!(texts.contains(&"Run with sudo"), "retry button missing: {texts:?}");
    }

    #[test]
    fn the_web_and_knowledge_settings_reach_the_tools() {
        let mut plugin = AiAgent::default();
        let mut settings = serde_json::Map::new();
        settings.insert(
            SETTING_WEB_SEARCH_PROVIDER.to_owned(),
            json!("duckduckgo"),
        );
        // A key left in the settings map is ignored: the vault is the only
        // place a secret is read from.
        settings.insert(SETTING_WEB_SEARCH_KEY.to_owned(), json!("plain-text"));
        settings.insert(SETTING_RAG_FOLDER.to_owned(), json!("/docs"));
        settings.insert(SETTING_HOST_RAG_FOLDER.to_owned(), json!("/srv/docs"));
        let secrets = [(
            SETTING_WEB_SEARCH_KEY.to_owned(),
            "secret".to_owned(),
        )]
        .into_iter()
        .collect();
        draw_with_secrets(&mut plugin, &mut settings, &secrets);
        assert_eq!(plugin.agent.tools.web_provider.as_deref(), Some("duckduckgo"));
        assert_eq!(plugin.agent.tools.web_key.as_deref(), Some("secret"));
        assert_eq!(plugin.agent.tools.rag_folders, ["/docs", "/srv/docs"]);
        assert!(plugin.agent.tools.embed.is_none(), "no embedding model configured");
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
