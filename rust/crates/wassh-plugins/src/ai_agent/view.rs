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
    Button, ButtonKind, select_str, small_button_enabled, text_area, text_input,
};

use super::agent::{
    history_from_json, history_to_json, turns_from_json, turns_to_json, Agent, Phase, ToolSettings,
    Turn, SUMMARY_SYSTEM_PROMPT,
};
use super::protocol::{
    default_providers, Protocol, ProviderConfig, CONVERSATIONS_PER_HOST_MAX, DATA_VERSION,
    DEFAULT_CHAT_TITLE, MAX_ATTACHMENTS,
    MAX_ATTACHMENT_CHARS, MAX_ATTACHMENT_BYTES, PROVIDER_OLLAMA, PROVIDER_OPENROUTER, SAFE_RULES,
    SETTING_DEFAULT_ALLOW_RULES, SETTING_DEFAULT_DENY_RULES, SETTING_GLOBAL_PROMPT,
    SETTING_HOST_ALLOW_RULES, SETTING_HOST_DENY_RULES, SETTING_HOST_PROMPT, SETTING_HOST_RAG_FOLDER,
    SETTING_RAG_FOLDER, SETTING_RAG_MODEL, SETTING_RAG_PROVIDER, SETTING_WEB_SEARCH_KEY,
    SETTING_WEB_SEARCH_PROVIDER, TITLE_MAX_CHARS,
};
use super::providers::{complete_blocking, CompletionRequest, EmbedConfig};
use super::tools;
use crate::color::to_color32;
use crate::plugin::{Plugin, PluginContext};
use crate::{PluginId, PluginManifest};

/// Key the conversation is stored under.
const CONVERSATION_KEY: &str = "conversation";
/// Key the provider list is stored under.
const PROVIDERS_KEY: &str = "providers";
/// Key the typed API keys are stored under.
const API_KEYS_KEY: &str = "apiKeys";
/// Tokens a reply may use.
const MAX_TOKENS: u32 = 4096;
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

/// A model-list fetch: the provider it belongs to and its answer.
type ModelFetch = (usize, Receiver<Result<Vec<String>, String>>);

/// What a summary worker is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SummaryMode {
    /// Show the summary here.
    Summary,
    /// Show it, then start a new chat carrying it.
    Transfer,
}

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
    /// A file picker running on its own thread.
    picking: Option<Receiver<Vec<String>>>,
    /// A model-list fetch running on its own thread.
    fetching: Option<ModelFetch>,
    /// A summary running on its own thread.
    summary: Option<(SummaryMode, Receiver<Result<String, String>>)>,
    /// The question composer, while `ask_user` waits.
    answer: String,
    /// The sudo password field, after a refused command.
    sudo: String,
    /// Whether the provider editor is open.
    editing: bool,
    /// Whether the stored conversation has been read back.
    loaded: bool,
    /// Caption and health for the pane header.
    caption: (String, HeaderHealth),
    /// Last problem reported to the user.
    error: Option<String>,
}

impl Default for AiAgent {
    fn default() -> Self {
        let providers = default_providers();
        let model = providers
            .first()
            .and_then(|provider| provider.models.first().cloned())
            .unwrap_or_default();
        Self {
            agent: Agent::new(),
            chats: Vec::new(),
            providers,
            provider: 0,
            model,
            keys: std::collections::BTreeMap::new(),
            draft: String::new(),
            attachments: Vec::new(),
            picking: None,
            fetching: None,
            summary: None,
            answer: String::new(),
            sudo: String::new(),
            editing: false,
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

    /// Read the stored conversation and providers back in.
    fn load(&mut self, settings: &serde_json::Map<String, Value>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Some(providers) = settings.get(PROVIDERS_KEY)
            && let Ok(parsed) = serde_json::from_value::<Vec<ProviderConfig>>(providers.clone())
            && !parsed.is_empty()
        {
            self.providers = parsed;
        }
        if let Some(keys) = settings.get(API_KEYS_KEY)
            && let Ok(parsed) =
                serde_json::from_value::<std::collections::BTreeMap<String, String>>(keys.clone())
        {
            self.keys = parsed;
        }
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

    /// Ask a provider for its model list on a worker thread.
    fn fetch_models(&mut self, index: usize) {
        if self.fetching.is_some() {
            return;
        }
        let Some(provider) = self.providers.get(index).cloned() else {
            return;
        };
        let key = self.keys.get(&provider.id).cloned().unwrap_or_default();
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(super::providers::provider_models(&provider, &key));
        });
        self.fetching = Some((index, events));
    }

    /// Collect a finished model list.
    fn poll_models(&mut self) {
        let Some((index, events)) = &self.fetching else {
            return;
        };
        match events.try_recv() {
            Ok(result) => {
                let index = *index;
                self.fetching = None;
                match result {
                    Ok(models) if !models.is_empty() => {
                        if let Some(provider) = self.providers.get_mut(index) {
                            provider.models = models;
                        }
                        self.error = None;
                    }
                    Ok(_) => self.error = Some("The provider listed no models.".to_owned()),
                    Err(error) => self.error = Some(error),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.fetching = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
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

    /// The user message with attachments folded in.
    fn message_with_attachments(&self, text: &str) -> String {
        if self.attachments.is_empty() {
            return text.to_owned();
        }
        let mut message = String::new();
        for attachment in &self.attachments {
            message.push_str(&format!(
                "[Attached file {}]\n{}\n\n",
                attachment.name(),
                attachment.text
            ));
        }
        message.push_str(text);
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

    /// Paint the provider editor.
    fn provider_editor(&mut self, ui: &mut Ui, palette: &wassh_core::Palette, muted: egui::Color32) {
        egui::ScrollArea::vertical()
            .id_salt("ai-providers")
            .max_height(180.0)
            .show(ui, |ui| {
                let mut remove = None;
                let mut started = None;
                let count = self.providers.len();
                for (index, provider) in self.providers.iter_mut().enumerate() {
                    let mut fetch = false;
                    ui.horizontal(|ui| {
                        let room = (ui.available_width() - 150.0).max(90.0);
                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(room, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_width(room);
                                text_input(ui, palette, &mut provider.name, "name");
                            },
                        );
                        if small_button_enabled(ui, palette, "Fetch", true) {
                            fetch = true;
                        }
                        if small_button_enabled(ui, palette, "Remove", count > 1) {
                            remove = Some(index);
                        }
                    });
                    if fetch {
                        started = Some(index);
                    }
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("URL").size(10.5).color(muted));
                        let room = (ui.available_width() - 8.0).max(80.0);
                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(room, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_width(room);
                                text_input(ui, palette, &mut provider.base_url, "https://host/v1");
                            },
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Models").size(10.5).color(muted));
                        let mut models = provider.models.join(", ");
                        let room = (ui.available_width() - 8.0).max(80.0);
                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(room, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_width(room);
                                if text_input(ui, palette, &mut models, "model-a, model-b").changed() {
                                    provider.models = models
                                        .split(',')
                                        .map(|model| model.trim().to_owned())
                                        .filter(|model| !model.is_empty())
                                        .collect();
                                }
                            },
                        );
                    });
                    ui.add_space(4.0);
                }
                if let Some(index) = remove {
                    self.providers.remove(index);
                    self.provider = self.provider.min(self.providers.len().saturating_sub(1));
                }
                if let Some(index) = started {
                    self.fetch_models(index);
                }
                if small_button_enabled(ui, palette, "Add provider", true) {
                    self.providers.push(ProviderConfig {
                        id: format!("custom-{}", self.providers.len() + 1),
                        name: "Custom provider".to_owned(),
                        protocol: Protocol::Openai,
                        base_url: "https://api.example.com/v1".to_owned(),
                        models: Vec::new(),
                        embedding_model: None,
                    });
                }
            });
    }
}

impl Plugin for AiAgent {
    fn manifest(&self) -> PluginManifest {
        PluginId::AiAgent.manifest()
    }

    fn header_status(&self) -> Option<(String, HeaderHealth)> {
        (!self.caption.0.is_empty()).then(|| self.caption.clone())
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = &context.palette;
        let muted = to_color32(palette.text_muted);
        // Taken out so the borrow of one field does not block the rest.
        let mut session = context.session.take();
        self.load(context.settings);
        self.apply_settings(context.settings, context.secret(SETTING_WEB_SEARCH_KEY));
        self.agent.now_ms = now_ms();
        self.poll_summary();
        self.poll_picker();
        self.poll_models();

        // Drain workers, then answer any tool result it produced.
        self.agent.poll(&mut session);
        if self.agent.take_needs_reply() {
            let request = self.request(context.settings, &context.host_label);
            if let Some(request) = request {
                self.agent.start(request);
            }
        }
        if self.agent.busy() || self.summary.is_some() || self.picking.is_some() {
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

        // Provider, model and the pane controls.
        ui.horizontal(|ui| {
            let labels: Vec<&'static str> = self
                .providers
                .iter()
                .map(|provider| &*Box::leak(provider.name.clone().into_boxed_str()))
                .collect();
            let current = self
                .active()
                .map(|provider| provider.name.clone())
                .unwrap_or_default();
            if let Some(chosen) = select_str(ui, "ai-provider", &current, &labels)
                && let Some(index) = self.providers.iter().position(|item| item.name == chosen)
            {
                self.provider = index;
                if let Some(model) = self.providers[index].models.first() {
                    self.model.clone_from(model);
                }
            }
            let room = (ui.available_width() - 96.0).max(120.0);
            ui.allocate_ui_with_layout(
                egui::Vec2::new(room, 0.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_width(room);
                    text_input(ui, palette, &mut self.model, "model");
                },
            );
            if small_button_enabled(ui, palette, "New chat", !busy && !self.agent.turns.is_empty()) {
                self.begin_new_chat();
            }
        });

        // Previous conversations.
        if !self.chats.is_empty() {
            ui.horizontal(|ui| {
                let labels: Vec<&'static str> = self
                    .chats
                    .iter()
                    .map(|chat| &*Box::leak(chat.title.clone().into_boxed_str()))
                    .collect();
                ui.label(egui::RichText::new("Chats").size(11.0).color(muted));
                if let Some(chosen) = select_str(ui, "ai-chats", "", &labels)
                    && let Some(index) = self.chats.iter().position(|chat| chat.title == chosen)
                {
                    self.open_chat(index);
                }
            });
        }

        // The provider's key, when it needs one.
        let provider_id = self.active().map(|provider| provider.id.clone());
        if let Some(provider_id) = provider_id.filter(|id| id != PROVIDER_OLLAMA) {
            let mut key = self.keys.get(&provider_id).cloned().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("API key").size(11.0).color(muted));
                let room = (ui.available_width() - 100.0).max(80.0);
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(room, 0.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_width(room);
                        if text_input(ui, palette, &mut key, "paste the provider key").changed() {
                            self.keys.insert(provider_id.clone(), key.clone());
                        }
                    },
                );
                if small_button_enabled(ui, palette, "Providers", true) {
                    self.editing = !self.editing;
                }
            });
        } else if small_button_enabled(ui, palette, "Providers", true) {
            self.editing = !self.editing;
        }
        if self.editing {
            self.provider_editor(ui, palette, muted);
        }

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
        let composer_height = if self.agent.asking() { 150.0 } else { 116.0 };
        let height = (ui.available_height() - composer_height).max(80.0);
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
            if busy {
                if Button::new("Stop")
                    .compact()
                    .kind(ButtonKind::Danger)
                    .show(ui, palette)
                    .clicked()
                {
                    self.agent.stop();
                }
            } else if small_button_enabled(ui, palette, "Send", !self.draft.trim().is_empty()) {
                submit = true;
            }
            if small_button_enabled(
                ui,
                palette,
                &format!("Attach ({})", self.attachments.len()),
                self.attachments.len() < MAX_ATTACHMENTS,
            ) {
                self.pick_attachments();
            }
            let room = (ui.available_width() - 8.0).max(80.0);
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
        });
        if submit && !busy && let Some(slash) = &slash && let Some(exact) = slash.exact {
            command = Some(exact);
        }
        if let Some(command) = command {
            self.run_command(command, context.settings, &context.host_label);
        } else if submit && !busy && !self.draft.trim().is_empty() {
            let message = self.message_with_attachments(self.draft.trim());
            if self.agent.push_user(&message) {
                self.draft.clear();
                self.attachments.clear();
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
        context.set_value(
            PROVIDERS_KEY,
            serde_json::to_value(&self.providers).unwrap_or(Value::Null),
        );
        context.set_value(
            API_KEYS_KEY,
            serde_json::to_value(&self.keys).unwrap_or(Value::Null),
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
        let painted = draw(&mut plugin, &mut settings);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"Ollama"), "provider picker missing: {texts:?}");
        assert!(
            texts.contains(&"Ask anything about this session."),
            "empty hint missing: {texts:?}"
        );
        assert!(texts.contains(&"Send"), "composer missing: {texts:?}");
        assert!(texts.contains(&"Attach (0)"), "attach button missing: {texts:?}");
        assert_eq!(
            plugin.header_status(),
            Some((DEFAULT_CHAT_TITLE.to_owned(), HeaderHealth::Ok))
        );
    }

    #[test]
    fn a_stored_conversation_comes_back_and_the_title_follows_it() {
        let mut settings = serde_json::Map::new();
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
        assert!(texts.contains(&"llama3.1"), "stored model missing: {texts:?}");
        assert!(
            texts.contains(&"how much disk is left"),
            "stored question missing: {texts:?}"
        );
        assert_eq!(plugin.agent.cwd, "/srv/app");
        assert_eq!(plugin.agent.history.len(), 2);
        assert_eq!(
            plugin.header_status(),
            Some(("how much disk is left".to_owned(), HeaderHealth::Ok))
        );
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
    fn attachments_are_folded_into_the_message() {
        let mut plugin = AiAgent::default();
        plugin.attachments.push(Attachment {
            path: "/tmp/notes.txt".to_owned(),
            text: "line one".to_owned(),
        });
        let message = plugin.message_with_attachments("summarise this");
        assert!(message.starts_with("[Attached file notes.txt]\nline one"), "{message}");
        assert!(message.ends_with("summarise this"), "{message}");
        assert!(plugin.message_with_attachments("plain").contains("Attached"));
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
        assert!(settings.contains_key(PROVIDERS_KEY));
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
