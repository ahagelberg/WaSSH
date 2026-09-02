/** Built-in plugin ids */
export const PLUGIN_ID_SERVER_MONITOR = 'server-monitor'
export const PLUGIN_ID_SCRATCHPAD = 'scratchpad'
export const PLUGIN_ID_MACRO_PAD = 'macro-pad'
export const PLUGIN_ID_MQTT_EXPLORER = 'mqtt-explorer'
export const PLUGIN_ID_SFTP = 'sftp'
export const PLUGIN_ID_AI_AGENT = 'ai-agent'

export const DEFAULT_ENABLED_PLUGINS: string[] = [
  PLUGIN_ID_SERVER_MONITOR,
  PLUGIN_ID_SCRATCHPAD,
  PLUGIN_ID_MACRO_PAD,
  PLUGIN_ID_MQTT_EXPLORER,
  PLUGIN_ID_SFTP,
  PLUGIN_ID_AI_AGENT
]

export type PluginActivation = 'manual' | 'auto'
export type PluginSource = 'builtin' | 'external'

export type PluginViewPlacement =
  | 'overlay'
  | 'split-left'
  | 'split-right'
  | 'split-top'
  | 'split-bottom'

export const PLUGIN_VIEW_PLACEMENTS: PluginViewPlacement[] = [
  'split-left',
  'split-right',
  'split-top',
  'split-bottom',
  'overlay'
]

export const PLUGIN_VIEW_PLACEMENT_LABELS: Record<PluginViewPlacement, string> = {
  'split-left': 'Left',
  'split-right': 'Right',
  'split-top': 'Top',
  'split-bottom': 'Bottom',
  overlay: 'Overlay'
}

export function isPluginViewPlacement(value: unknown): value is PluginViewPlacement {
  return (
    value === 'overlay' ||
    value === 'split-left' ||
    value === 'split-right' ||
    value === 'split-top' ||
    value === 'split-bottom'
  )
}

export type StreamMode = 'observe' | 'intercept'

export type StreamDirection = 'inbound' | 'outbound'

/** Host-managed side connection kinds (serial reserved for future) */
export type SideConnectionKind = 'ssh-exec' | 'ssh-shell' | 'tcp' | 'serial'

export type PluginSettingsFieldType =
  | 'boolean'
  | 'number'
  | 'string'
  | 'stringList'
  | 'macroList'

export interface PluginMacroButton {
  id: string
  label: string
  text: string
  /** e.g. "Ctrl+Shift+1" — empty = no hotkey */
  hotkey: string
}

export interface PluginSettingsField {
  key: string
  label: string
  type: PluginSettingsFieldType
  default: unknown
  description?: string
  /** When true, string fields use a password input in settings UI */
  secret?: boolean
}

export interface PluginToolbarContribution {
  label: string
}

export interface PluginViewContribution {
  id: string
  placement: PluginViewPlacement
  title?: string
}

export interface PluginManifest {
  id: string
  name: string
  version: string
  description: string
  activation: PluginActivation
  source: PluginSource
  contributes: {
    toolbar?: PluginToolbarContribution
    /** Options dialog section heading (app-wide settings) */
    settingsHeading?: string
    /** App-wide settings (Options dialog) */
    settingsSchema?: PluginSettingsField[]
    /** Host dialog section heading for per-host plugin settings */
    hostSettingsHeading?: string
    /** Per-host settings (Host / session settings dialog) */
    hostSettingsSchema?: PluginSettingsField[]
    views?: PluginViewContribution[]
    /** Future: menu contributions */
  }
}

export interface PluginListItem extends PluginManifest {
  enabled: boolean
}

export interface PluginActiveStateEvent {
  tabId: string
  pluginId: string
  active: boolean
}

export interface PluginMessageEvent {
  tabId: string
  pluginId: string
  payload: unknown
}

export interface SideConnectionOpenRequest {
  kind: SideConnectionKind
  /** ssh-exec command */
  command?: string
  /** ssh-shell: open an isolated duplicate SSH client */
  duplicate?: boolean
  /** tcp destination */
  host?: string
  port?: number
}

export interface SideConnectionOpened {
  connectionId: string
}

export interface SideConnectionDataEvent {
  connectionId: string
  data: string
}

export interface SideConnectionClosedEvent {
  connectionId: string
  error?: string
}

/** Default poll interval for server monitor (ms) */
export const SERVER_MONITOR_DEFAULT_INTERVAL_MS = 1000

/** Minimum poll interval for server monitor (ms) */
export const SERVER_MONITOR_MIN_INTERVAL_MS = 500

/** How many top processes to return per sample */
export const SERVER_MONITOR_TOP_PROCESS_COUNT = 24

/** Bytes in one kibibyte (ps rss /proc/meminfo units) */
export const BYTES_PER_KIB = 1024

/** Bits per byte (link speed → byte rate) */
export const BITS_PER_BYTE = 8

/** sysfs net speed file unit (decimal megabits) */
export const SERVER_MONITOR_MEGABIT_BITS = 1_000_000

/** Linux /proc/diskstats sector size */
export const SERVER_MONITOR_DISK_SECTOR_BYTES = 512

/** /sys thermal zone temp units → °C */
export const SERVER_MONITOR_TEMP_MILLI_PER_C = 1000

/** Loopback iface excluded from network table */
export const SERVER_MONITOR_LOOPBACK_IFACE = 'lo'

/** Visibility defaults for monitor panel sections */
export const SERVER_MONITOR_SHOW_GAUGES_DEFAULT = true
export const SERVER_MONITOR_SHOW_SPARKS_DEFAULT = true
export const SERVER_MONITOR_SHOW_STATUS_DEFAULT = true
export const SERVER_MONITOR_SHOW_PROCESSES_DEFAULT = true
export const SERVER_MONITOR_SHOW_NETWORK_DEFAULT = true

/** Client-side process table sort */
export type ServerMonitorProcessSort = 'cpu' | 'mem'

/** One row from remote `ps` (CPU-sorted from host) */
export interface ServerMonitorProcess {
  pid: number
  user: string
  /** Single-letter process state from `ps` */
  state: string
  nice: number
  threads: number
  cpuPercent: number
  memPercent: number
  rssBytes: number
  command: string
}

/** Per-interface counters and derived rates */
export interface ServerMonitorNetIface {
  name: string
  rxBytes: number
  txBytes: number
  /** Bytes/sec; null until two samples exist */
  rxRate: number | null
  /** Bytes/sec; null until two samples exist */
  txRate: number | null
  /** Nominal link speed (bits/sec); null if unknown or down */
  speedBitsPerSec: number | null
}

/** Thermal zone reading when /sys exposes it */
export interface ServerMonitorTemp {
  name: string
  celsius: number
}

/** Structured stats pushed from the server-monitor main module to the UI */
export interface ServerMonitorSnapshot {
  updatedAt: number
  hostname: string
  uptimeSec: number
  load1: number
  load5: number
  load15: number
  /** Kernel / OS summary from `uname` */
  kernel: string
  /** Logical CPU count */
  cpuCount: number
  /** Runnable threads (from loadavg) */
  procsRunning: number
  /** Total threads (from loadavg) */
  procsTotal: number
  /** Aggregate 0–100; null until two CPU samples exist */
  cpuPercent: number | null
  /** Per-logical-CPU 0–100; null entries until two samples */
  cpuCores: Array<number | null>
  memTotalBytes: number
  /** total − available (gauge / overall used) */
  memUsedBytes: number
  memAvailableBytes: number
  memFreeBytes: number
  memBuffersBytes: number
  memCachedBytes: number
  swapTotalBytes: number
  swapUsedBytes: number
  diskTotalBytes: number
  diskUsedBytes: number
  /** Cumulative whole-disk bytes since boot (diskstats) */
  diskReadBytes: number
  diskWriteBytes: number
  /** Bytes/sec; null until two samples */
  diskReadRate: number | null
  diskWriteRate: number | null
  temperatures: ServerMonitorTemp[]
  processes: ServerMonitorProcess[]
  network: ServerMonitorNetIface[]
  error?: string
}

/** Default MQTT broker host on the remote machine */
export const MQTT_EXPLORER_DEFAULT_HOST = '127.0.0.1'

/** Default plain MQTT port */
export const MQTT_EXPLORER_DEFAULT_PORT = 1883

/** Max retained history entries per topic in the UI */
export const MQTT_EXPLORER_HISTORY_LIMIT = 100

/** Brief highlight duration when a topic receives a message (ms) */
export const MQTT_EXPLORER_BLINK_MS = 500

export type MqttExplorerStatusState =
  | 'idle'
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'unavailable'
  | 'error'

export type MqttExplorerErrorKind =
  | 'not_ssh'
  | 'no_broker'
  | 'auth_failed'
  | 'unreachable'
  | 'other'

/** Main → renderer status event */
export interface MqttExplorerStatusPayload {
  type: 'status'
  state: MqttExplorerStatusState
  reason?: string
  errorKind?: MqttExplorerErrorKind
}

/** Main → renderer message event */
export interface MqttExplorerMessagePayload {
  type: 'message'
  topic: string
  /** UTF-8 text when binary is false */
  payloadText?: string
  /** Base64 when binary is true */
  payloadBase64?: string
  binary: boolean
  qos: 0 | 1 | 2
  retain: boolean
  timestamp: number
}

/** Renderer → main */
export type MqttExplorerRendererMessage =
  | {
      type: 'publish'
      topic: string
      payloadBase64: string
      qos: 0 | 1 | 2
      retain: boolean
    }
  | { type: 'reconnect' }

/** SFTP entry kind derived from the remote mode bits */
export type SftpEntryType = 'file' | 'directory' | 'symlink' | 'other'

/** One row in the remote file manager listing */
export interface SftpEntry {
  name: string
  /** Full remote path (parent + name) */
  path: string
  type: SftpEntryType
  size: number
  /** Numeric permission bits (e.g. 0o755) */
  mode: number
  /** Human-readable permission string (e.g. "-rw-r--r--") */
  modeSymbolic: string
  /** Last modified time (epoch ms) */
  mtime: number
  uid?: number
  gid?: number
}

export type SftpErrorKind =
  | 'not_ssh'
  | 'not_found'
  | 'permission'
  | 'not_dir'
  | 'exists'
  | 'name_in_use'
  | 'io'
  | 'connection'
  | 'cancelled'
  | 'other'

export type SftpStatusState = 'idle' | 'connecting' | 'connected' | 'error'

/** Main → renderer: connection / cwd status */
export interface SftpStatusPayload {
  type: 'status'
  state: SftpStatusState
  /** Current remote working directory */
  cwd?: string
  reason?: string
  errorKind?: SftpErrorKind
}

/** Main → renderer: directory listing result */
export interface SftpListPayload {
  type: 'listResult'
  path: string
  cwd: string
  entries: SftpEntry[]
  error?: string
  errorKind?: SftpErrorKind
}

export type SftpOpName = 'mkdir' | 'rename' | 'chmod' | 'delete'

/** Main → renderer: result of a mutating operation */
export interface SftpOpResultPayload {
  type: 'opResult'
  op: SftpOpName
  path: string
  ok: boolean
  error?: string
  errorKind?: SftpErrorKind
}

export type SftpTransferDirection = 'upload' | 'download'

/** Main → renderer: byte progress for an active transfer */
export interface SftpTransferProgressPayload {
  type: 'transferProgress'
  direction: SftpTransferDirection
  remotePath: string
  transferredBytes: number
  totalBytes: number
}

/** Main → renderer: transfer finished */
export interface SftpTransferDonePayload {
  type: 'transferDone'
  direction: SftpTransferDirection
  remotePath: string
  state: 'done' | 'error' | 'cancelled'
  error?: string
  errorKind?: SftpErrorKind
}

/** Renderer → main SFTP commands */
export type SftpRendererMessage =
  | { type: 'getStatus' }
  | { type: 'list'; path?: string }
  | { type: 'mkdir'; path: string }
  | { type: 'rename'; oldPath: string; newPath: string }
  | { type: 'chmod'; path: string; mode: number }
  | { type: 'delete'; path: string }
  | { type: 'download'; path: string }
  | { type: 'uploadDialog'; path?: string }
  | { type: 'uploadStart'; name: string; size: number; path?: string }
  | { type: 'uploadChunk'; name: string; data: Uint8Array }
  | { type: 'uploadEnd'; name: string }
  | { type: 'cancel' }
  | { type: 'resetCwd' }

/* AI agent */

/** Settings field keys (stringList of allow/deny rules) */
export const AI_AGENT_SETTING_DEFAULT_ALLOW_RULES = 'defaultAllowRules'
export const AI_AGENT_SETTING_DEFAULT_DENY_RULES = 'defaultDenyRules'
export const AI_AGENT_SETTING_HOST_ALLOW_RULES = 'allowRules'
export const AI_AGENT_SETTING_HOST_DENY_RULES = 'denyRules'

/** Line prefix that switches a rule to regex matching */
export const AI_AGENT_RULE_REGEX_PREFIX = 'regex:'

/** Agent wire protocols */
export const AI_AGENT_PROTOCOL_OPENAI = 'openai'
export const AI_AGENT_PROTOCOL_ANTHROPIC = 'anthropic'
export type AiAgentProviderProtocol =
  | typeof AI_AGENT_PROTOCOL_OPENAI
  | typeof AI_AGENT_PROTOCOL_ANTHROPIC

/** Built-in provider presets (ids are stable) */
export const AI_AGENT_LOCAL_PROVIDER_ID = 'local'
export const AI_AGENT_OPENROUTER_PROVIDER_ID = 'openrouter'
export const AI_AGENT_ANTHROPIC_PROVIDER_ID = 'anthropic'
export const AI_AGENT_DEEPSEEK_PROVIDER_ID = 'deepseek'
export const AI_AGENT_GROK_PROVIDER_ID = 'grok'

export const AI_AGENT_LOCAL_BASE_URL = 'http://127.0.0.1:11434/v1'
export const AI_AGENT_OPENROUTER_BASE_URL = 'https://openrouter.ai/api/v1'
export const AI_AGENT_ANTHROPIC_BASE_URL = 'https://api.anthropic.com'
export const AI_AGENT_DEEPSEEK_BASE_URL = 'https://api.deepseek.com'
export const AI_AGENT_GROK_BASE_URL = 'https://api.x.ai/v1'

/** Anthropic protocol additions */
export const AI_AGENT_ANTHROPIC_VERSION = '2023-06-01'
export const AI_AGENT_ANTHROPIC_PATH = '/v1/messages'
export const AI_AGENT_OPENAI_CHAT_PATH = '/chat/completions'

/** Default model suggestions per provider (editable in the provider manager) */
export const AI_AGENT_LOCAL_DEFAULT_MODELS = ['llama3.1', 'qwen2.5-coder:7b']
export const AI_AGENT_OPENROUTER_DEFAULT_MODELS = ['anthropic/claude-sonnet-4-20250514']
export const AI_AGENT_ANTHROPIC_DEFAULT_MODELS = [
  'claude-sonnet-4-20250514',
  'claude-opus-4-20250514',
  'claude-3-5-haiku-20241022'
]
export const AI_AGENT_DEEPSEEK_DEFAULT_MODELS = ['deepseek-chat', 'deepseek-reasoner']
export const AI_AGENT_GROK_DEFAULT_MODELS = ['grok-3', 'grok-3-mini-beta']

export interface AiAgentProviderConfig {
  id: string
  name: string
  protocol: AiAgentProviderProtocol
  baseUrl: string
  models: string[]
}

export const AI_AGENT_DEFAULT_PROVIDERS: AiAgentProviderConfig[] = [
  {
    id: AI_AGENT_LOCAL_PROVIDER_ID,
    name: 'Local (OpenAI compatible)',
    protocol: AI_AGENT_PROTOCOL_OPENAI,
    baseUrl: AI_AGENT_LOCAL_BASE_URL,
    models: [...AI_AGENT_LOCAL_DEFAULT_MODELS]
  },
  {
    id: AI_AGENT_OPENROUTER_PROVIDER_ID,
    name: 'Open Router',
    protocol: AI_AGENT_PROTOCOL_OPENAI,
    baseUrl: AI_AGENT_OPENROUTER_BASE_URL,
    models: [...AI_AGENT_OPENROUTER_DEFAULT_MODELS]
  },
  {
    id: AI_AGENT_ANTHROPIC_PROVIDER_ID,
    name: 'Anthropic',
    protocol: AI_AGENT_PROTOCOL_ANTHROPIC,
    baseUrl: AI_AGENT_ANTHROPIC_BASE_URL,
    models: [...AI_AGENT_ANTHROPIC_DEFAULT_MODELS]
  },
  {
    id: AI_AGENT_DEEPSEEK_PROVIDER_ID,
    name: 'DeepSeek',
    protocol: AI_AGENT_PROTOCOL_OPENAI,
    baseUrl: AI_AGENT_DEEPSEEK_BASE_URL,
    models: [...AI_AGENT_DEEPSEEK_DEFAULT_MODELS]
  },
  {
    id: AI_AGENT_GROK_PROVIDER_ID,
    name: 'Grok (xAI)',
    protocol: AI_AGENT_PROTOCOL_OPENAI,
    baseUrl: AI_AGENT_GROK_BASE_URL,
    models: [...AI_AGENT_GROK_DEFAULT_MODELS]
  }
]

/**
 * Built-in read-only command patterns that auto-run without approval.
 * Lines may be globs; `regex:` prefix switches to a regular expression.
 */
export const AI_AGENT_SAFE_RULES: string[] = [
  'pwd',
  'whoami',
  'id*',
  'hostname*',
  'uname*',
  'date',
  'uptime',
  'ls*',
  'lscpu*',
  'free*',
  'df*',
  'ps*',
  'git status*',
  'git log*',
  'git diff*',
  'git branch*',
  'git remote*',
  'git show*',
  'git rev-parse*'
]

/** Plugin-owned data file schema */
export const AI_AGENT_DATA_VERSION = 1

/** Vault id namespace for AI agent provider API keys (safeStorage-encrypted). */
export const AI_AGENT_KEY_VAULT_PREFIX = 'ai-agent:'

export function aiAgentVaultId(providerId: string): string {
  return `${AI_AGENT_KEY_VAULT_PREFIX}${providerId}`
}

export interface AiAgentDataFile {
  version: number
  providers: AiAgentProviderConfig[]
  conversations: Record<string, AiAgentConversation>
  /** User-authored instructions always added to prompts (all providers) */
  rules: string
}

/** One tool invocation requested by the model */
export interface AiAgentToolCall {
  id: string
  command: string
}

export type AiAgentToolOutcome = 'ok' | 'denied' | 'error' | 'timeout' | 'cancelled'

export interface AiAgentConversationUserMsg {
  role: 'user'
  text: string
  /** Whether recent terminal output was attached to this message */
  usedTerminalContext: boolean
}

export interface AiAgentConversationAssistantMsg {
  role: 'assistant'
  text?: string
  toolCalls?: AiAgentToolCall[]
  providerId?: string
  model?: string
  /** True when the run was stopped by the user mid-message */
  stopped?: boolean
}

export interface AiAgentConversationToolMsg {
  role: 'tool'
  toolCallId: string
  command: string
  content: string
  outcome: AiAgentToolOutcome
  truncated: boolean
}

export type AiAgentConversationMsg =
  | AiAgentConversationUserMsg
  | AiAgentConversationAssistantMsg
  | AiAgentConversationToolMsg

/** Persisted per-host conversation */
export interface AiAgentConversation {
  version: number
  activeProviderId: string
  activeModel: string
  hostLabel: string
  cwd: string
  messages: AiAgentConversationMsg[]
}

export type AiAgentRunPhase = 'no_session' | 'idle' | 'running' | 'ask' | 'paused'

export interface AiAgentApprovalRequest {
  requestId: string
  command: string
  cwd: string
}

/** Main → renderer: full UI snapshot */
export interface AiAgentStateSnapshot {
  type: 'state'
  providers: AiAgentProviderConfig[]
  /** Provider ids that currently have a key stored in the vault */
  providerKeys: string[]
  conversation: AiAgentConversation | null
  runPhase: AiAgentRunPhase
  hostKey: string
  hostLabel: string
  ssh: boolean
  pendingApproval: AiAgentApprovalRequest | null
  /** User-authored rules for prompts (all providers) */
  rules: string
  lastError?: string
}

/** Main → renderer: streamed assistant text delta */
export interface AiAgentDeltaPayload {
  type: 'delta'
  text: string
}

/** Main → renderer: transient status line */
export interface AiAgentToastPayload {
  type: 'toast'
  kind: 'error' | 'info'
  text: string
}

export type AiAgentApprovalDecision = 'allow' | 'deny' | 'allowAlways' | 'denyAlways'

/** Renderer → main */
export type AiAgentRendererMessage =
  | { type: 'sync' }
  | { type: 'probe' }
  | { type: 'chat'; providerId: string; model: string; text: string; attachTerminal?: boolean }
  | { type: 'stop' }
  | { type: 'resume' }
  | { type: 'discardPaused' }
  | { type: 'approval'; requestId: string; decision: AiAgentApprovalDecision }
  | { type: 'rulesChanged'; rules: string }
  | { type: 'select'; providerId: string; model: string }
  | { type: 'providersChanged'; providers: AiAgentProviderConfig[] }
  | { type: 'newChat'; providerId: string; model: string }

export function defaultPluginSettingsFromSchema(
  schema: PluginSettingsField[] | undefined
): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  if (!schema) {
    return out
  }
  for (const field of schema) {
    out[field.key] = field.default
  }
  return out
}

export function mergePluginSettings(
  schema: PluginSettingsField[] | undefined,
  stored: unknown
): Record<string, unknown> {
  const defaults = defaultPluginSettingsFromSchema(schema)
  if (!schema || schema.length === 0) {
    return defaults
  }
  if (!stored || typeof stored !== 'object' || Array.isArray(stored)) {
    return defaults
  }
  const src = stored as Record<string, unknown>
  const out = { ...defaults }
  for (const field of schema) {
    if (Object.prototype.hasOwnProperty.call(src, field.key)) {
      out[field.key] = src[field.key]
    }
  }
  return out
}

/**
 * Merge app-wide + per-host plugin settings for a session.
 * Host keys override app keys when both schemas define the same key (they should not).
 */
export function mergePluginSessionSettings(
  manifest: Pick<PluginManifest, 'contributes'> | undefined,
  appStored: unknown,
  hostStored: unknown
): Record<string, unknown> {
  const app = mergePluginSettings(manifest?.contributes.settingsSchema, appStored)
  const host = mergePluginSettings(manifest?.contributes.hostSettingsSchema, hostStored)
  return { ...app, ...host }
}

/** Normalize HostProfile / ConnectionParams pluginSettings maps */
export function normalizeHostPluginSettings(
  raw: unknown
): Record<string, Record<string, unknown>> {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return {}
  }
  const out: Record<string, Record<string, unknown>> = {}
  for (const [pluginId, value] of Object.entries(raw as Record<string, unknown>)) {
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      out[pluginId] = { ...(value as Record<string, unknown>) }
    }
  }
  return out
}
