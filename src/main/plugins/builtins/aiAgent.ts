import { randomUUID } from 'crypto'
import {
  AI_AGENT_DATA_VERSION,
  AI_AGENT_PROTOCOL_ANTHROPIC,
  AI_AGENT_PROTOCOL_OPENAI,
  AI_AGENT_SETTING_DEFAULT_ALLOW_RULES,
  AI_AGENT_SETTING_DEFAULT_DENY_RULES,
  AI_AGENT_SETTING_HOST_ALLOW_RULES,
  AI_AGENT_SETTING_HOST_DENY_RULES,
  aiAgentVaultId,
  type AiAgentApprovalRequest,
  type AiAgentConversation,
  type AiAgentConversationMsg,
  type AiAgentConversationToolMsg,
  type AiAgentDataFile,
  type AiAgentProviderConfig,
  type AiAgentRendererMessage,
  type AiAgentRunPhase,
  type AiAgentStateSnapshot
} from '../../../shared/plugins'
import type { PluginMainContext, PluginMainModule } from '../PluginHost'
import { decideCommand } from '../aiAgent/permissions'
import {
  complete,
  RUN_COMMAND_TOOL_NAME,
  type ApiMessage,
  type ApiToolCallMsg
} from '../aiAgent/providers'

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

function asDataFile(raw: unknown): AiAgentDataFile {
  const fallback: AiAgentDataFile = {
    version: AI_AGENT_DATA_VERSION,
    providers: [],
    conversations: {},
    rules: ''
  }
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return fallback
  }
  const src = raw as AiAgentDataFile
  const providers: AiAgentProviderConfig[] = Array.isArray(src.providers)
    ? src.providers
        .filter(
          (p): p is AiAgentProviderConfig =>
            !!p && typeof p === 'object' && typeof p.id === 'string' && typeof p.name === 'string'
        )
        .map((p) => ({
          id: p.id,
          name: p.name,
          protocol:
            p.protocol === AI_AGENT_PROTOCOL_ANTHROPIC
              ? AI_AGENT_PROTOCOL_ANTHROPIC
              : AI_AGENT_PROTOCOL_OPENAI,
          baseUrl: typeof p.baseUrl === 'string' ? p.baseUrl : '',
          models: Array.isArray(p.models) ? p.models.filter((m) => typeof m === 'string') : []
        }))
    : []
  const conversations: Record<string, AiAgentConversation> =
    src.conversations && typeof src.conversations === 'object' && !Array.isArray(src.conversations)
      ? src.conversations
      : {}
  return {
    version: AI_AGENT_DATA_VERSION,
    providers,
    conversations,
    rules: typeof src.rules === 'string' ? src.rules : ''
  }
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
    runPhase: host.phase,
    hostKey: host.hostKey,
    hostLabel: host.hostLabel,
    ssh,
    pendingApproval: host.pendingApproval,
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
  if (!host.conversation) {
    return
  }
  trimHistory(host.conversation)
  if (dataFile) {
    dataFile.conversations[host.hostKey] = host.conversation
  }
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
    const stored = dataFile?.conversations[hostKey]
    const conversation: AiAgentConversation = stored ?? {
      version: AI_AGENT_DATA_VERSION,
      activeProviderId: '',
      activeModel: '',
      hostLabel,
      cwd: '/',
      messages: []
    }
    host = {
      hostKey,
      hostLabel,
      conversation,
      phase: 'idle',
      extraAllow: [],
      extraDeny: [],
      pendingApproval: null,
      approvalResolve: null,
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
      hosts.delete(hostKey)
    }
  } else {
    hostRefCount.set(hostKey, count)
  }
}

function systemPrompt(host: HostState, projectRules: string): string {
  const conv = host.conversation
  const cwd = conv?.cwd || '/'
  const base = [
    `You are an AI development agent connected to the remote host "${host.hostLabel}".`,
    `Current working directory: ${cwd}`,
    '',
    'You can execute shell commands on that host with the run_command tool.',
    'Commands run non-interactively in a fresh shell (your working directory is preserved between calls).',
    'Avoid interactive commands (vim, top, less, tail -f). Prefer small, verifiable steps.',
    'After each command you see its combined stdout/stderr and exit code.',
    'Never invent command output. If a step fails, diagnose from the real output and continue or report.',
    'When the task is complete, reply with a concise plain-text summary; you do not need to call more tools.'
  ]
  const userRules = (dataFile?.rules ?? '').trim()
  const remote = projectRules.trim()
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
      out.push({ role: 'user', content: msg.text })
      continue
    }
    if (msg.role === 'assistant') {
      const toolCalls: ApiToolCallMsg[] = (msg.toolCalls ?? []).map((tc) => ({
        id: tc.id,
        name: RUN_COMMAND_TOOL_NAME,
        arguments: JSON.stringify({ command: tc.command })
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
  host.inRun = false
  host.phase = 'paused'
}

function listSetting(settings: Record<string, unknown>, key: string): string[] {
  const value = settings[key]
  return Array.isArray(value) ? value.filter((s): s is string => typeof s === 'string') : []
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
  command: string,
  content: string,
  outcome: AiAgentConversationToolMsg['outcome'],
  truncated: boolean
): AiAgentConversationMsg {
  return { role: 'tool', toolCallId, command, content, outcome, truncated }
}

async function askApproval(host: HostState, command: string): Promise<string> {
  host.pendingApproval = { requestId: randomUUID(), command, cwd: host.conversation?.cwd || '/' }
  host.phase = 'ask'
  pushState(host)
  const decision = await new Promise<string>((resolve) => {
    host.approvalResolve = resolve
  })
  host.approvalResolve = null
  host.pendingApproval = null
  return decision
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
    pushToast(host, 'error', 'No model provider configured — open the gear menu and pick one.')
    return
  }
  if (!conv.activeModel) {
    pushToast(host, 'error', 'Pick a model for this conversation first.')
    return
  }
  // Read the key from the encrypted vault (safeStorage/DPAPI) for every run so
  // keys survive restarts without any renderer round-trip or in-memory cache.
  const apiKey = ctx.getSecret(aiAgentVaultId(provider.id)) ?? ''
  if (!apiKey && provider.protocol === 'anthropic') {
    pushToast(host, 'error', `No API key set for provider "${provider.name}".`)
    return
  }

  const settings = ctx.getSettings()
  const hostAllow = listSetting(settings, AI_AGENT_SETTING_HOST_ALLOW_RULES)
  const hostDeny = listSetting(settings, AI_AGENT_SETTING_HOST_DENY_RULES)
  const appAllow = listSetting(settings, AI_AGENT_SETTING_DEFAULT_ALLOW_RULES)
  const appDeny = listSetting(settings, AI_AGENT_SETTING_DEFAULT_DENY_RULES)
  const projectRules = await readProjectRules(ctx)

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
        system: systemPrompt(host, projectRules),
        messages: toApiMessages(conv),
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
        toolCalls: result.toolCalls
          .map((tc) => ({ id: tc.id, command: extractCommand(tc.arguments) }))
          .filter((tc) => tc.command.length > 0)
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
        const command = extractCommand(tc.arguments)
        if (!command) {
          conv.messages.push(
            toolResultMessage(tc.id, tc.arguments, 'Model sent an empty command.', 'error', false)
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
          action = await askApproval(host, command)
          if (!host.inRun) {
            keepRunning = false
            break
          }
          if (action === 'allowAlways') {
            host.extraAllow.push(command)
          } else if (action === 'denyAlways') {
            host.extraDeny.push(command)
          }
        }
        if (action === 'deny' || action === 'denyAlways') {
          conv.messages.push(toolResultMessage(tc.id, command, '', 'denied', false))
          pushState(host)
          persistConversation(host)
          continue
        }

        const execResult = await execCommand(host, command)
        if (!host.inRun) {
          keepRunning = false
          break
        }
        if (execResult.pwd) {
          conv.cwd = execResult.pwd
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
            command,
            toolContentForModel(execResult, command, outcome),
            outcome,
            execResult.truncated
          )
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

async function execCommand(host: HostState, command: string): Promise<ExecResult> {
  const ctx = host.runCtx
  if (!ctx) {
    return { outcome: 'error', content: 'Session is not connected', exitCode: null, truncated: false, pwd: null }
  }
  const token = randomUUID().slice(0, 8)
  const statusTag = `__WASSH_STATUS_${token}`
  const pwdTag = `__WASSH_PWD_${token}`
  const cwd = host.conversation?.cwd || '/'
  const wrapped = [
    `cd ${shellQuote(cwd)} >/dev/null 2>&1 || true`,
    command,
    '__wassh_status=$?',
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
}

function hostForCtx(ctx: PluginMainContext): HostState | null {
  const tab = tabs.get(ctx.tabId)
  if (!tab) {
    return null
  }
  return hosts.get(tab.hostKey) ?? null
}

function chatMessageWithTerminal(tab: TabRuntime, text: string, attachTerminal: boolean): string {
  if (!attachTerminal || !tab.terminalTail) {
    return text
  }
  const tail = tab.terminalTail.slice(-TERMINAL_CONTEXT_EXCERPT_CHARS)
  return `${text}\n\n[Recent terminal output]\n${tail}`
}

async function handleRendererMessage(
  ctx: PluginMainContext,
  payload: AiAgentRendererMessage
): Promise<void> {
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
      dataFile.providers = payload.providers
      saveData()
    }
    const host = hostForCtx(ctx)
    if (host) {
      pushState(host)
    }
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
    return
  }
  if (payload.type === 'select') {
    host.conversation.activeProviderId = payload.providerId
    host.conversation.activeModel = payload.model
    persistConversation(host)
    pushState(host)
    return
  }
  if (payload.type === 'newChat') {
    host.conversation.messages = []
    host.conversation.activeProviderId = payload.providerId
    host.conversation.activeModel = payload.model
    persistConversation(host)
    pushState(host)
    return
  }
  if (payload.type === 'chat') {
    if (!ctx.isSshSession()) {
      pushToast(host, 'error', 'The AI agent needs an SSH session to run commands.')
      return
    }
    if (host.phase === 'running' || host.phase === 'ask') {
      pushToast(host, 'info', 'The agent is busy — stop it or wait for the current run.')
      return
    }
    pruneUnresolvedTail(host.conversation)
    host.conversation.activeProviderId = payload.providerId
    host.conversation.activeModel = payload.model
    const text = chatMessageWithTerminal(tab, payload.text.trim(), payload.attachTerminal === true)
    host.conversation.messages.push({ role: 'user', text, usedTerminalContext: payload.attachTerminal === true })
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
    await handleRendererMessage(ctx, payload as AiAgentRendererMessage)
  }
}


