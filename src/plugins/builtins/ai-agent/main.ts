import { randomUUID } from 'crypto'
import { readdir, readFile } from 'fs/promises'
import { extname, join } from 'path'
import {
  AI_AGENT_CONVERSATIONS_PER_HOST_MAX,
  AI_AGENT_DATA_VERSION,
  AI_AGENT_DEFAULT_CHAT_TITLE,
  AI_AGENT_DEFAULT_WEB_SEARCH_PROVIDER,
  AI_AGENT_TITLE_MAX_CHARS
} from './defaults'
import { PLUGIN_ID_AI_AGENT, aiAgentVaultId } from './id'
import {
  AI_AGENT_GROUP_LOCAL_FS,
  AI_AGENT_GROUP_REMOTE_FS,
  AI_AGENT_GROUP_WEB_ACCESS,
  AI_AGENT_GROUP_WEB_SEARCH,
  AI_AGENT_DATETIME_METHOD,
  AI_AGENT_LOCAL_FS_METHODS,
  AI_AGENT_REMOTE_FS_METHODS,
  AI_AGENT_WEB_ACCESS_METHODS,
  AI_AGENT_WEB_SEARCH_METHODS,
  TOOL_DEF_RUN_COMMAND
} from './apiMethods'
import {
  allExternalApiTools,
  allowedGroupMethods,
  isMethodAllowed,
  permissionSettingKey,
  resolvePermission,
  resolveToolTarget
} from './pluginApiTools'
import {
  AI_AGENT_PROTOCOL_ANTHROPIC,
  AI_AGENT_PROTOCOL_OPENAI,
  AI_AGENT_SETTING_DEFAULT_ALLOW_RULES,
  AI_AGENT_SETTING_DEFAULT_DENY_RULES,
  AI_AGENT_SETTING_GLOBAL_PROMPT,
  AI_AGENT_SETTING_HOST_ALLOW_RULES,
  AI_AGENT_SETTING_HOST_DENY_RULES,
  AI_AGENT_SETTING_HOST_PROMPT,
  AI_AGENT_SETTING_PROMPT_FILES_FOLDER,
  AI_AGENT_SETTING_WEB_SEARCH_API_KEY,
  AI_AGENT_SETTING_WEB_SEARCH_PROVIDER,
  AI_AGENT_TOOL_GET_CURRENT_TIME,
  AI_AGENT_TOOL_LOCAL_FS_LIST,
  AI_AGENT_TOOL_LOCAL_FS_READ,
  AI_AGENT_TOOL_LOCAL_FS_WRITE,
  AI_AGENT_TOOL_LOCAL_FS_EDIT,
  AI_AGENT_TOOL_REMOTE_FS_DELETE,
  AI_AGENT_TOOL_REMOTE_FS_EDIT,
  AI_AGENT_TOOL_REMOTE_FS_LIST,
  AI_AGENT_TOOL_REMOTE_FS_READ,
  AI_AGENT_TOOL_REMOTE_FS_WRITE,
  AI_AGENT_TOOL_RUN_COMMAND,
  AI_AGENT_TOOL_WEB_FETCH,
  AI_AGENT_TOOL_WEB_SEARCH,
  type AiAgentApprovalKind,
  type AiAgentApprovalRequest,
  type AiAgentChatAttachment,
  type AiAgentConversation,
  type AiAgentConversationMsg,
  type AiAgentConversationSummary,
  type AiAgentConversationToolMsg,
  type AiAgentDataFile,
  type AiAgentProviderConfig,
  type AiAgentRendererMessage,
  type AiAgentRunPhase,
  type AiAgentStateSnapshot,
  type AiAgentSudoRequest,
  type AiAgentToolOutcome
} from './protocol'
import type { PluginMainContext, PluginMainModule } from '@plugin-api/main'
import type { PluginApiMethod } from '@plugin-api/shared'
import { decideCommand } from './permissions'
import {
  complete,
  listModels,
  RUN_COMMAND_TOOL_NAME,
  type ApiMessage,
  type ApiToolCallMsg
} from './providers'
import {
  executeDateTime,
  executeLocalFsList,
  executeLocalFsRead,
  executeLocalFsWrite,
  executeLocalFsEdit,
  executeRemoteFsDelete,
  executeRemoteFsEdit,
  executeRemoteFsList,
  executeRemoteFsRead,
  executeRemoteFsWrite,
  executeWebFetch,
  executeWebSearch
} from './tools'

/** Cap on a plugin-API tool result before it's fed back to the model (generous - not a token limit). */
const MAX_PLUGIN_API_RESULT_CHARS = 1_000_000


/** Max chained tool steps per run before we stop the loop */
const MAX_RUN_STEPS = 50

/** max_tokens sent to providers */
const MAX_TOKENS = 8192

/** Command timeout before the exec channel is closed (ms) */
const EXEC_TIMEOUT_MS = 120_000

/** Bytes of command output kept for the model (marker tail is extra) */
const MAX_OUTPUT_CHARS = 64_000

/** Extra tail bytes retained so exit/pwd markers survive truncation */
const MARKER_TAIL_CHARS = 4_000

/** Terminal context tail kept per tab */
const TERMINAL_TAIL_CHARS = 8_000

/** Max chars of terminal context attached to one message */
const TERMINAL_CONTEXT_EXCERPT_CHARS = 6_000

/** History entries kept per conversation (oldest trimmed) */
const HISTORY_MAX = 160

/** Remote project rules file read from the working directory */
const PROJECT_RULES_FILE = '.wasshrules'

/** Local prompt file extensions loaded from the configured folder. */
const PROMPT_FILE_EXTENSIONS = new Set(['.txt', '.yaml', '.yml'])

/** How long a successful sudo password / NOPASSWD probe stays cached in memory (ms) */
const SUDO_PASSWORD_CACHE_MS = 5 * 60 * 1000

/** Detects a sudo invocation in a shell command line */
const SUDO_COMMAND_RE = /\bsudo\b/

/** Marker printed when sudo -S / -n authentication fails inside the exec wrapper */
const SUDO_AUTH_FAILED_MARKER = '[wassh: sudo authentication failed]'

interface TabRuntime {
  ctx: PluginMainContext
  hostKey: string
  terminalTail: string
}

interface HostState {
  hostKey: string
  hostLabel: string
  conversation: AiAgentConversation | null
  phase: AiAgentRunPhase
  extraAllow: string[]
  extraDeny: string[]
  pendingApproval: AiAgentApprovalRequest | null
  approvalResolve: ((decision: string) => void) | null
  pendingSudo: AiAgentSudoRequest | null
  sudoResolve: ((password: string | null) => void) | null
  /** In-memory only — never persisted or sent to the renderer */
  sudoPassword: string | null
  sudoPasswordExpiresAt: number
  /** True while NOPASSWD sudo works; expires with the same cache window */
  sudoNopassExpiresAt: number
  controller: AbortController | null
  connectionId: string | null
  /** The tab ctx that started the current run (used to close exec channels) */
  runCtx: PluginMainContext | null
  stopped: boolean
  inRun: boolean
  lastError?: string
}

const tabs = new Map<string, TabRuntime>()
const hosts = new Map<string, HostState>()
const hostRefCount = new Map<string, number>()
let dataFile: AiAgentDataFile | null = null
let dataWriter: PluginMainContext | null = null

function isProviderConfig(value: unknown): value is AiAgentProviderConfig {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return false
  }
  const provider = value as Record<string, unknown>
  return (
    typeof provider.id === 'string' &&
    provider.id.trim().length > 0 &&
    typeof provider.name === 'string' &&
    typeof provider.baseUrl === 'string' &&
    Array.isArray(provider.models)
  )
}

function normalizeProviders(raw: unknown): AiAgentProviderConfig[] {
  if (!Array.isArray(raw)) {
    return []
  }
  const providerIds = new Set<string>()
  const providers: AiAgentProviderConfig[] = []
  for (const value of raw) {
    if (!isProviderConfig(value)) {
      continue
    }
    if (
      value.protocol !== AI_AGENT_PROTOCOL_OPENAI &&
      value.protocol !== AI_AGENT_PROTOCOL_ANTHROPIC
    ) {
      continue
    }
    const id = value.id.trim()
    if (providerIds.has(id)) {
      continue
    }
    providerIds.add(id)
    providers.push({
      id,
      name: value.name.trim() || 'Provider',
      protocol:
        value.protocol === AI_AGENT_PROTOCOL_ANTHROPIC
          ? AI_AGENT_PROTOCOL_ANTHROPIC
          : AI_AGENT_PROTOCOL_OPENAI,
      baseUrl: value.baseUrl.trim(),
      models: value.models
        .filter((model): model is string => typeof model === 'string' && model.trim().length > 0)
        .map((model) => model.trim())
    })
  }
  return providers
}

function asDataFile(raw: unknown): AiAgentDataFile {
  const fallback: AiAgentDataFile = {
    version: AI_AGENT_DATA_VERSION,
    providers: [],
    conversations: {},
    activeConversationId: {},
    rules: ''
  }
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return fallback
  }
  const src = raw as AiAgentDataFile & { conversations?: Record<string, unknown> }
  const providers = normalizeProviders(src.providers)
  const { conversations, activeConversationId } = migrateConversations(
    src.conversations && typeof src.conversations === 'object' && !Array.isArray(src.conversations)
      ? (src.conversations as Record<string, unknown>)
      : {},
    src.activeConversationId &&
      typeof src.activeConversationId === 'object' &&
      !Array.isArray(src.activeConversationId)
      ? src.activeConversationId
      : {}
  )
  return {
    version: AI_AGENT_DATA_VERSION,
    providers,
    conversations,
    activeConversationId,
    rules: typeof src.rules === 'string' ? src.rules : ''
  }
}

/** Marker separating user text from attached terminal excerpt */
const TERMINAL_CONTEXT_MARKER = '\n\n[Recent terminal output]'

function titleFromMessages(messages: AiAgentConversationMsg[]): string {
  const first = messages.find((m): m is Extract<AiAgentConversationMsg, { role: 'user' }> => m.role === 'user')
  if (!first) {
    return AI_AGENT_DEFAULT_CHAT_TITLE
  }
  let text = first.text
  const markerAt = text.indexOf(TERMINAL_CONTEXT_MARKER)
  if (markerAt >= 0) {
    text = text.slice(0, markerAt)
  }
  text = text.replace(/\s+/g, ' ').trim()
  if (!text) {
    return AI_AGENT_DEFAULT_CHAT_TITLE
  }
  if (text.length <= AI_AGENT_TITLE_MAX_CHARS) {
    return text
  }
  return `${text.slice(0, AI_AGENT_TITLE_MAX_CHARS - 1).trimEnd()}…`
}

function normalizeConversation(
  raw: Record<string, unknown>,
  fallbackHostKey: string
): AiAgentConversation | null {
  const messages = Array.isArray(raw.messages) ? (raw.messages as AiAgentConversationMsg[]) : []
  const id = typeof raw.id === 'string' && raw.id.length > 0 ? raw.id : randomUUID()
  const hostKey =
    typeof raw.hostKey === 'string' && raw.hostKey.length > 0 ? raw.hostKey : fallbackHostKey
  if (!hostKey) {
    return null
  }
  const title =
    typeof raw.title === 'string' && raw.title.trim().length > 0
      ? raw.title.trim()
      : titleFromMessages(messages)
  const lastSelectedModelByProvider = Object.fromEntries(
    Object.entries(raw.lastSelectedModelByProvider ?? {}).filter(
      ([providerId, model]) => typeof providerId === 'string' && typeof model === 'string'
    )
  ) as Record<string, string>
  const activeProviderId = typeof raw.activeProviderId === 'string' ? raw.activeProviderId : ''
  const activeModel = typeof raw.activeModel === 'string' ? raw.activeModel : ''
  if (activeProviderId && activeModel && !lastSelectedModelByProvider[activeProviderId]) {
    lastSelectedModelByProvider[activeProviderId] = activeModel
  }
  return {
    id,
    hostKey,
    title,
    updatedAt: typeof raw.updatedAt === 'number' ? raw.updatedAt : Date.now(),
    version: AI_AGENT_DATA_VERSION,
    activeProviderId,
    activeModel,
    lastSelectedModelByProvider,
    hostLabel: typeof raw.hostLabel === 'string' ? raw.hostLabel : '',
    cwd: typeof raw.cwd === 'string' ? raw.cwd : '/',
    messages
  }
}

/**
 * v1 stored one conversation per hostKey. v2 keys by conversation id and
 * tracks the active id per host.
 */
function migrateConversations(
  raw: Record<string, unknown>,
  activeRaw: Record<string, string>
): {
  conversations: Record<string, AiAgentConversation>
  activeConversationId: Record<string, string>
} {
  const conversations: Record<string, AiAgentConversation> = {}
  const activeConversationId: Record<string, string> = { ...activeRaw }
  for (const [key, value] of Object.entries(raw)) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      continue
    }
    const entry = value as Record<string, unknown>
    const hasNewShape = typeof entry.id === 'string' && typeof entry.hostKey === 'string'
    const conv = normalizeConversation(entry, hasNewShape ? String(entry.hostKey) : key)
    if (!conv) {
      continue
    }
    conversations[conv.id] = conv
    if (!hasNewShape && !activeConversationId[key]) {
      activeConversationId[key] = conv.id
    }
  }
  return { conversations, activeConversationId }
}

function createConversation(
  hostKey: string,
  hostLabel: string,
  providerId: string,
  model: string,
  cwd: string
): AiAgentConversation {
  return {
    id: randomUUID(),
    hostKey,
    title: AI_AGENT_DEFAULT_CHAT_TITLE,
    updatedAt: Date.now(),
    version: AI_AGENT_DATA_VERSION,
    activeProviderId: providerId,
    activeModel: model,
    lastSelectedModelByProvider: providerId && model ? { [providerId]: model } : {},
    hostLabel,
    cwd,
    messages: []
  }
}

function conversationsForHost(hostKey: string): AiAgentConversation[] {
  if (!dataFile) {
    return []
  }
  return Object.values(dataFile.conversations).filter((c) => c.hostKey === hostKey)
}

function listConversationSummaries(hostKey: string): AiAgentConversationSummary[] {
  return conversationsForHost(hostKey)
    .slice()
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .map((c) => ({
      id: c.id,
      title: c.title || AI_AGENT_DEFAULT_CHAT_TITLE,
      updatedAt: c.updatedAt
    }))
}

function pruneHostConversations(hostKey: string): void {
  if (!dataFile) {
    return
  }
  const list = conversationsForHost(hostKey)
    .slice()
    .sort((a, b) => b.updatedAt - a.updatedAt)
  if (list.length <= AI_AGENT_CONVERSATIONS_PER_HOST_MAX) {
    return
  }
  const activeId = dataFile.activeConversationId[hostKey]
  const drop = list.slice(AI_AGENT_CONVERSATIONS_PER_HOST_MAX)
  for (const conv of drop) {
    if (conv.id === activeId) {
      continue
    }
    delete dataFile.conversations[conv.id]
  }
}

function refreshConversationTitle(conversation: AiAgentConversation): void {
  if (conversation.title !== AI_AGENT_DEFAULT_CHAT_TITLE && conversation.title.length > 0) {
    return
  }
  conversation.title = titleFromMessages(conversation.messages)
}

function loadData(ctx: PluginMainContext): void {
  if (!dataFile) {
    dataFile = asDataFile(ctx.getData())
  }
  if (!dataWriter) {
    dataWriter = ctx
  }
}

function saveData(): void {
  if (dataFile && dataWriter) {
    dataWriter.setData(dataFile)
  }
}

function findProvider(providerId: string): AiAgentProviderConfig | undefined {
  return dataFile?.providers.find((p) => p.id === providerId)
}

function tabCtxForHost(hostKey: string): PluginMainContext | undefined {
  for (const tab of tabs.values()) {
    if (tab.hostKey === hostKey) {
      return tab.ctx
    }
  }
  return undefined
}

function pushState(host: HostState): void {
  const ctx = tabCtxForHost(host.hostKey)
  // Key presence is derived live from the vault; nothing is persisted on the
  // provider config.
  const providerKeys = ctx
    ? (dataFile?.providers ?? [])
        .filter((p) => ctx.getSecret(aiAgentVaultId(p.id)) !== null)
        .map((p) => p.id)
    : []
  const ssh = Array.from(tabs.values()).some(
    (tab) => tab.hostKey === host.hostKey && tab.ctx.isSshSession()
  )
  const snapshot: AiAgentStateSnapshot = {
    type: 'state',
    providers: dataFile?.providers ?? [],
    providerKeys,
    conversation: host.conversation,
    conversationSummaries: listConversationSummaries(host.hostKey),
    runPhase: host.phase,
    hostKey: host.hostKey,
    hostLabel: host.hostLabel,
    ssh,
    pendingApproval: host.pendingApproval,
    pendingSudo: host.pendingSudo,
    rules: dataFile?.rules ?? '',
    lastError: host.lastError
  }
  for (const tab of tabs.values()) {
    if (tab.hostKey === host.hostKey) {
      tab.ctx.sendToRenderer(snapshot)
    }
  }
}

function pushToast(host: HostState, kind: 'error' | 'info', text: string): void {
  for (const tab of tabs.values()) {
    if (tab.hostKey === host.hostKey) {
      tab.ctx.sendToRenderer({ type: 'toast', kind, text })
    }
  }
}

function pushDelta(host: HostState, text: string): void {
  for (const tab of tabs.values()) {
    if (tab.hostKey === host.hostKey) {
      tab.ctx.sendToRenderer({ type: 'delta', text })
    }
  }
}

/** Drop an in-flight tail (unresolved tool turn or stopped partial text). */
function pruneUnresolvedTail(conversation: AiAgentConversation): void {
  const messages = conversation.messages
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const msg = messages[i]
    if (msg.role !== 'assistant') {
      continue
    }
    const callIds = new Set((msg.toolCalls ?? []).map((tc) => tc.id))
    for (let j = i + 1; j < messages.length; j += 1) {
      const later = messages[j]
      if (later.role === 'tool') {
        callIds.delete(later.toolCallId)
      }
    }
    if (callIds.size === 0 && !msg.stopped) {
      break
    }
    messages.splice(i)
    break
  }
}

function trimHistory(conversation: AiAgentConversation): void {
  if (conversation.messages.length > HISTORY_MAX) {
    conversation.messages.splice(0, conversation.messages.length - HISTORY_MAX)
  }
}

function persistConversation(host: HostState): void {
  if (!host.conversation || !dataFile) {
    return
  }
  trimHistory(host.conversation)
  refreshConversationTitle(host.conversation)
  host.conversation.updatedAt = Date.now()
  host.conversation.hostKey = host.hostKey
  dataFile.conversations[host.conversation.id] = host.conversation
  dataFile.activeConversationId[host.hostKey] = host.conversation.id
  pruneHostConversations(host.hostKey)
  saveData()
}

function extractCommand(argumentsJson: string): string {
  try {
    const parsed = JSON.parse(argumentsJson) as { command?: unknown }
    return typeof parsed.command === 'string' ? parsed.command.trim() : ''
  } catch {
    return ''
  }
}

function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

/** Single-quote shell escape for paths embedded in commands */
interface ProbeResult {
  hostKey: string
  hostLabel: string
  cwd: string
  os: string
}

async function probeHost(ctx: PluginMainContext): Promise<ProbeResult | null> {
  if (!ctx.isSshSession()) {
    return null
  }
  const command = [
    "printf 'HOST=%s\\n' \"$(hostname 2>/dev/null || uname -n 2>/dev/null || echo unknown)\"",
    "printf 'USER=%s\\n' \"$(id -un 2>/dev/null || echo unknown)\"",
    "printf 'PWD=%s\\n' \"$(pwd 2>/dev/null || echo /)\"",
    "printf 'OS=%s\\n' \"$(uname -srm 2>/dev/null || echo unknown)\""
  ].join('; ')
  const raw = await ctx.execCapture(command)
  const values = new Map<string, string>()
  for (const line of raw.split('\n')) {
    const eq = line.indexOf('=')
    if (eq > 0) {
      values.set(line.slice(0, eq).trim(), line.slice(eq + 1).trim())
    }
  }
  const host = values.get('HOST') || 'unknown'
  const user = values.get('USER') || 'unknown'
  return {
    hostKey: `${user}@${host}`,
    hostLabel: `${user}@${host}`,
    cwd: values.get('PWD') || '/',
    os: values.get('OS') || 'unknown'
  }
}

function ensureHost(hostKey: string, hostLabel: string, ctx: PluginMainContext): HostState {
  let host = hosts.get(hostKey)
  if (!host) {
    const activeId = dataFile?.activeConversationId[hostKey]
    let conversation =
      activeId && dataFile?.conversations[activeId]?.hostKey === hostKey
        ? dataFile.conversations[activeId]
        : null
    if (!conversation) {
      const recent = conversationsForHost(hostKey)
        .slice()
        .sort((a, b) => b.updatedAt - a.updatedAt)[0]
      conversation =
        recent ??
        createConversation(hostKey, hostLabel, '', '', '/')
      if (dataFile) {
        dataFile.conversations[conversation.id] = conversation
        dataFile.activeConversationId[hostKey] = conversation.id
        saveData()
      }
    }
    conversation.hostLabel = hostLabel
    host = {
      hostKey,
      hostLabel,
      conversation,
      phase: 'idle',
      extraAllow: [],
      extraDeny: [],
      pendingApproval: null,
      approvalResolve: null,
      pendingSudo: null,
      sudoResolve: null,
      sudoPassword: null,
      sudoPasswordExpiresAt: 0,
      sudoNopassExpiresAt: 0,
      controller: null,
      connectionId: null,
      runCtx: null,
      stopped: false,
      inRun: false
    }
    hosts.set(hostKey, host)
  } else if (host.conversation) {
    host.conversation.hostLabel = hostLabel
  }
  hostRefCount.set(hostKey, (hostRefCount.get(hostKey) ?? 0) + 1)
  return host
}

function releaseHost(hostKey: string, ctx: PluginMainContext): void {
  const count = (hostRefCount.get(hostKey) ?? 1) - 1
  if (count <= 0) {
    hostRefCount.delete(hostKey)
    const host = hosts.get(hostKey)
    if (host) {
      abortCurrent(host)
      if (host.inRun) {
        pauseHost(host)
      }
      host.extraAllow = []
      host.extraDeny = []
      host.approvalResolve = null
      host.pendingApproval = null
      if (host.sudoResolve) {
        const resolve = host.sudoResolve
        host.sudoResolve = null
        host.pendingSudo = null
        resolve(null)
      }
      clearSudoCache(host)
      hosts.delete(hostKey)
    }
  } else {
    hostRefCount.set(hostKey, count)
  }
}

interface PromptAdditions {
  globalText: string
  hostText: string
  localFiles: string
  projectRules: string
}

function systemPrompt(
  host: HostState,
  additions: PromptAdditions,
  activeTools: PluginApiMethod[]
): string {
  const conv = host.conversation
  const cwd = conv?.cwd || '/'
  const toolDescriptions = activeTools.map((t) => `- ${t.name}: ${t.description}`)
  const base = [
    `You are an AI development agent connected to the remote host "${host.hostLabel}".`,
    `Current working directory: ${cwd}`,
    '',
    'You have access to tools to interact with the environment and system. Call tools whenever needed to gather facts or take actions.',
    '',
    'Available tools:',
    ...toolDescriptions,
    '',
    'Guidelines:',
    '- When asked about the current date or time, call get_current_time to obtain the exact current time.',
    '- Avoid interactive commands (vim, top, less, tail -f). Prefer small, verifiable steps.',
    '- After each command or tool call you see its actual result. Never invent output.',
    '- When the task is complete, reply with a concise plain-text summary; you do not need to call more tools.'
  ]
  const globalText = additions.globalText.trim()
  const hostText = additions.hostText.trim()
  const localFiles = additions.localFiles.trim()
  if (globalText || hostText || localFiles) {
    base.push('', 'Additional prompt instructions and context:', '')
    if (globalText) {
      base.push(`[Global prompt]\n${globalText}`, '')
    }
    if (hostText) {
      base.push(`[Host prompt]\n${hostText}`, '')
    }
    if (localFiles) {
      base.push(localFiles)
    }
  }
  const userRules = (dataFile?.rules ?? '').trim()
  const remote = additions.projectRules.trim()
  if (userRules || remote) {
    base.push('', 'Rules you must follow:', '')
    if (userRules) {
      base.push(`[Rules from WaSSH settings]\n${userRules}`, '')
    }
    if (remote) {
      base.push(`[Project rules from ${PROJECT_RULES_FILE} in the working directory]\n${remote}`)
    }
  }
  return base.join('\n')
}

function stringSetting(settings: Record<string, unknown>, key: string): string {
  return typeof settings[key] === 'string' ? settings[key].trim() : ''
}

async function readLocalPromptFiles(folder: string): Promise<string> {
  if (!folder) {
    return ''
  }
  let names: string[]
  try {
    const entries = await readdir(folder, { withFileTypes: true })
    names = entries
      .filter(
        (entry) =>
          entry.isFile() && PROMPT_FILE_EXTENSIONS.has(extname(entry.name).toLowerCase())
      )
      .map((entry) => entry.name)
      .sort((left, right) => left.localeCompare(right))
  } catch {
    return ''
  }
  const sections = await Promise.all(
    names.map(async (name) => {
      try {
        const content = (await readFile(join(folder, name), 'utf8')).trim()
        return content ? `[Prompt file: ${name}]\n${content}` : ''
      } catch {
        return ''
      }
    })
  )
  return sections.filter(Boolean).join('\n\n')
}

async function readProjectRules(ctx: PluginMainContext): Promise<string> {
  if (!ctx.isSshSession()) {
    return ''
  }
  try {
    return (await ctx.execCapture(`cat ${PROJECT_RULES_FILE} 2>/dev/null; true`)).trim()
  } catch {
    return ''
  }
}

function toApiMessages(conversation: AiAgentConversation): ApiMessage[] {
  const out: ApiMessage[] = []
  for (const msg of conversation.messages) {
    if (msg.role === 'user') {
      out.push({ role: 'user', content: msg.modelText ?? msg.text })
      continue
    }
    if (msg.role === 'assistant') {
      const toolCalls: ApiToolCallMsg[] = (msg.toolCalls ?? []).map((tc) => ({
        id: tc.id,
        name: tc.name || RUN_COMMAND_TOOL_NAME,
        arguments:
          tc.argumentsJson ||
          (tc.name === RUN_COMMAND_TOOL_NAME ? JSON.stringify({ command: tc.command }) : '{}')
      }))
      out.push({
        role: 'assistant',
        content: msg.text ?? '',
        toolCalls: toolCalls.length > 0 ? toolCalls : undefined
      })
      continue
    }
    out.push({ role: 'tool', toolCallId: msg.toolCallId, content: msg.content })
  }
  return out
}

function abortCurrent(host: HostState): void {
  if (host.controller) {
    host.controller.abort()
    host.controller = null
  }
  if (host.connectionId) {
    host.runCtx?.closeSideConnection(host.connectionId)
    host.connectionId = null
  }
}

/** Stop execution because the last viewer closed (dock closed). */
function pauseHost(host: HostState): void {
  abortCurrent(host)
  if (host.conversation) {
    pruneUnresolvedTail(host.conversation)
    persistConversation(host)
  }
  host.pendingApproval = null
  host.approvalResolve = null
  if (host.sudoResolve) {
    const resolve = host.sudoResolve
    host.sudoResolve = null
    host.pendingSudo = null
    resolve(null)
  }
  clearSudoCache(host)
  host.inRun = false
  host.phase = 'paused'
}

function clearSudoCache(host: HostState): void {
  host.sudoPassword = null
  host.sudoPasswordExpiresAt = 0
  host.sudoNopassExpiresAt = 0
}

function commandNeedsSudo(command: string): boolean {
  return SUDO_COMMAND_RE.test(command)
}

function hasCachedSudoPassword(host: HostState): boolean {
  return host.sudoPassword !== null && Date.now() < host.sudoPasswordExpiresAt
}

function hasCachedSudoNopass(host: HostState): boolean {
  return Date.now() < host.sudoNopassExpiresAt
}

async function canSudoWithoutPassword(host: HostState): Promise<boolean> {
  const ctx = host.runCtx
  if (!ctx) {
    return false
  }
  try {
    const out = await ctx.execCapture(
      'sudo -n -v >/dev/null 2>&1; printf "__WASSH_SUDO_NOPASS:%s\\n" "$?"'
    )
    return /__WASSH_SUDO_NOPASS:0\b/.test(out)
  } catch {
    return false
  }
}

async function askSudoPassword(host: HostState, command: string): Promise<string | null> {
  host.pendingSudo = { requestId: randomUUID(), command }
  host.phase = 'ask_sudo'
  pushState(host)
  const password = await new Promise<string | null>((resolve) => {
    host.sudoResolve = resolve
  })
  host.sudoResolve = null
  host.pendingSudo = null
  if (host.inRun && !host.stopped) {
    host.phase = 'running'
    pushState(host)
  }
  return password
}

/**
 * Ensure sudo can authenticate for this command.
 * Returns false if the user cancelled the password prompt.
 */
async function ensureSudoCredentials(host: HostState, command: string): Promise<boolean> {
  if (hasCachedSudoPassword(host) || hasCachedSudoNopass(host)) {
    return true
  }
  if (await canSudoWithoutPassword(host)) {
    host.sudoNopassExpiresAt = Date.now() + SUDO_PASSWORD_CACHE_MS
    return true
  }
  if (!host.inRun || host.stopped) {
    return false
  }
  const password = await askSudoPassword(host, command)
  if (password === null) {
    return false
  }
  host.sudoPassword = password
  host.sudoPasswordExpiresAt = Date.now() + SUDO_PASSWORD_CACHE_MS
  return true
}

function listSetting(settings: Record<string, unknown>, key: string): string[] {
  const value = settings[key]
  return Array.isArray(value) ? value.filter((s): s is string => typeof s === 'string') : []
}

function isToolEnabled(activeTools: PluginApiMethod[], toolName: string): boolean {
  return activeTools.some((tool) => tool.name === toolName)
}

function applyApprovalDecision(host: HostState, subject: string, decision: string): boolean {
  if (decision === 'allowAlways') {
    host.extraAllow.push(subject)
  } else if (decision === 'denyAlways') {
    host.extraDeny.push(subject)
  }
  return decision !== 'deny' && decision !== 'denyAlways'
}

interface ExecResult {
  outcome: 'ok' | 'error' | 'timeout' | 'cancelled'
  content: string
  exitCode: number | null
  truncated: boolean
  pwd: string | null
}

function toolResultMessage(
  toolCallId: string,
  name: string,
  command: string,
  content: string,
  outcome: AiAgentConversationToolMsg['outcome'],
  truncated: boolean
): AiAgentConversationMsg {
  return { role: 'tool', toolCallId, name, command, content, outcome, truncated }
}

async function askApproval(host: HostState, kind: AiAgentApprovalKind, subject: string): Promise<string> {
  host.pendingApproval = {
    requestId: randomUUID(),
    kind,
    subject,
    cwd: host.conversation?.cwd || '/'
  }
  host.phase = 'ask'
  pushState(host)
  const decision = await new Promise<string>((resolve) => {
    host.approvalResolve = resolve
  })
  host.approvalResolve = null
  host.pendingApproval = null
  return decision
}

function parseJsonArgs(jsonStr: string): Record<string, unknown> {
  try {
    const val = JSON.parse(jsonStr)
    return val && typeof val === 'object' ? (val as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}

function extractToolDisplayCommand(name: string, argsJson: string): string {
  const args = parseJsonArgs(argsJson)
  if (name === AI_AGENT_TOOL_RUN_COMMAND) {
    return typeof args.command === 'string' ? args.command : ''
  }
  if (name === AI_AGENT_TOOL_GET_CURRENT_TIME) {
    return 'get_current_time'
  }
  if (name === AI_AGENT_TOOL_WEB_FETCH) {
    return `fetch ${String(args.url || '')}`
  }
  if (name === AI_AGENT_TOOL_WEB_SEARCH) {
    return `search "${String(args.query || '')}"`
  }
  if (name === AI_AGENT_TOOL_REMOTE_FS_READ) {
    return `remote_read ${String(args.path || '')}`
  }
  if (name === AI_AGENT_TOOL_REMOTE_FS_WRITE) {
    return `remote_write ${String(args.path || '')}`
  }
  if (name === AI_AGENT_TOOL_REMOTE_FS_EDIT) {
    return `remote_edit ${String(args.path || '')}`
  }
  if (name === AI_AGENT_TOOL_REMOTE_FS_LIST) {
    return `remote_ls ${String(args.path || '.')}`
  }
  if (name === AI_AGENT_TOOL_REMOTE_FS_DELETE) {
    return `remote_delete ${String(args.path || '')}`
  }
  if (name === AI_AGENT_TOOL_LOCAL_FS_READ) {
    return `local_read ${String(args.path || '')}`
  }
  if (name === AI_AGENT_TOOL_LOCAL_FS_WRITE) {
    return `local_write ${String(args.path || '')}`
  }
  if (name === AI_AGENT_TOOL_LOCAL_FS_EDIT) {
    return `local_edit ${String(args.path || '')}`
  }
  if (name === AI_AGENT_TOOL_LOCAL_FS_LIST) {
    return `local_ls ${String(args.path || '.')}`
  }
  return `${name} ${JSON.stringify(args)}`
}

function toolContentForModel(
  result: ExecResult,
  command: string,
  outcome: AiAgentConversationToolMsg['outcome']
): string {
  if (outcome !== 'ok') {
    if (outcome === 'denied') {
      return 'The command was denied by permission rules.'
    }
    if (outcome === 'cancelled') {
      return 'The command was cancelled by the user.'
    }
    if (outcome === 'timeout') {
      return `${result.content}\n[command timed out]`.trim()
    }
    return result.content || 'Command failed.'
  }
  const parts = [`$ ${command}`, result.content]
  if (result.exitCode !== null && result.exitCode !== 0) {
    parts.push(`[exit code: ${result.exitCode}]`)
  }
  if (result.truncated) {
    parts.push('[output truncated]')
  }
  return parts.filter((p) => p.length > 0).join('\n')
}

async function runLoop(host: HostState, tab: TabRuntime): Promise<void> {
  const ctx = tab.ctx
  const conv = host.conversation
  if (!conv) {
    return
  }
  const provider = findProvider(conv.activeProviderId)
  if (!provider) {
    pushToast(host, 'error', 'No model provider configured — configure one in Options.')
    return
  }
  if (!conv.activeModel) {
    pushToast(host, 'error', 'Pick a model for this conversation first.')
    return
  }
  // Read the key from the encrypted vault (safeStorage/DPAPI) for every run so
  // keys survive restarts without any renderer round-trip or in-memory cache.
  const apiKey = ctx.getSecret(aiAgentVaultId(provider.id)) ?? ''
  if (!apiKey && provider.protocol === AI_AGENT_PROTOCOL_ANTHROPIC) {
    pushToast(host, 'error', `No API key set for provider "${provider.name}".`)
    return
  }
  const settings = ctx.getSettings()
  const hostAllow = listSetting(settings, AI_AGENT_SETTING_HOST_ALLOW_RULES)
  const hostDeny = listSetting(settings, AI_AGENT_SETTING_HOST_DENY_RULES)
  const appAllow = listSetting(settings, AI_AGENT_SETTING_DEFAULT_ALLOW_RULES)
  const appDeny = listSetting(settings, AI_AGENT_SETTING_DEFAULT_DENY_RULES)
  const projectRules = await readProjectRules(ctx)
  const promptAdditions: PromptAdditions = {
    globalText: stringSetting(settings, AI_AGENT_SETTING_GLOBAL_PROMPT),
    hostText: stringSetting(settings, AI_AGENT_SETTING_HOST_PROMPT),
    localFiles: await readLocalPromptFiles(
      stringSetting(settings, AI_AGENT_SETTING_PROMPT_FILES_FOLDER)
    ),
    projectRules
  }

  const activeTools: PluginApiMethod[] = [
    TOOL_DEF_RUN_COMMAND,
    ...allowedGroupMethods(PLUGIN_ID_AI_AGENT, AI_AGENT_GROUP_WEB_ACCESS, AI_AGENT_WEB_ACCESS_METHODS, settings),
    ...allowedGroupMethods(PLUGIN_ID_AI_AGENT, AI_AGENT_GROUP_WEB_SEARCH, AI_AGENT_WEB_SEARCH_METHODS, settings),
    ...allowedGroupMethods(PLUGIN_ID_AI_AGENT, AI_AGENT_GROUP_REMOTE_FS, AI_AGENT_REMOTE_FS_METHODS, settings),
    ...allowedGroupMethods(PLUGIN_ID_AI_AGENT, AI_AGENT_GROUP_LOCAL_FS, AI_AGENT_LOCAL_FS_METHODS, settings),
    ...(isMethodAllowed(settings, PLUGIN_ID_AI_AGENT, AI_AGENT_DATETIME_METHOD) ? [AI_AGENT_DATETIME_METHOD] : []),
    ...allExternalApiTools(ctx, settings)
  ]

  host.runCtx = ctx
  host.inRun = true
  host.stopped = false
  host.phase = 'running'
  pushState(host)

  let partialText = ''
  try {
    for (let step = 0; step < MAX_RUN_STEPS && host.inRun && !host.stopped; step += 1) {
      const controller = new AbortController()
      host.controller = controller
      partialText = ''
      const result = await complete({
        baseUrl: provider.baseUrl,
        apiKey,
        protocol: provider.protocol,
        model: conv.activeModel,
        system: systemPrompt(host, promptAdditions, activeTools),
        messages: toApiMessages(conv),
        tools: activeTools,
        maxTokens: MAX_TOKENS,
        signal: controller.signal,
        onDelta: (text) => {
          partialText += text
          pushDelta(host, text)
        }
      })
      host.controller = null
      if (!host.inRun || host.stopped) {
        conv.messages.push({
          role: 'assistant',
          text: partialText || result.text,
          providerId: provider.id,
          model: conv.activeModel,
          stopped: true
        })
        pushState(host)
        persistConversation(host)
        break
      }
      conv.messages.push({
        role: 'assistant',
        text: result.text,
        providerId: provider.id,
        model: conv.activeModel,
        toolCalls: result.toolCalls.map((tc) => ({
          id: tc.id,
          name: tc.name,
          command: extractToolDisplayCommand(tc.name, tc.arguments),
          argumentsJson: tc.arguments
        }))
      })
      pushState(host)
      persistConversation(host)

      if (result.toolCalls.length === 0) {
        break
      }
      let keepRunning = true
      for (const tc of result.toolCalls) {
        if (!host.inRun || host.stopped) {
          keepRunning = false
          break
        }

        const toolName = tc.name || RUN_COMMAND_TOOL_NAME
        const toolArgs = parseJsonArgs(tc.arguments)
        const displayCommand = extractToolDisplayCommand(toolName, tc.arguments)

        if (!isToolEnabled(activeTools, toolName)) {
          conv.messages.push(
            toolResultMessage(
              tc.id,
              toolName,
              displayCommand,
              `Tool "${toolName}" is disabled for this conversation.`,
              'denied',
              false
            )
          )
          pushState(host)
          persistConversation(host)
          continue
        }

        if (toolName === AI_AGENT_TOOL_RUN_COMMAND) {
          const command = typeof toolArgs.command === 'string' ? toolArgs.command.trim() : ''
          if (!command) {
            conv.messages.push(
              toolResultMessage(
                tc.id,
                toolName,
                tc.arguments,
                'Model sent an empty command.',
                'error',
                false
              )
            )
            pushState(host)
            continue
          }

          const decision = decideCommand(
            command,
            hostAllow,
            hostDeny,
            appAllow,
            appDeny,
            host.extraAllow,
            host.extraDeny
          )
          let action: string = decision
          if (decision === 'ask') {
            action = await askApproval(host, 'command', command)
            if (!host.inRun) {
              keepRunning = false
              break
            }
          }
          if (!applyApprovalDecision(host, command, action)) {
            conv.messages.push(toolResultMessage(tc.id, toolName, command, '', 'denied', false))
            pushState(host)
            persistConversation(host)
            continue
          }

          let sudoStdin: string | undefined
          if (commandNeedsSudo(command)) {
            const ok = await ensureSudoCredentials(host, command)
            if (!host.inRun) {
              keepRunning = false
              break
            }
            if (!ok) {
              conv.messages.push(
                toolResultMessage(
                  tc.id,
                  toolName,
                  command,
                  'Sudo password prompt was cancelled.',
                  'cancelled',
                  false
                )
              )
              pushState(host)
              persistConversation(host)
              continue
            }
            sudoStdin = hasCachedSudoPassword(host) ? (host.sudoPassword ?? '') : ''
          }

          const execResult = (await ctx.callPluginApi(PLUGIN_ID_AI_AGENT, AI_AGENT_TOOL_RUN_COMMAND, {
            command,
            sudoStdin
          })) as ExecResult
          if (!host.inRun) {
            keepRunning = false
            break
          }
          const outcome: AiAgentConversationToolMsg['outcome'] =
            execResult.outcome === 'ok'
              ? 'ok'
              : execResult.outcome === 'timeout'
                ? 'timeout'
                : execResult.outcome === 'cancelled'
                  ? 'cancelled'
                  : 'error'
          conv.messages.push(
            toolResultMessage(
              tc.id,
              toolName,
              command,
              toolContentForModel(execResult, command, outcome),
              outcome,
              execResult.truncated
            )
          )
          pushState(host)
          persistConversation(host)
          continue
        }

        const target = resolveToolTarget(toolName)
        const permission = resolvePermission(settings, target.pluginId, target.method)
        if (permission === 'deny') {
          conv.messages.push(
            toolResultMessage(tc.id, toolName, displayCommand, 'This action is not permitted.', 'denied', false)
          )
          pushState(host)
          persistConversation(host)
          continue
        }
        if (permission === 'ask') {
          const decision = await askApproval(host, 'permission', displayCommand)
          if (!host.inRun) {
            keepRunning = false
            break
          }
          if (decision === 'allowAlways') {
            ctx.setSettingValue(permissionSettingKey(target.pluginId, target.method), 'allow')
          }
          if (decision === 'deny' || decision === 'denyAlways') {
            conv.messages.push(
              toolResultMessage(tc.id, toolName, displayCommand, 'Action denied by user.', 'denied', false)
            )
            pushState(host)
            persistConversation(host)
            continue
          }
        }

        let toolOutput = ''
        let toolOutcome: AiAgentToolOutcome = 'ok'
        try {
          const raw = await ctx.callPluginApi(target.pluginId, target.method, toolArgs)
          toolOutput = typeof raw === 'string' ? raw : JSON.stringify(raw)
          if (toolOutput.length > MAX_PLUGIN_API_RESULT_CHARS) {
            toolOutput = `${toolOutput.slice(0, MAX_PLUGIN_API_RESULT_CHARS)}\n\n[Result truncated at ${MAX_PLUGIN_API_RESULT_CHARS} characters]`
          }
        } catch (err) {
          toolOutput = `Error running tool "${toolName}": ${err instanceof Error ? err.message : String(err)}`
          toolOutcome = 'error'
        }

        if (!host.inRun) {
          keepRunning = false
          break
        }

        conv.messages.push(
          toolResultMessage(tc.id, toolName, displayCommand, toolOutput, toolOutcome, false)
        )
        pushState(host)
        persistConversation(host)
      }
      if (!keepRunning) {
        break
      }
    }
  } catch (err) {
    const aborted = err instanceof Error && err.name === 'AbortError'
    if (aborted && host.stopped && host.inRun && conv && partialText) {
      conv.messages.push({
        role: 'assistant',
        text: partialText,
        providerId: provider.id,
        model: conv.activeModel,
        stopped: true
      })
      pushState(host)
      persistConversation(host)
    }
    if (host.inRun && !host.stopped && !aborted) {
      host.lastError = err instanceof Error ? err.message : String(err)
      pushToast(host, 'error', host.lastError)
    }
  } finally {
    host.controller = null
    if (host.inRun) {
      host.inRun = false
      host.runCtx = null
      // A user-initiated stop parks the run in 'paused' so the view can offer
      // a Continue action; anything else returns to idle.
      host.phase = host.stopped ? 'paused' : 'idle'
    }
    pushState(host)
    persistConversation(host)
  }
}

async function execCommand(
  host: HostState,
  command: string,
  sudoStdin?: string
): Promise<ExecResult> {
  const ctx = host.runCtx
  if (!ctx) {
    return { outcome: 'error', content: 'Session is not connected', exitCode: null, truncated: false, pwd: null }
  }
  const token = randomUUID().slice(0, 8)
  const statusTag = `__WASSH_STATUS_${token}`
  const pwdTag = `__WASSH_PWD_${token}`
  const cwd = host.conversation?.cwd || '/'
  const sudoPreamble =
    sudoStdin === undefined
      ? []
      : [
          // Password is read from stdin (written by the main process) — never embedded in argv.
          'IFS= read -r __wassh_sudo_pw || true',
          'if [ -n "${__wassh_sudo_pw}" ]; then',
          '  printf \'%s\\n\' "${__wassh_sudo_pw}" | sudo -S -p \'\' -v >/dev/null 2>&1',
          '  __wassh_sudo_auth=$?',
          'else',
          '  sudo -n -v >/dev/null 2>&1',
          '  __wassh_sudo_auth=$?',
          'fi',
          'unset __wassh_sudo_pw',
          'if [ "${__wassh_sudo_auth}" -ne 0 ]; then',
          `  printf '%s\\n' '${SUDO_AUTH_FAILED_MARKER}'`,
          '  __wassh_status=1',
          'else'
        ]
  const sudoClose = sudoStdin === undefined ? [] : ['fi']
  const wrapped = [
    `cd ${shellQuote(cwd)} >/dev/null 2>&1 || true`,
    '__wassh_status=0',
    ...sudoPreamble,
    ...(sudoStdin === undefined
      ? [command, '__wassh_status=$?']
      : [
          `  ${command}`,
          '  __wassh_status=$?',
          ...sudoClose
        ]),
    `printf '\\n${statusTag}:%s__\\n' "$__wassh_status"`,
    `printf '${pwdTag}:%s__\\n' "$(pwd 2>/dev/null)"`
  ].join('\n')

  return await new Promise<ExecResult>((resolve) => {
    let settled = false
    let raw = ''
    let truncated = false
    let timer: ReturnType<typeof setTimeout> | null = null

    const finish = (outcome: ExecResult['outcome'], reason?: string): void => {
      if (settled) {
        return
      }
      settled = true
      if (timer) {
        clearTimeout(timer)
      }
      if (host.connectionId) {
        host.runCtx?.closeSideConnection(host.connectionId)
        host.connectionId = null
      }
      const statusRe = new RegExp(`\\n?${statusTag}:(\\d+)__`, 'g')
      const pwdRe = new RegExp(`\\n?${pwdTag}:(.*?)__`, 'g')
      let exitCode: number | null = null
      let pwd: string | null = null
      const statusMatch = statusRe.exec(raw)
      if (statusMatch) {
        exitCode = Number(statusMatch[1])
      }
      const pwdMatch = pwdRe.exec(raw)
      if (pwdMatch) {
        pwd = pwdMatch[1]
      }
      let content = raw.replace(new RegExp(`\\n?${statusTag}:\\d+__`, 'g'), '')
      content = content.replace(new RegExp(`\\n?${pwdTag}:[^\\n]*__`, 'g'), '')
      if (host.stopped && outcome === 'ok') {
        outcome = 'cancelled'
      } else if (outcome === 'ok' && statusMatch === null) {
        outcome = 'error'
        reason = 'Command produced no exit marker'
      }
      if (reason) {
        content = content.length > 0 ? `${content}\n[exec error: ${reason}]` : `[exec error: ${reason}]`
      }
      resolve({
        outcome,
        content,
        exitCode,
        truncated,
        pwd
      })
    }

    void ctx
      .openSideConnection({ kind: 'ssh-exec', command: wrapped })
      .then((connectionId) => {
        if (settled) {
          ctx.closeSideConnection(connectionId)
          return
        }
        host.connectionId = connectionId
        if (sudoStdin !== undefined) {
          ctx.writeSideConnection(connectionId, `${sudoStdin}\n`)
        }
        const offData = ctx.onSideData(connectionId, (chunk) => {
          if (settled) {
            return
          }
          raw += chunk
          const limit = MAX_OUTPUT_CHARS + MARKER_TAIL_CHARS
          if (raw.length > limit) {
            raw = raw.slice(-limit)
            truncated = true
          }
        })
        const offClose = ctx.onSideClosed(connectionId, (error) => {
          offData()
          finish(error ? 'error' : 'ok', error)
        })
        timer = setTimeout(() => finish('timeout'), EXEC_TIMEOUT_MS)
      })
      .catch((err) => {
        finish('error', err instanceof Error ? err.message : String(err))
      })
  })
}

async function setupForTab(ctx: PluginMainContext, forceProbe: boolean): Promise<void> {
  loadData(ctx)
  let probe: ProbeResult | null = null
  if (forceProbe) {
    probe = await probeHost(ctx).catch(() => null)
  }
  const isSsh = ctx.isSshSession()
  const hostKey = probe ? probe.hostKey : `tab:${ctx.tabId}`
  const hostLabel = probe ? probe.hostLabel : isSsh ? 'SSH session' : 'Session'

  const existing = tabs.get(ctx.tabId)
  if (existing && existing.hostKey === hostKey) {
    let host = hosts.get(hostKey)
    if (!host) {
      host = ensureHost(hostKey, hostLabel, ctx)
    }
    pushState(host)
    void refreshAllModels(ctx)
    return
  }
  if (existing) {
    tabs.delete(ctx.tabId)
    releaseHost(existing.hostKey, ctx)
  }
  const tab: TabRuntime = { ctx, hostKey, terminalTail: '' }
  tabs.set(ctx.tabId, tab)
  ctx.registerStreamHandler('observe', 'inbound', (data) => {
    tab.terminalTail = (tab.terminalTail + data).slice(-TERMINAL_TAIL_CHARS)
    return data
  })
  const host = ensureHost(hostKey, hostLabel, ctx)
  if (probe) {
    if (host.conversation) {
      host.conversation.cwd = probe.cwd
    }
  }
  pushState(host)
  void refreshAllModels(ctx)
}

async function refreshAllModels(ctx: PluginMainContext): Promise<void> {
  const providers = dataFile?.providers ?? []
  await Promise.all(providers.map((provider) => handleRefreshModels(ctx, provider.id)))
}

function hostForCtx(ctx: PluginMainContext): HostState | null {
  const tab = tabs.get(ctx.tabId)
  if (!tab) {
    return null
  }
  return hosts.get(tab.hostKey) ?? null
}

/**
 * Query a provider's own API for available models, update its models array
 * with the reported models, and push state to the client. Refresh runs quietly
 * without popup messages.
 */
async function handleRefreshModels(
  ctx: PluginMainContext,
  providerId: string
): Promise<void> {
  const host = hostForCtx(ctx)
  const provider = findProvider(providerId)
  if (!provider) {
    if (host) {
      pushState(host)
    }
    return
  }
  const apiKey = ctx.getSecret(aiAgentVaultId(provider.id)) ?? ''
  if (!apiKey && provider.protocol === AI_AGENT_PROTOCOL_ANTHROPIC) {
    if (host) {
      pushState(host)
    }
    return
  }
  try {
    const fetched = await listModels({
      baseUrl: provider.baseUrl,
      apiKey,
      protocol: provider.protocol
    })
    if (fetched.length > 0) {
      provider.models = fetched.slice().sort((a, b) =>
        a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' })
      )
      saveData()
    }
  } catch {
    // Model refresh errors leave the configured model list unchanged.
  } finally {
    if (host) {
      pushState(host)
    }
  }
}

function formatAttachmentForModel(attachment: AiAgentChatAttachment): string {
  if (attachment.binary || attachment.text == null) {
    return `--- file: ${attachment.name} (binary, content omitted) ---`
  }
  const body = attachment.truncated
    ? `${attachment.text}\n…[truncated]`
    : attachment.text
  return `--- file: ${attachment.name} ---\n${body}\n--- end file: ${attachment.name} ---`
}

function chatMessageWithContext(
  tab: TabRuntime,
  text: string,
  attachTerminal: boolean,
  attachments: AiAgentChatAttachment[] | undefined
): string {
  const parts: string[] = []
  if (text) {
    parts.push(text)
  }
  if (attachments && attachments.length > 0) {
    parts.push(attachments.map(formatAttachmentForModel).join('\n\n'))
  }
  if (attachTerminal && tab.terminalTail) {
    const tail = tab.terminalTail.slice(-TERMINAL_CONTEXT_EXCERPT_CHARS)
    parts.push(`${TERMINAL_CONTEXT_MARKER.trim()}\n${tail}`)
  }
  return parts.join('\n\n')
}

function isChatAttachment(value: unknown): value is AiAgentChatAttachment {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return false
  }
  const attachment = value as Record<string, unknown>
  return (
    typeof attachment.name === 'string' &&
    typeof attachment.truncated === 'boolean' &&
    typeof attachment.binary === 'boolean' &&
    (attachment.mimeType === undefined || typeof attachment.mimeType === 'string') &&
    (attachment.text === undefined || typeof attachment.text === 'string')
  )
}

function isRendererMessage(payload: unknown): payload is AiAgentRendererMessage {
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
    return false
  }
  const message = payload as Record<string, unknown>
  if (typeof message.type !== 'string') {
    return false
  }
  switch (message.type) {
    case 'sync':
    case 'probe':
    case 'stop':
    case 'resume':
    case 'discardPaused':
      return true
    case 'providersChanged':
      return Array.isArray(message.providers) && message.providers.every(isProviderConfig)
    case 'checkProvider':
      return isProviderConfig(message.provider)
    case 'refreshProvider':
      return isProviderConfig(message.provider)
    case 'refreshModels':
      return typeof message.providerId === 'string'
    case 'openChat':
    case 'deleteChat':
      return typeof message.conversationId === 'string'
    case 'rulesChanged':
      return typeof message.rules === 'string'
    case 'select':
    case 'newChat':
      return typeof message.providerId === 'string' && typeof message.model === 'string'
    case 'approval':
      return (
        typeof message.requestId === 'string' &&
        (message.decision === 'allow' ||
          message.decision === 'deny' ||
          message.decision === 'allowAlways' ||
          message.decision === 'denyAlways')
      )
    case 'sudoPassword':
      return (
        typeof message.requestId === 'string' &&
        (typeof message.password === 'string' || message.password === null)
      )
    case 'chat':
      return (
        typeof message.providerId === 'string' &&
        typeof message.model === 'string' &&
        typeof message.text === 'string' &&
        (message.attachTerminal === undefined || typeof message.attachTerminal === 'boolean') &&
        (message.attachments === undefined ||
          (Array.isArray(message.attachments) && message.attachments.every(isChatAttachment)))
      )
    default:
      return false
  }
}

async function handleRendererMessage(
  ctx: PluginMainContext,
  payload: unknown
): Promise<unknown> {
  if (!isRendererMessage(payload)) {
    throw new Error('Invalid AI agent renderer message')
  }
  if (payload.type === 'sync') {
    const host = hostForCtx(ctx)
    if (host) {
      pushState(host)
    } else {
      await setupForTab(ctx, true)
    }
    return
  }
  if (payload.type === 'probe') {
    await setupForTab(ctx, true)
    return
  }
  if (payload.type === 'providersChanged') {
    if (dataFile) {
      dataFile.providers = normalizeProviders(payload.providers)
      saveData()
    }
    const host = hostForCtx(ctx)
    if (host) {
      pushState(host)
      void refreshAllModels(ctx)
    }
    return
  }
  if (payload.type === 'checkProvider') {
    try {
      const models = await listModels({
        baseUrl: payload.provider.baseUrl,
        apiKey: ctx.getSecret(aiAgentVaultId(payload.provider.id)) ?? '',
        protocol: payload.provider.protocol
      })
      return { ok: true, models }
    } catch (error) {
      return {
        ok: false,
        models: [],
        message: error instanceof Error ? error.message : String(error)
      }
    }
  }
  if (payload.type === 'refreshProvider') {
    try {
      const models = await listModels({
        baseUrl: payload.provider.baseUrl,
        apiKey: ctx.getSecret(aiAgentVaultId(payload.provider.id)) ?? '',
        protocol: payload.provider.protocol
      })
      return { ok: true, models }
    } catch (error) {
      return {
        ok: false,
        models: [],
        message: error instanceof Error ? error.message : String(error)
      }
    }
  }
  if (payload.type === 'refreshModels') {
    await handleRefreshModels(ctx, payload.providerId)
    return
  }
  if (payload.type === 'rulesChanged') {
    if (dataFile) {
      dataFile.rules = payload.rules
      saveData()
    }
    const host = hostForCtx(ctx)
    if (host) {
      pushState(host)
    }
    return
  }

  const tab = tabs.get(ctx.tabId)
  const host = tab ? hosts.get(tab.hostKey) ?? null : null
  if (!tab || !host || !host.conversation) {
    return
  }

  if (payload.type === 'approval') {
    if (host.phase === 'ask' && host.pendingApproval?.requestId === payload.requestId) {
      const resolve = host.approvalResolve
      if (resolve) {
        host.approvalResolve = null
        resolve(payload.decision)
        if (host.inRun) {
          host.phase = 'running'
        }
        pushState(host)
      }
    }
    return
  }
  if (payload.type === 'sudoPassword') {
    if (host.phase === 'ask_sudo' && host.pendingSudo?.requestId === payload.requestId) {
      const resolve = host.sudoResolve
      if (resolve) {
        host.sudoResolve = null
        resolve(payload.password)
      }
    }
    return
  }
  if (payload.type === 'stop') {
    host.stopped = true
    if (host.controller) {
      host.controller.abort()
    }
    if (host.phase === 'ask' && host.approvalResolve) {
      const resolve = host.approvalResolve
      host.approvalResolve = null
      resolve('deny')
    }
    if (host.phase === 'ask_sudo' && host.sudoResolve) {
      const resolve = host.sudoResolve
      host.sudoResolve = null
      host.pendingSudo = null
      resolve(null)
    }
    return
  }
  if (payload.type === 'select') {
    host.conversation.activeProviderId = payload.providerId
    host.conversation.activeModel = payload.model
    if (payload.providerId && payload.model) {
      host.conversation.lastSelectedModelByProvider[payload.providerId] = payload.model
    }
    persistConversation(host)
    pushState(host)
    return
  }
  if (payload.type === 'newChat') {
    if (host.phase === 'running' || host.phase === 'ask' || host.phase === 'ask_sudo') {
      pushToast(host, 'info', 'The agent is busy — stop it or wait for the current run.')
      return
    }
    if (host.conversation.messages.length === 0) {
      host.conversation.activeProviderId = payload.providerId
      host.conversation.activeModel = payload.model
      if (payload.providerId && payload.model) {
        host.conversation.lastSelectedModelByProvider[payload.providerId] = payload.model
      }
      host.conversation.title = AI_AGENT_DEFAULT_CHAT_TITLE
      persistConversation(host)
      pushState(host)
      return
    }
    persistConversation(host)
    host.conversation = createConversation(
      host.hostKey,
      host.hostLabel,
      payload.providerId,
      payload.model,
      host.conversation.cwd || '/'
    )
    persistConversation(host)
    pushState(host)
    return
  }
  if (payload.type === 'openChat') {
    if (host.phase === 'running' || host.phase === 'ask' || host.phase === 'ask_sudo') {
      pushToast(host, 'info', 'The agent is busy — stop it or wait for the current run.')
      return
    }
    const next = dataFile?.conversations[payload.conversationId]
    if (!next || next.hostKey !== host.hostKey) {
      return
    }
    persistConversation(host)
    host.conversation = next
    if (dataFile) {
      dataFile.activeConversationId[host.hostKey] = next.id
      saveData()
    }
    pushState(host)
    return
  }
  if (payload.type === 'deleteChat') {
    if (host.phase === 'running' || host.phase === 'ask' || host.phase === 'ask_sudo') {
      pushToast(host, 'info', 'The agent is busy — stop it or wait for the current run.')
      return
    }
    if (!dataFile) {
      return
    }
    const target = dataFile.conversations[payload.conversationId]
    if (!target || target.hostKey !== host.hostKey) {
      return
    }
    delete dataFile.conversations[payload.conversationId]
    if (host.conversation.id === payload.conversationId) {
      const fallback =
        conversationsForHost(host.hostKey)
          .slice()
          .sort((a, b) => b.updatedAt - a.updatedAt)[0] ??
        createConversation(
          host.hostKey,
          host.hostLabel,
          host.conversation.activeProviderId,
          host.conversation.activeModel,
          host.conversation.cwd || '/'
        )
      host.conversation = fallback
      dataFile.conversations[fallback.id] = fallback
      dataFile.activeConversationId[host.hostKey] = fallback.id
    }
    saveData()
    pushState(host)
    return
  }
  if (payload.type === 'chat') {
    if (!ctx.isSshSession()) {
      pushToast(host, 'error', 'The AI agent needs an SSH session to run commands.')
      return
    }
    if (host.phase === 'running' || host.phase === 'ask' || host.phase === 'ask_sudo') {
      pushToast(host, 'info', 'The agent is busy — stop it or wait for the current run.')
      return
    }
    pruneUnresolvedTail(host.conversation)
    host.conversation.activeProviderId = payload.providerId
    host.conversation.activeModel = payload.model
    if (payload.providerId && payload.model) {
      host.conversation.lastSelectedModelByProvider[payload.providerId] = payload.model
    }
    const displayText = payload.text.trim()
    const attachments = Array.isArray(payload.attachments) ? payload.attachments : []
    const modelText = chatMessageWithContext(
      tab,
      displayText,
      payload.attachTerminal === true,
      attachments
    )
    if (!modelText) {
      return
    }
    host.conversation.messages.push({
      role: 'user',
      text: displayText || (attachments.length > 0 ? '(attached files)' : ''),
      modelText,
      usedTerminalContext: payload.attachTerminal === true,
      attachedFiles: attachments.map((a) => a.name)
    })
    persistConversation(host)
    void runLoop(host, tab)
    return
  }
  if (payload.type === 'resume') {
    if (host.phase !== 'idle' && host.phase !== 'paused') {
      return
    }
    pruneUnresolvedTail(host.conversation)
    host.phase = 'idle'
    void runLoop(host, tab)
  }
}

/**
 * Handle `ctx.callPluginApi(PLUGIN_ID_AI_AGENT, method, params)` for every
 * method AI Agent declares in `contributes.api` - the same execution path
 * used whether the call originates from the LLM tool-dispatch loop (which
 * gates permissions before calling this) or, in principle, another plugin.
 * Performs no permission checks itself; see `AI_AGENT_API_METHODS` doc.
 */
async function handleApiCall(
  ctx: PluginMainContext,
  method: string,
  params: unknown
): Promise<unknown> {
  const args = params && typeof params === 'object' ? (params as Record<string, unknown>) : {}

  if (method === AI_AGENT_TOOL_RUN_COMMAND) {
    const host = hostForCtx(ctx)
    if (!host) {
      throw new Error('No active AI Agent conversation on this tab')
    }
    const command = typeof args.command === 'string' ? args.command : ''
    const sudoStdin = typeof args.sudoStdin === 'string' ? args.sudoStdin : undefined
    const result = await execCommand(host, command, sudoStdin)
    if (result.content.includes(SUDO_AUTH_FAILED_MARKER)) {
      clearSudoCache(host)
    } else if (sudoStdin !== undefined && result.outcome === 'ok') {
      if (hasCachedSudoPassword(host)) {
        host.sudoPasswordExpiresAt = Date.now() + SUDO_PASSWORD_CACHE_MS
      } else {
        host.sudoNopassExpiresAt = Date.now() + SUDO_PASSWORD_CACHE_MS
      }
    }
    if (result.pwd && host.conversation) {
      host.conversation.cwd = result.pwd
    }
    return result
  }
  if (method === AI_AGENT_TOOL_GET_CURRENT_TIME) {
    return executeDateTime()
  }
  if (method === AI_AGENT_TOOL_WEB_FETCH) {
    const maxChars = typeof args.maxChars === 'number' ? args.maxChars : undefined
    return executeWebFetch(String(args.url || ''), maxChars)
  }
  if (method === AI_AGENT_TOOL_WEB_SEARCH) {
    const settings = ctx.getSettings()
    const provider =
      typeof settings[AI_AGENT_SETTING_WEB_SEARCH_PROVIDER] === 'string'
        ? (settings[AI_AGENT_SETTING_WEB_SEARCH_PROVIDER] as string)
        : AI_AGENT_DEFAULT_WEB_SEARCH_PROVIDER
    const apiKey =
      typeof settings[AI_AGENT_SETTING_WEB_SEARCH_API_KEY] === 'string'
        ? (settings[AI_AGENT_SETTING_WEB_SEARCH_API_KEY] as string)
        : ''
    const limit = typeof args.limit === 'number' ? args.limit : undefined
    return executeWebSearch(String(args.query || ''), provider, apiKey, limit)
  }
  if (method === AI_AGENT_TOOL_REMOTE_FS_READ) {
    const maxChars = typeof args.maxChars === 'number' ? args.maxChars : undefined
    return executeRemoteFsRead(ctx, String(args.path || ''), maxChars)
  }
  if (method === AI_AGENT_TOOL_REMOTE_FS_WRITE) {
    return executeRemoteFsWrite(ctx, String(args.path || ''), String(args.content || ''))
  }
  if (method === AI_AGENT_TOOL_REMOTE_FS_EDIT) {
    return executeRemoteFsEdit(ctx, String(args.path || ''), String(args.oldText || ''), String(args.newText || ''))
  }
  if (method === AI_AGENT_TOOL_REMOTE_FS_LIST) {
    return executeRemoteFsList(ctx, typeof args.path === 'string' ? args.path : '.')
  }
  if (method === AI_AGENT_TOOL_REMOTE_FS_DELETE) {
    return executeRemoteFsDelete(ctx, String(args.path || ''))
  }
  if (method === AI_AGENT_TOOL_LOCAL_FS_READ) {
    const maxChars = typeof args.maxChars === 'number' ? args.maxChars : undefined
    return executeLocalFsRead(String(args.path || ''), maxChars)
  }
  if (method === AI_AGENT_TOOL_LOCAL_FS_WRITE) {
    return executeLocalFsWrite(String(args.path || ''), String(args.content || ''))
  }
  if (method === AI_AGENT_TOOL_LOCAL_FS_EDIT) {
    return executeLocalFsEdit(String(args.path || ''), String(args.oldText || ''), String(args.newText || ''))
  }
  if (method === AI_AGENT_TOOL_LOCAL_FS_LIST) {
    return executeLocalFsList(String(args.path || '.'))
  }
  throw new Error(`Unknown AI Agent API method: ${method}`)
}

export const aiAgentMain: PluginMainModule = {
  async onActivate(ctx) {
    await setupForTab(ctx, true)
  },
  onDeactivate(ctx) {
    const tab = tabs.get(ctx.tabId)
    if (tab) {
      tabs.delete(ctx.tabId)
      releaseHost(tab.hostKey, ctx)
    }
  },
  async onMessage(ctx, payload) {
    return handleRendererMessage(ctx, payload)
  },
  onApiCall: handleApiCall
}
