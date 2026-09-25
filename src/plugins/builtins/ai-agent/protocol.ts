export const AI_AGENT_SETTING_DEFAULT_ALLOW_RULES = 'defaultAllowRules'
export const AI_AGENT_SETTING_DEFAULT_DENY_RULES = 'defaultDenyRules'
export const AI_AGENT_SETTING_GLOBAL_PROMPT = 'globalPrompt'
export const AI_AGENT_SETTING_HOST_ALLOW_RULES = 'allowRules'
export const AI_AGENT_SETTING_HOST_DENY_RULES = 'denyRules'
export const AI_AGENT_SETTING_HOST_PROMPT = 'hostPrompt'

export const AI_AGENT_SETTING_RAG_FOLDER = 'ragFolder'
export const AI_AGENT_SETTING_RAG_PROVIDER_ID = 'ragProviderId'
export const AI_AGENT_SETTING_RAG_EMBEDDING_MODEL = 'ragEmbeddingModel'
export const AI_AGENT_SETTING_HOST_RAG_FOLDER = 'hostRagFolder'

export const AI_AGENT_SETTING_WEB_SEARCH_PROVIDER = 'webSearchProvider'
export const AI_AGENT_SETTING_WEB_SEARCH_API_KEY = 'webSearchApiKey'

export type AiAgentWebSearchProvider = 'bing' | 'brave' | 'google' | 'duckduckgo' | 'custom'

export const AI_AGENT_TOOL_RUN_COMMAND = 'run_command'
export const AI_AGENT_TOOL_WEB_FETCH = 'web_fetch'
export const AI_AGENT_TOOL_WEB_SEARCH = 'web_search'
export const AI_AGENT_TOOL_REMOTE_FS_READ = 'remote_fs_read_file'
export const AI_AGENT_TOOL_REMOTE_FS_WRITE = 'remote_fs_write_file'
export const AI_AGENT_TOOL_REMOTE_FS_LIST = 'remote_fs_list_dir'
export const AI_AGENT_TOOL_REMOTE_FS_DELETE = 'remote_fs_delete_file'
export const AI_AGENT_TOOL_REMOTE_FS_EDIT = 'remote_fs_edit_file'
export const AI_AGENT_TOOL_LOCAL_FS_READ = 'local_fs_read_file'
export const AI_AGENT_TOOL_LOCAL_FS_WRITE = 'local_fs_write_file'
export const AI_AGENT_TOOL_LOCAL_FS_LIST = 'local_fs_list_dir'
export const AI_AGENT_TOOL_LOCAL_FS_EDIT = 'local_fs_edit_file'
export const AI_AGENT_TOOL_GET_CURRENT_TIME = 'get_current_time'
export const AI_AGENT_TOOL_RAG_SEARCH = 'rag_search'
export const AI_AGENT_TOOL_DEV_VIEW_FILE = 'dev_view_file'
export const AI_AGENT_TOOL_DEV_EDIT_FILE = 'dev_edit_file'
export const AI_AGENT_TOOL_DEV_CREATE_FILE = 'dev_create_file'
export const AI_AGENT_TOOL_DEV_LIST_DIR = 'dev_list_directory'
export const AI_AGENT_TOOL_DEV_GREP = 'dev_grep_search'
export const AI_AGENT_TOOL_DEV_FIND_FILES = 'dev_find_files'
export const AI_AGENT_TOOL_DEV_DIFF_FILE = 'dev_diff_file'
export const AI_AGENT_TOOL_TERMINAL_READ = 'terminal_read'
export const AI_AGENT_TOOL_TERMINAL_WRITE = 'terminal_write'
export const AI_AGENT_TOOL_TERMINAL_KEYS = 'terminal_send_keys'
export const AI_AGENT_TOOL_TERMINAL_WAIT = 'terminal_wait'

export const AI_AGENT_RULE_REGEX_PREFIX = 'regex:'

export const AI_AGENT_PROTOCOL_OPENAI = 'openai'
export const AI_AGENT_PROTOCOL_ANTHROPIC = 'anthropic'
export type AiAgentProviderProtocol =
  | typeof AI_AGENT_PROTOCOL_OPENAI
  | typeof AI_AGENT_PROTOCOL_ANTHROPIC

export const AI_AGENT_ANTHROPIC_VERSION = '2023-06-01'
export const AI_AGENT_ANTHROPIC_PATH = '/v1/messages'
export const AI_AGENT_OPENAI_CHAT_PATH = '/chat/completions'
export const AI_AGENT_OPENAI_MODELS_PATH = '/models'
export const AI_AGENT_OPENAI_EMBEDDINGS_PATH = '/embeddings'
export const AI_AGENT_ANTHROPIC_MODELS_PATH = '/v1/models'

export interface AiAgentProviderConfig {
  id: string
  name: string
  protocol: AiAgentProviderProtocol
  baseUrl: string
  models: string[]
  /** Model used for RAG embeddings via this provider's OpenAI-compatible /embeddings endpoint. */
  embeddingModel?: string
}

export interface AiAgentDataFile {
  version: number
  providers: AiAgentProviderConfig[]
  conversations: Record<string, AiAgentConversation>
  activeConversationId: Record<string, string>
  rules: string
}

export interface AiAgentToolCall {
  id: string
  name?: string
  command: string
  argumentsJson?: string
}

export type AiAgentToolOutcome = 'ok' | 'denied' | 'error' | 'timeout' | 'cancelled'

export interface AiAgentConversationUserMsg {
  role: 'user'
  text: string
  modelText?: string
  usedTerminalContext: boolean
  attachedFiles?: string[]
}

export interface AiAgentConversationAssistantMsg {
  role: 'assistant'
  text?: string
  toolCalls?: AiAgentToolCall[]
  providerId?: string
  model?: string
  stopped?: boolean
}

export interface AiAgentConversationToolMsg {
  role: 'tool'
  toolCallId: string
  name?: string
  command: string
  content: string
  outcome: AiAgentToolOutcome
  truncated: boolean
}

export type AiAgentConversationMsg =
  | AiAgentConversationUserMsg
  | AiAgentConversationAssistantMsg
  | AiAgentConversationToolMsg

export interface AiAgentConversation {
  id: string
  hostKey: string
  title: string
  updatedAt: number
  version: number
  activeProviderId: string
  activeModel: string
  lastSelectedModelByProvider: Record<string, string>
  hostLabel: string
  cwd: string
  messages: AiAgentConversationMsg[]
}

export interface AiAgentConversationSummary {
  id: string
  title: string
  updatedAt: number
}

export type AiAgentRunPhase = 'no_session' | 'idle' | 'running' | 'ask' | 'ask_sudo' | 'paused'

/** 'command' = run_command's own allow/deny rule-list gate; 'permission' = a trinary tool permission. */
export type AiAgentApprovalKind = 'command' | 'permission'

export interface AiAgentApprovalRequest {
  requestId: string
  kind: AiAgentApprovalKind
  subject: string
  cwd: string
}

export interface AiAgentSudoRequest {
  requestId: string
  command: string
}


/** Conversation fields carried by every state snapshot; messages travel separately. */
export type AiAgentConversationMeta = Omit<AiAgentConversation, 'messages'>

export interface AiAgentStateSnapshot {
  type: 'state'
  providers: AiAgentProviderConfig[]
  providerKeys: string[]
  /** Conversation without its messages, or null when there is none. */
  conversation: AiAgentConversationMeta | null
  /**
   * How to apply `messages`: 'reset' replaces the renderer's list, 'append'
   * extends it. 'append' with an empty list means the list is unchanged.
   */
  messageMode: 'reset' | 'append'
  /** Messages to apply per `messageMode`. */
  messages: AiAgentConversationMsg[]
  conversationSummaries: AiAgentConversationSummary[]
  runPhase: AiAgentRunPhase
  hostKey: string
  hostLabel: string
  ssh: boolean
  pendingApproval: AiAgentApprovalRequest | null
  pendingSudo: AiAgentSudoRequest | null
  rules: string
  lastError?: string
}

export interface AiAgentDeltaPayload {
  type: 'delta'
  text: string
}

export interface AiAgentToolOutputPayload {
  type: 'toolOutput'
  toolCallId: string
  text: string
}

export interface AiAgentToastPayload {
  type: 'toast'
  kind: 'error' | 'info'
  text: string
}

export interface AiAgentChatAttachment {
  name: string
  mimeType?: string
  text?: string
  truncated: boolean
  binary: boolean
}

/** 'allowSession' allows every later command until this host's session ends (in-memory only). */
export type AiAgentApprovalDecision = 'allow' | 'deny' | 'allowAlways' | 'denyAlways' | 'allowSession'

export type AiAgentRendererMessage =
  | { type: 'sync' }
  | { type: 'probe' }
  | {
      type: 'chat'
      providerId: string
      model: string
      text: string
      attachTerminal?: boolean
      attachments?: AiAgentChatAttachment[]
    }
  | { type: 'stop' }
  | { type: 'resume' }
  | { type: 'discardPaused' }
  /** `pattern` is the rule list entry an 'allowAlways'/'denyAlways' decision saves. */
  | { type: 'approval'; requestId: string; decision: AiAgentApprovalDecision; pattern?: string }
  | { type: 'sudoPassword'; requestId: string; password: string | null }
  | { type: 'rulesChanged'; rules: string }
  | { type: 'select'; providerId: string; model: string }
  | { type: 'providersChanged'; providers: AiAgentProviderConfig[] }
  | { type: 'checkProvider'; provider: AiAgentProviderConfig }
  | { type: 'refreshProvider'; provider: AiAgentProviderConfig }
  | { type: 'refreshModels'; providerId: string; silent?: boolean }
  | { type: 'newChat'; providerId: string; model: string }
  | { type: 'openChat'; conversationId: string }
  | { type: 'deleteChat'; conversationId: string }
  | { type: 'renameChat'; conversationId: string; title: string }
  /** Drop this user message and everything after it, so it can be edited and resent. */
  | { type: 'rewind'; messageIndex: number }
