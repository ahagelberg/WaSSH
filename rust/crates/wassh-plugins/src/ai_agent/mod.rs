//! The AI agent plugin.
//!
//! A chat with an LLM that can act on the session: the provider layer mirrors
//! `src/plugins/builtins/ai-agent/providers.ts`, the rules mirror
//! `permissions.ts`, and the pane mirrors the reference view.

mod agent;
mod markdown;
mod rag;
mod protocol;
mod providers;
mod rules;
mod settings_ui;
mod tools;
mod web;
mod view;

pub use protocol::{
    ApiMessage, ApiToolCall, Completion, PROVIDER_CUSTOM, PROVIDER_TYPES, Protocol, ProviderConfig,
    ProviderType, join_url, DATA_VERSION, DEFAULT_CHAT_TITLE, SAFE_RULES,
    SETTING_DEFAULT_ALLOW_RULES, SETTING_DEFAULT_DENY_RULES, SETTING_GLOBAL_PROMPT,
    SETTING_HOST_ALLOW_RULES,
    SETTING_HOST_DENY_RULES, SETTING_HOST_PROMPT, TITLE_MAX_CHARS,
};
pub use providers::{provider_models, CompletionRequest, EmbedConfig, StreamEvent};
pub use agent::{Agent, Phase, ToolSettings, Turn, Waiting};
pub use rules::{approval_scopes, decide_command, ApprovalScope, RuleDecision};
pub use settings_ui::{
    API_KEYS_KEY, PROVIDERS_KEY, ProviderEditor, is_provider_key_field, provider_ids,
    provider_secret_field,
};
pub use view::AiAgent;
