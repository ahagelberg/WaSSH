//! The AI agent plugin.
//!
//! A chat with an LLM that can act on the session: the provider layer mirrors
//! `src/plugins/builtins/ai-agent/providers.ts`, the rules mirror
//! `permissions.ts`, and the pane mirrors the reference view.

mod agent;
mod markdown;
mod permissions;
mod protocol;
mod providers;
mod rag;
mod rules;
mod settings_ui;
mod tools;
mod view;
mod web;

pub use agent::{Agent, Phase, ToolSettings, Turn, Waiting};
pub use permissions::{GATES, GROUPS, Permission, Permissions, group_key, setting_key};
pub use protocol::{
    ApiMessage, ApiToolCall, Completion, DATA_VERSION, DEFAULT_CHAT_TITLE, PROVIDER_CUSTOM,
    PROVIDER_TYPES, Protocol, ProviderConfig, ProviderType, SAFE_RULES,
    SETTING_DEFAULT_ALLOW_RULES, SETTING_DEFAULT_DENY_RULES, SETTING_GLOBAL_PROMPT,
    SETTING_HOST_ALLOW_RULES, SETTING_HOST_DENY_RULES, SETTING_HOST_PROMPT, TITLE_MAX_CHARS,
    join_url,
};
pub use providers::{CompletionRequest, EmbedConfig, StreamEvent, provider_models};
pub use rules::{ApprovalScope, RuleDecision, approval_scopes, decide_command};
pub use settings_ui::{
    API_KEYS_KEY, PROVIDER_DIALOG_WIDTH, PROVIDERS_KEY, ProviderEditor, is_provider_key_field,
    provider_ids, provider_secret_field,
};
pub use view::AiAgent;
