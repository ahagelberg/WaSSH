export const AI_AGENT_SETTING_DEFAULT_ALLOW_RULES = 'defaultAllowRules'
export const AI_AGENT_SETTING_DEFAULT_DENY_RULES = 'defaultDenyRules'
export const AI_AGENT_SETTING_HOST_ALLOW_RULES = 'allowRules'
export const AI_AGENT_SETTING_HOST_DENY_RULES = 'denyRules'

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
export const AI_AGENT_ANTHROPIC_MODELS_PATH = '/v1/models'

export interface AiAgentProviderConfig {
  id: string
  name: string
  protocol: AiAgentProviderProtocol
  baseUrl: string
  models: string[]
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


export interface AiAgentStateSnapshot {
  type: 'state'
  providers: AiAgentProviderConfig[]
  providerKeys: string[]
  conversation: AiAgentConversation | null
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

export type AiAgentApprovalDecision = 'allow' | 'deny' | 'allowAlways' | 'denyAlways'

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
  | { type: 'approval'; requestId: string; decision: AiAgentApprovalDecision }
  | { type: 'sudoPassword'; requestId: string; password: string | null }
  | { type: 'rulesChanged'; rules: string }
  | { type: 'select'; providerId: string; model: string }
  | { type: 'providersChanged'; providers: AiAgentProviderConfig[] }
  | { type: 'refreshModels'; providerId: string; silent?: boolean }
  | { type: 'newChat'; providerId: string; model: string }
  | { type: 'openChat'; conversationId: string }
  | { type: 'deleteChat'; conversationId: string }
