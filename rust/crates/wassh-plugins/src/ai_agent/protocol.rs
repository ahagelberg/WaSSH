//! Shared vocabulary of the AI agent plugin.
//!
//! Mirrors `src/plugins/builtins/ai-agent/protocol.ts` and `defaults.ts`: the
//! same setting keys, tool names, provider defaults and limits, so a stored
//! settings file means the same thing to both applications.

// The reference names every tool and setting up front; tools that are not
// implemented yet keep their constants here so the port stays traceable.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Setting key: the text included in every prompt.
pub const SETTING_GLOBAL_PROMPT: &str = "globalPrompt";
/// Setting key: app-wide command patterns that run without asking.
pub const SETTING_DEFAULT_ALLOW_RULES: &str = "defaultAllowRules";
/// Setting key: app-wide command patterns that are always blocked.
pub const SETTING_DEFAULT_DENY_RULES: &str = "defaultDenyRules";
/// Setting key: this host's prompt text.
pub const SETTING_HOST_PROMPT: &str = "hostPrompt";
/// Setting key: this host's allow patterns.
pub const SETTING_HOST_ALLOW_RULES: &str = "allowRules";
/// Setting key: this host's deny patterns.
pub const SETTING_HOST_DENY_RULES: &str = "denyRules";
/// Setting key: the local knowledge-base folder.
pub const SETTING_RAG_FOLDER: &str = "ragFolder";
/// Setting key: the provider used for embeddings.
pub const SETTING_RAG_PROVIDER: &str = "ragProviderId";
/// Setting key: the embedding model.
pub const SETTING_RAG_MODEL: &str = "ragEmbeddingModel";
/// Setting key: this host's extra knowledge folder.
pub const SETTING_HOST_RAG_FOLDER: &str = "hostRagFolder";
/// Setting key: which search engine `web_search` uses.
pub const SETTING_WEB_SEARCH_PROVIDER: &str = "webSearchProvider";
/// Setting key: the search engine's API key.
pub const SETTING_WEB_SEARCH_KEY: &str = "webSearchApiKey";

/// Tool: run a shell command with output captured.
pub const TOOL_RUN_COMMAND: &str = "run_command";
/// Tool: ask the user a multiple-choice question.
pub const TOOL_ASK_USER: &str = "ask_user";
/// Tool: fetch a URL.
pub const TOOL_WEB_FETCH: &str = "web_fetch";
/// Tool: search the web.
pub const TOOL_WEB_SEARCH: &str = "web_search";
/// Tool: read a remote file.
pub const TOOL_REMOTE_READ: &str = "remote_fs_read_file";
/// Tool: write a remote file.
pub const TOOL_REMOTE_WRITE: &str = "remote_fs_write_file";
/// Tool: list a remote directory.
pub const TOOL_REMOTE_LIST: &str = "remote_fs_list_dir";
/// Tool: delete a remote file.
pub const TOOL_REMOTE_DELETE: &str = "remote_fs_delete_file";
/// Tool: edit a remote file in place.
pub const TOOL_REMOTE_EDIT: &str = "remote_fs_edit_file";
/// Tool: read a local file.
pub const TOOL_LOCAL_READ: &str = "local_fs_read_file";
/// Tool: write a local file.
pub const TOOL_LOCAL_WRITE: &str = "local_fs_write_file";
/// Tool: list a local directory.
pub const TOOL_LOCAL_LIST: &str = "local_fs_list_dir";
/// Tool: edit a local file in place.
pub const TOOL_LOCAL_EDIT: &str = "local_fs_edit_file";
/// Tool: the exact current time.
pub const TOOL_CURRENT_TIME: &str = "get_current_time";
/// Tool: search the knowledge base.
pub const TOOL_RAG_SEARCH: &str = "rag_search";
/// Tool: view a file through the editor API.
pub const TOOL_DEV_VIEW: &str = "dev_view_file";
/// Tool: edit a file through the editor API.
pub const TOOL_DEV_EDIT: &str = "dev_edit_file";
/// Tool: create a file through the editor API.
pub const TOOL_DEV_CREATE: &str = "dev_create_file";
/// Tool: list a directory through the editor API.
pub const TOOL_DEV_LIST: &str = "dev_list_directory";
/// Tool: grep through the editor API.
pub const TOOL_DEV_GREP: &str = "dev_grep_search";
/// Tool: find files through the editor API.
pub const TOOL_DEV_FIND: &str = "dev_find_files";
/// Tool: diff a file through the editor API.
pub const TOOL_DEV_DIFF: &str = "dev_diff_file";
/// Tool: read the terminal screen.
pub const TOOL_TERMINAL_READ: &str = "terminal_read";
/// Tool: type into the terminal.
pub const TOOL_TERMINAL_WRITE: &str = "terminal_write";
/// Tool: send keys to the terminal.
pub const TOOL_TERMINAL_KEYS: &str = "terminal_send_keys";
/// Tool: wait for the terminal to settle.
pub const TOOL_TERMINAL_WAIT: &str = "terminal_wait";
/// Tool: one-call system baseline.
pub const TOOL_SYSTEM_OVERVIEW: &str = "system_overview";
/// Tool: the process list.
pub const TOOL_SYSTEM_PROCESSES: &str = "system_process_list";
/// Tool: disk and inode usage.
pub const TOOL_SYSTEM_DISK: &str = "system_disk_usage";
/// Tool: the memory report.
pub const TOOL_SYSTEM_MEMORY: &str = "system_memory_report";

/// Prefix that marks a rule list entry as a regular expression.
pub const RULE_REGEX_PREFIX: &str = "regex:";

/// Provider protocol: OpenAI-compatible `chat/completions`.
pub const PROTOCOL_OPENAI: &str = "openai";
/// Provider protocol: Anthropic `v1/messages`.
pub const PROTOCOL_ANTHROPIC: &str = "anthropic";
/// Anthropic API version header.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Anthropic chat path.
pub const ANTHROPIC_CHAT_PATH: &str = "/v1/messages";
/// Anthropic model list path.
pub const ANTHROPIC_MODELS_PATH: &str = "/v1/models";
/// OpenAI-compatible chat path.
pub const OPENAI_CHAT_PATH: &str = "/chat/completions";
/// OpenAI-compatible model list path.
pub const OPENAI_MODELS_PATH: &str = "/models";
/// OpenAI-compatible embeddings path.
pub const OPENAI_EMBEDDINGS_PATH: &str = "/embeddings";

/// Version of the plugin's stored data.
pub const DATA_VERSION: u32 = 2;
/// Title a fresh chat gets.
pub const DEFAULT_CHAT_TITLE: &str = "New chat";
/// Chats kept per host.
pub const CONVERSATIONS_PER_HOST_MAX: usize = 40;
/// Longest generated chat title.
pub const TITLE_MAX_CHARS: usize = 48;
/// Files that can be attached to one message.
pub const MAX_ATTACHMENTS: usize = 8;
/// Largest attachment read from disk.
pub const MAX_ATTACHMENT_BYTES: u64 = 200_000;
/// Longest attachment text sent to the model.
pub const MAX_ATTACHMENT_CHARS: usize = 100_000;

/// Provider id of the local Ollama server.
pub const PROVIDER_OLLAMA: &str = "ollama";
/// Provider id of OpenRouter.
pub const PROVIDER_OPENROUTER: &str = "openrouter";
/// Provider id of Anthropic.
pub const PROVIDER_ANTHROPIC: &str = "anthropic";
/// Provider id of DeepSeek.
pub const PROVIDER_DEEPSEEK: &str = "deepseek";
/// Provider id of xAI.
pub const PROVIDER_GROK: &str = "grok";
/// Provider id of OpenAI.
pub const PROVIDER_OPENAI: &str = "openai";
/// Id prefix of a provider the user fills in themselves.
pub const CUSTOM_PROVIDER_PREFIX: &str = "custom-";

/// Which wire format a provider speaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// OpenAI-compatible.
    #[default]
    Openai,
    /// Anthropic messages API.
    Anthropic,
}

impl Protocol {
    /// Parse the stored spelling; unknown values are OpenAI-compatible.
    pub fn parse(text: &str) -> Self {
        match text {
            PROTOCOL_ANTHROPIC => Self::Anthropic,
            _ => Self::Openai,
        }
    }

    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => PROTOCOL_OPENAI,
            Self::Anthropic => PROTOCOL_ANTHROPIC,
        }
    }
}

/// One provider the user can chat through.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Stable id, also the key its API key is stored under.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Wire format.
    #[serde(default)]
    pub protocol: Protocol,
    /// Base URL, without the operation path.
    pub base_url: String,
    /// Models offered in the picker.
    #[serde(default)]
    pub models: Vec<String>,
    /// Model used for embeddings, when this provider can produce them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
}

/// A provider type the add dialog offers.
///
/// Picking one decides the endpoint and the wire format, so the dialog only asks for
/// what that service needs: an API key, and for the services the user hosts himself,
/// a name and an endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderType {
    /// Stable id the provider is stored under; also the vault field of its key.
    pub id: &'static str,
    /// Name shown in the dropdown.
    pub name: &'static str,
    /// Wire format.
    pub protocol: Protocol,
    /// Endpoint, empty when the user supplies it.
    pub base_url: &'static str,
    /// Whether the service needs an API key (a local Ollama server does not).
    pub needs_key: bool,
}

/// Id of the "somewhere else" entry: an endpoint the user describes himself.
pub const PROVIDER_CUSTOM: &str = "custom";

/// The provider types the add dialog lists, in the order it shows them.
pub const PROVIDER_TYPES: &[ProviderType] = &[
    ProviderType {
        id: PROVIDER_OPENAI,
        name: "OpenAI",
        protocol: Protocol::Openai,
        base_url: "https://api.openai.com/v1",
        needs_key: true,
    },
    ProviderType {
        id: PROVIDER_ANTHROPIC,
        name: "Anthropic",
        protocol: Protocol::Anthropic,
        base_url: "https://api.anthropic.com",
        needs_key: true,
    },
    ProviderType {
        id: PROVIDER_DEEPSEEK,
        name: "DeepSeek",
        protocol: Protocol::Openai,
        base_url: "https://api.deepseek.com",
        needs_key: true,
    },
    ProviderType {
        id: PROVIDER_GROK,
        name: "Grok (xAI)",
        protocol: Protocol::Openai,
        base_url: "https://api.x.ai/v1",
        needs_key: true,
    },
    ProviderType {
        id: PROVIDER_OPENROUTER,
        name: "Open Router",
        protocol: Protocol::Openai,
        base_url: "https://openrouter.ai/api/v1",
        needs_key: true,
    },
    ProviderType {
        id: PROVIDER_OLLAMA,
        name: "Ollama (local)",
        protocol: Protocol::Openai,
        base_url: "http://127.0.0.1:11434/v1",
        needs_key: false,
    },
    ProviderType {
        id: PROVIDER_CUSTOM,
        name: "Other (OpenAI or Anthropic compatible)",
        protocol: Protocol::Openai,
        base_url: "",
        needs_key: true,
    },
];

impl ProviderType {
    /// Whether the endpoint is the user's to type, rather than the service's.
    pub fn has_fixed_endpoint(self) -> bool {
        !self.base_url.is_empty() && self.id != PROVIDER_OLLAMA
    }
}

/// Command patterns that run without asking, as the reference ships them.
pub const SAFE_RULES: [&str; 19] = [
    "pwd",
    "whoami",
    "id*",
    "hostname*",
    "uname*",
    "date",
    "uptime",
    "ls*",
    "lscpu*",
    "free*",
    "df*",
    "ps*",
    "git status*",
    "git log*",
    "git diff*",
    "git branch*",
    "git remote*",
    "git show*",
    "git rev-parse*",
];

/// One tool call the model asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApiToolCall {
    /// Provider-side id, echoed back with the result.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Raw JSON arguments.
    pub arguments: String,
}

/// One message in the provider's shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiMessage {
    /// The user's turn.
    User {
        /// Text sent to the model.
        content: String,
    },
    /// The assistant's turn, with the tool calls it asked for.
    Assistant {
        /// Text it wrote.
        content: String,
        /// Tools it wants run.
        tool_calls: Vec<ApiToolCall>,
    },
    /// A tool's answer.
    Tool {
        /// Call being answered.
        tool_call_id: String,
        /// The result text.
        content: String,
    },
}

/// What a completion produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    /// Assistant text.
    pub text: String,
    /// Tools it asked for.
    pub tool_calls: Vec<ApiToolCall>,
    /// Provider stop reason, when it sent one.
    pub stop_reason: Option<String>,
}

/// Join a base URL and an operation path with exactly one slash.
pub fn join_url(base_url: &str, path: &str) -> String {
    format!("{}{path}", base_url.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_add_dialog_offers_the_known_services() {
        let names: Vec<&str> = PROVIDER_TYPES.iter().map(|kind| kind.name).collect();
        assert_eq!(
            names,
            [
                "OpenAI",
                "Anthropic",
                "DeepSeek",
                "Grok (xAI)",
                "Open Router",
                "Ollama (local)",
                "Other (OpenAI or Anthropic compatible)",
            ]
        );
        // Every type knows its endpoint and wire format, except the one the user describes.
        let anthropic = PROVIDER_TYPES
            .iter()
            .find(|kind| kind.id == PROVIDER_ANTHROPIC)
            .expect("anthropic");
        assert_eq!(anthropic.protocol, Protocol::Anthropic);
        assert_eq!(anthropic.base_url, "https://api.anthropic.com");
        assert!(anthropic.needs_key);
        // A local Ollama server has no key; its endpoint stays editable.
        let ollama = PROVIDER_TYPES
            .iter()
            .find(|kind| kind.id == PROVIDER_OLLAMA)
            .expect("ollama");
        assert!(!ollama.needs_key);
        assert!(!ollama.has_fixed_endpoint());
        let custom = PROVIDER_TYPES
            .iter()
            .find(|kind| kind.id == PROVIDER_CUSTOM)
            .expect("custom");
        assert!(custom.base_url.is_empty());
        // Ids are unique, or two types would share one vault field.
        let mut ids: Vec<&str> = PROVIDER_TYPES.iter().map(|kind| kind.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn a_provider_round_trips_through_the_stored_shape() {
        let provider = ProviderConfig {
            id: "ollama".to_owned(),
            name: "Ollama".to_owned(),
            protocol: Protocol::Openai,
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            models: vec!["llama3".to_owned()],
            embedding_model: Some("nomic-embed-text".to_owned()),
        };
        let json = serde_json::to_string(&provider).expect("serialise");
        assert!(json.contains("\"baseUrl\""), "{json}");
        assert!(json.contains("\"embeddingModel\""), "{json}");
        let back: ProviderConfig = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, provider);
    }

    #[test]
    fn urls_join_with_one_slash() {
        assert_eq!(
            join_url("https://api.x.ai/v1/", OPENAI_CHAT_PATH),
            "https://api.x.ai/v1/chat/completions"
        );
        assert_eq!(
            join_url("https://api.anthropic.com", ANTHROPIC_CHAT_PATH),
            "https://api.anthropic.com/v1/messages"
        );
    }
}
