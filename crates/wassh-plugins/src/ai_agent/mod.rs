//! The AI agent plugin.
//!
//! A chat with an LLM that can act on the session: the provider layer mirrors
//! `src/plugins/builtins/ai-agent/providers.ts`, the rules mirror
//! `permissions.ts`, and the pane mirrors the reference view.

mod agent;
mod callers;
pub(crate) mod map;
pub(crate) mod markdown;
mod permissions;
mod protocol;
mod providers;
mod rag;
mod rules;
mod scopes;
mod script;
mod settings_ui;
mod tools;
mod view;
mod web;

pub use agent::{
    Agent, CommandKeys, Phase, ScriptRow, SpawnRequest, Subagent, SubagentEnd, SubagentState,
    ToolSettings, Turn, Waiting,
};
pub use callers::{Caller, CallerMode, Callers};
pub use permissions::{GATES, GROUPS, Permission, Permissions, group_key, setting_key};
pub use protocol::{
    ApiMessage, ApiToolCall, Completion, DATA_VERSION, DEFAULT_CHAT_TITLE, PROVIDER_CUSTOM,
    PROVIDER_TYPES, Protocol, ProviderConfig, ProviderType, SAFE_RULES, SETTING_DEFAULT_ALLOW_RULES,
    SETTING_DEFAULT_DENY_RULES, SETTING_GLOBAL_PROMPT, SETTING_HOST_ALLOW_RULES,
    SETTING_HOST_DENY_RULES, SETTING_HOST_PROMPT, SETTING_HOST_SCRIPT_MODE,
    SETTING_HOST_TOOL_CALLER_DEFAULT, SETTING_SCRIPT_MODE, SETTING_TOOL_CALLERS,
    SETTING_TOOL_CALLER_DEFAULT, TITLE_MAX_CHARS, TOOL_RUN_SCRIPT, Usage, join_url,
};
pub use providers::{CompletionRequest, EmbedConfig, StreamEvent, provider_models};
pub use rules::{ApprovalScope, RuleDecision, approval_scopes, decide_command};
pub use settings_ui::{
    API_KEYS_KEY, PROVIDERS_KEY, ProviderEditor, is_provider_key_field, provider_ids,
    provider_secret_field,
};
pub use view::AiAgent;
