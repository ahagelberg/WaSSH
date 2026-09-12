import {
  useEffect,
  useRef,
  useState,
  type DragEvent as ReactDragEvent,
  type ReactElement
} from 'react'
import Markdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import './styles.css'

import {
  AI_AGENT_ANTHROPIC_BASE_URL,
  AI_AGENT_DEFAULT_CHAT_TITLE,
  AI_AGENT_DEFAULT_PROVIDERS,
  AI_AGENT_MAX_ATTACHMENT_BYTES,
  AI_AGENT_MAX_ATTACHMENT_CHARS,
  AI_AGENT_MAX_ATTACHMENTS,
  AI_AGENT_OLLAMA_PROVIDER_ID
} from './defaults'
import { aiAgentVaultId } from './id'
import {
  AI_AGENT_PROTOCOL_ANTHROPIC,
  AI_AGENT_PROTOCOL_OPENAI,
  AI_AGENT_SETTING_HOST_ALLOW_RULES,
  AI_AGENT_SETTING_HOST_DENY_RULES,
  type AiAgentApprovalDecision,
  type AiAgentChatAttachment,
  type AiAgentConversation,
  type AiAgentConversationSummary,
  type AiAgentConversationToolMsg,
  type AiAgentDeltaPayload,
  type AiAgentProviderConfig,
  type AiAgentProviderProtocol,
  type AiAgentStateSnapshot,
  type AiAgentToastPayload,
  type AiAgentToolOutputPayload
} from './protocol'
import type { PluginViewProps } from '@plugin-api/renderer'

/** Streaming rows rendered between two state snapshots may not exceed this */
const STREAM_PLACEHOLDER_LIMIT = 1_000_000

/** Streaming command output rendered before the final tool result is available. */
const TOOL_OUTPUT_STREAM_LIMIT = 64_000

/** Max characters shown in the queued-message preview strip */
const QUEUE_PREVIEW_MAX_CHARS = 120

/** Null bytes in the first N bytes → treat file as binary */
const BINARY_PROBE_BYTES = 8_000

/** Pointer movement before a provider row drag starts (mirrors the host list) */
const PROVIDER_DRAG_THRESHOLD_PX = 4

/** Drop before a provider row when the pointer is above this fraction of its height */
const PROVIDER_DROP_BEFORE_RATIO = 0.5

/** Drag ghost offset from the pointer (px) */
const PROVIDER_DRAG_GHOST_OFFSET_X_PX = 12
const PROVIDER_DRAG_GHOST_OFFSET_Y_PX = 10

interface QueuedMessage {
  id: string
  text: string
  attachTerminal: boolean
  attachments: AiAgentChatAttachment[]
  providerId: string
  model: string
}


type PhaseLabel = Record<string, string>

const PHASE_LABELS: PhaseLabel = {
  idle: 'Ready',
  running: 'Running…',
  ask: 'Waiting for approval',
  ask_sudo: 'Waiting for sudo password',
  paused: 'Paused',
  no_session: 'No SSH session'
}

function isOllamaLike(provider: { id: string }): boolean {
  return provider.id === AI_AGENT_OLLAMA_PROVIDER_ID
}

function outcomeLabel(outcome: AiAgentConversationToolMsg['outcome']): string {
  if (outcome === 'ok') {
    return 'ok'
  }
  if (outcome === 'denied') {
    return 'denied'
  }
  if (outcome === 'timeout') {
    return 'timed out'
  }
  if (outcome === 'cancelled') {
    return 'cancelled'
  }
  return 'failed'
}

function toolBadge(name?: string): { label: string; className: string } {
  if (!name || name === 'run_command') {
    return { label: 'SSH', className: 'tool-badge-ssh' }
  }
  if (name.startsWith('remote_fs_')) {
    return { label: 'Remote SFTP', className: 'tool-badge-remote-fs' }
  }
  if (name.startsWith('local_fs_')) {
    return { label: 'Local PC', className: 'tool-badge-local-fs' }
  }
  if (name === 'web_search') {
    return { label: 'Search', className: 'tool-badge-search' }
  }
  if (name === 'web_fetch') {
    return { label: 'Web', className: 'tool-badge-web' }
  }
  if (name === 'get_current_time') {
    return { label: 'Time', className: 'tool-badge-time' }
  }
  return { label: 'Tool', className: 'tool-badge-generic' }
}

const MARKDOWN_COMPONENTS: Components = {
  a({ node: _node, ...props }) {
    return <a {...props} target="_blank" rel="noreferrer" />
  },
  pre({ node: _node, ...props }) {
    return <pre {...props} className="ai-agent-code" />
  }
}

function MarkdownText({ text }: { text: string }): ReactElement {
  return (
    <Markdown remarkPlugins={[remarkGfm]} components={MARKDOWN_COMPONENTS} skipHtml>
      {text}
    </Markdown>
  )
}

interface ViewState {
  providers: AiAgentProviderConfig[]
  /** Provider ids with a key currently stored in the vault */
  providerKeys: string[]
  conversation: AiAgentConversation | null
  conversationSummaries: AiAgentConversationSummary[]
  runPhase: AiAgentStateSnapshot['runPhase']
  hostLabel: string
  ssh: boolean
  pendingApproval: AiAgentStateSnapshot['pendingApproval']
  pendingSudo: AiAgentStateSnapshot['pendingSudo']
  rules: string
  lastError?: string
}

function emptyViewState(): ViewState {
  return {
    providers: [],
    providerKeys: [],
    conversation: null,
    conversationSummaries: [],
    runPhase: 'no_session',
    hostLabel: '',
    ssh: false,
    pendingApproval: null,
    pendingSudo: null,
    rules: ''
  }
}

function formatChatTime(ts: number): string {
  try {
    return new Date(ts).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    })
  } catch {
    return ''
  }
}

function isFileDrag(e: ReactDragEvent<HTMLElement>): boolean {
  return Array.from(e.dataTransfer.types).includes('Files')
}

function collectDroppedFiles(dt: DataTransfer | null): File[] {
  if (!dt) {
    return []
  }
  const items = dt.items
  if (items && items.length > 0) {
    const files: File[] = []
    for (const item of Array.from(items)) {
      if (item.kind !== 'file') {
        continue
      }
      const entry = item.webkitGetAsEntry?.()
      if (entry?.isDirectory) {
        continue
      }
      const file = item.getAsFile()
      if (file) {
        files.push(file)
      }
    }
    return files
  }
  return Array.from(dt.files)
}

function looksBinary(bytes: Uint8Array): boolean {
  const n = Math.min(bytes.length, BINARY_PROBE_BYTES)
  for (let i = 0; i < n; i += 1) {
    if (bytes[i] === 0) {
      return true
    }
  }
  return false
}

async function readAttachment(file: File): Promise<AiAgentChatAttachment> {
  const truncatedByBytes = file.size > AI_AGENT_MAX_ATTACHMENT_BYTES
  const blob = truncatedByBytes ? file.slice(0, AI_AGENT_MAX_ATTACHMENT_BYTES) : file
  const buffer = await blob.arrayBuffer()
  const bytes = new Uint8Array(buffer)
  if (looksBinary(bytes)) {
    return {
      name: file.name,
      mimeType: file.type || undefined,
      truncated: truncatedByBytes,
      binary: true
    }
  }
  let text = new TextDecoder('utf-8', { fatal: false }).decode(bytes)
  let truncated = truncatedByBytes
  if (text.length > AI_AGENT_MAX_ATTACHMENT_CHARS) {
    text = text.slice(0, AI_AGENT_MAX_ATTACHMENT_CHARS)
    truncated = true
  }
  return {
    name: file.name,
    mimeType: file.type || undefined,
    text,
    truncated,
    binary: false
  }
}

export default function AiAgentView({
  tabId,
  pluginId,
  settings,
  onSettingsPatch
}: PluginViewProps): ReactElement {
  const [view, setView] = useState<ViewState>(emptyViewState)
  const [stream, setStream] = useState('')
  const [toolOutput, setToolOutput] = useState<Record<string, string>>({})
  const [input, setInput] = useState('')
  const [attach, setAttach] = useState(false)
  const [attachments, setAttachments] = useState<AiAgentChatAttachment[]>([])
  const [dropActive, setDropActive] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [toast, setToast] = useState<{ kind: 'error' | 'info'; text: string } | null>(null)
  const [queued, setQueued] = useState<QueuedMessage | null>(null)
  const [sudoPassword, setSudoPassword] = useState('')
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const sudoInputRef = useRef<HTMLInputElement>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const promptInputRef = useRef<HTMLTextAreaElement>(null)
  const dragDepthRef = useRef(0)
  /** Indicates that the queued message should replace the active run. */
  const forceAfterStopRef = useRef(false)
  /** Identifies the queued message that has already been dispatched. */
  const drainedQueueIdRef = useRef<string | null>(null)
  /** Prevents repeated initial model refreshes for the Ollama preset. */
  const mountedOllamaRefreshRef = useRef(false)

  const send = (payload: Parameters<typeof window.wassh.sendPluginMessage>[2]): void => {
    void window.wassh.sendPluginMessage(tabId, pluginId, payload)
  }

  const showToast = (kind: 'error' | 'info', text: string): void => {
    setToast({ kind, text })
    if (toastTimer.current) {
      clearTimeout(toastTimer.current)
    }
    toastTimer.current = setTimeout(() => {
      toastTimer.current = null
      setToast(null)
    }, 5000)
  }

  useEffect(() => {
    const off = window.wassh.onPluginMessage((ev) => {
      if (ev.tabId !== tabId || ev.pluginId !== pluginId) {
        return
      }
      const payload = ev.payload as
        | AiAgentStateSnapshot
        | AiAgentDeltaPayload
        | AiAgentToastPayload
        | AiAgentToolOutputPayload
        | null
      if (!payload || typeof payload !== 'object' || !('type' in payload)) {
        return
      }
      if (payload.type === 'state') {
        setView({
          providers: payload.providers,
          providerKeys: payload.providerKeys,
          conversation: payload.conversation,
          conversationSummaries: payload.conversationSummaries ?? [],
          runPhase: payload.runPhase,
          hostLabel: payload.hostLabel,
          ssh: payload.ssh,
          pendingApproval: payload.pendingApproval,
          pendingSudo: payload.pendingSudo,
          rules: payload.rules,
          lastError: payload.lastError
        })
        setStream('')
        setToolOutput({})
        if (payload.runPhase !== 'ask_sudo') {
          setSudoPassword('')
        }
        return
      }
      if (payload.type === 'delta') {
        setStream((prev) => (prev + payload.text).slice(0, STREAM_PLACEHOLDER_LIMIT))
        return
      }
      if (payload.type === 'toolOutput') {
        setToolOutput((current) => ({
          ...current,
          [payload.toolCallId]: ((current[payload.toolCallId] ?? '') + payload.text)
            .slice(0, TOOL_OUTPUT_STREAM_LIMIT)
        }))
        return
      }
      if (payload.type === 'toast') {
        showToast(payload.kind, payload.text)
      }
    })
    return () => {
      off()
      if (toastTimer.current) {
        clearTimeout(toastTimer.current)
      }
    }
  }, [tabId, pluginId])

  useEffect(() => {
    send({ type: 'sync' })
  }, [tabId, pluginId])

  useEffect(() => {
    const onProvidersChanged = (event: Event): void => {
      const providers = (event as CustomEvent<AiAgentProviderConfig[]>).detail
      if (Array.isArray(providers)) {
        send({ type: 'providersChanged', providers })
      }
    }
    window.addEventListener('ai-agent-providers-changed', onProvidersChanged)
    return () => window.removeEventListener('ai-agent-providers-changed', onProvidersChanged)
  }, [tabId, pluginId])

  const conv = view.conversation
  const providers = view.providers
  const activeProvider =
    providers.find((p) => p.id === conv?.activeProviderId) ?? providers[0]
  const activeModel =
    conv?.activeModel ||
    (activeProvider ? conv?.lastSelectedModelByProvider[activeProvider.id] : '') ||
    activeProvider?.models[0] ||
    ''
  const busy =
    view.runPhase === 'running' || view.runPhase === 'ask' || view.runPhase === 'ask_sudo'

  useEffect(() => {
    if (mountedOllamaRefreshRef.current) {
      return
    }
    const ollama = providers.find((p) => isOllamaLike(p))
    if (ollama) {
      mountedOllamaRefreshRef.current = true
      send({ type: 'refreshModels', providerId: ollama.id })
    }
  }, [providers])

  const dispatchChat = (msg: Omit<QueuedMessage, 'id'>): void => {
    send({
      type: 'chat',
      providerId: msg.providerId,
      model: msg.model,
      text: msg.text,
      attachTerminal: msg.attachTerminal,
      attachments: msg.attachments.length > 0 ? msg.attachments : undefined
    })
  }

  useEffect(() => {
    if (!queued) {
      return
    }
    if (drainedQueueIdRef.current === queued.id) {
      return
    }
    const forceReady =
      forceAfterStopRef.current &&
      (view.runPhase === 'idle' || view.runPhase === 'paused')
    const autoReady = !forceAfterStopRef.current && view.runPhase === 'idle'
    if (!forceReady && !autoReady) {
      return
    }
    drainedQueueIdRef.current = queued.id
    forceAfterStopRef.current = false
    const msg = queued
    setQueued(null)
    dispatchChat(msg)
  }, [view.runPhase, queued, tabId, pluginId])

  const validateOutgoing = (text: string, fileCount: number): boolean => {
    if (!text && fileCount === 0) {
      return false
    }
    if (providers.length === 0) {
      showToast('error', 'No model providers configured — configure one in Options.')
      return false
    }
    if (!activeProvider) {
      return false
    }
    if (!activeModel) {
      showToast('error', 'Pick a model first.')
      return false
    }
    if (!view.ssh) {
      showToast('error', 'The AI agent needs an SSH session to run commands.')
      return false
    }
    return true
  }

  const buildQueued = (
    text: string,
    attachTerminal: boolean,
    files: AiAgentChatAttachment[]
  ): QueuedMessage | null => {
    if (!activeProvider || !validateOutgoing(text, files.length)) {
      return null
    }
    return {
      id: crypto.randomUUID(),
      text,
      attachTerminal,
      attachments: files,
      providerId: activeProvider.id,
      model: activeModel
    }
  }

  const clearComposerAttachments = (): void => {
    setAttachments([])
    setAttach(false)
  }

  const addFiles = async (files: File[]): Promise<void> => {
    if (files.length === 0) {
      return
    }
    const room = AI_AGENT_MAX_ATTACHMENTS - attachments.length
    if (room <= 0) {
      showToast('info', `At most ${AI_AGENT_MAX_ATTACHMENTS} files can be attached.`)
      return
    }
    const take = files.slice(0, room)
    if (files.length > room) {
      showToast('info', `Only ${AI_AGENT_MAX_ATTACHMENTS} files can be attached; extras skipped.`)
    }
    const read = await Promise.all(take.map((f) => readAttachment(f)))
    setAttachments((prev) => [...prev, ...read])
  }

  const selectProvider = (providerId: string): void => {
    const provider = providers.find((p) => p.id === providerId)
    const model = conv?.lastSelectedModelByProvider[providerId] ?? provider?.models[0] ?? ''
    send({ type: 'select', providerId, model })
    if (provider) {
      send({ type: 'refreshModels', providerId })
    }
    if (conv) {
      setView((prev) =>
        prev.conversation
          ? {
              ...prev,
              conversation: {
                ...prev.conversation,
                activeProviderId: providerId,
                activeModel: model,
                lastSelectedModelByProvider: model
                  ? { ...prev.conversation.lastSelectedModelByProvider, [providerId]: model }
                  : prev.conversation.lastSelectedModelByProvider
              }
            }
          : prev
      )
    }
  }

  const selectModel = (model: string): void => {
    if (activeProvider) {
      send({ type: 'select', providerId: activeProvider.id, model })
      if (conv) {
        setView((prev) =>
          prev.conversation
            ? {
                ...prev,
                conversation: {
                  ...prev.conversation,
                  activeModel: model,
                  lastSelectedModelByProvider: {
                    ...prev.conversation.lastSelectedModelByProvider,
                    [activeProvider.id]: model
                  }
                }
              }
            : prev
        )
      }
    }
  }

  const handleSend = (): void => {
    const text = input.trim()

    if (busy) {
      if (queued) {
        const next =
          text || attachments.length > 0
            ? buildQueued(text, attach, attachments)
            : queued
        if (!next) {
          return
        }
        forceAfterStopRef.current = true
        drainedQueueIdRef.current = null
        setQueued(next)
        setInput('')
        clearComposerAttachments()
        send({ type: 'stop' })
        return
      }
      if (!text && attachments.length === 0) {
        return
      }
      const next = buildQueued(text, attach, attachments)
      if (!next) {
        return
      }
      drainedQueueIdRef.current = null
      setQueued(next)
      setInput('')
      clearComposerAttachments()
      return
    }

    const toSend =
      text || attachments.length > 0 ? buildQueued(text, attach, attachments) : queued
    if (!toSend) {
      return
    }
    forceAfterStopRef.current = false
    drainedQueueIdRef.current = toSend.id
    setQueued(null)
    setInput('')
    clearComposerAttachments()
    dispatchChat(toSend)
  }

  const clearQueue = (): void => {
    forceAfterStopRef.current = false
    drainedQueueIdRef.current = queued?.id ?? null
    setQueued(null)
  }

  const hostRuleList = (key: string): string[] => {
    const value = settings[key]
    return Array.isArray(value) ? value.filter((s): s is string => typeof s === 'string') : []
  }

  const patchHostRules = (key: string, pattern: string): void => {
    const next = [...hostRuleList(key), pattern]
    onSettingsPatch({ [key]: next })
  }

  const handleApproval = (decision: AiAgentApprovalDecision): void => {
    const request = view.pendingApproval
    if (!request) {
      return
    }
    if (request.kind === 'command') {
      if (decision === 'allowAlways') {
        patchHostRules(AI_AGENT_SETTING_HOST_ALLOW_RULES, request.subject)
      } else if (decision === 'denyAlways') {
        patchHostRules(AI_AGENT_SETTING_HOST_DENY_RULES, request.subject)
      }
    }
    send({ type: 'approval', requestId: request.requestId, decision })
  }

  const submitSudoPassword = (password: string | null): void => {
    const request = view.pendingSudo
    if (!request) {
      return
    }
    send({ type: 'sudoPassword', requestId: request.requestId, password })
    setSudoPassword('')
  }

  useEffect(() => {
    if (view.runPhase === 'ask_sudo' && sudoInputRef.current) {
      sudoInputRef.current.focus()
    }
  }, [view.runPhase, view.pendingSudo?.requestId])

  const canResume =
    conv !== null &&
    conv.messages.length > 0 &&
    (view.runPhase === 'paused' ||
      (view.runPhase === 'idle' && conv.messages[conv.messages.length - 1].role === 'user'))

  const messages = conv?.messages ?? []
  const messagesRef = useRef<HTMLDivElement>(null)

  const messageRows: ReactElement[] = []
  for (let i = 0; i < messages.length; i += 1) {
    const msg = messages[i]
    if (msg.role === 'user') {
      messageRows.push(
        <div key={i} className="ai-agent-msg ai-agent-user">
          <div className="ai-agent-msg-meta">
            {msg.attachedFiles && msg.attachedFiles.length > 0 ? (
              <span
                className="ai-agent-ctx-tag"
                title={msg.attachedFiles.join(', ')}
              >
                +{msg.attachedFiles.length} file{msg.attachedFiles.length === 1 ? '' : 's'}
              </span>
            ) : null}
            {msg.usedTerminalContext ? (
              <span className="ai-agent-ctx-tag" title="Recent terminal output was attached">
                +terminal
              </span>
            ) : null}
          </div>
          <div className="ai-agent-user-text"><MarkdownText text={msg.text} /></div>
        </div>
      )
      continue
    }
    if (msg.role === 'assistant') {
      const toolChildren: ReactElement[] = []
      for (const tc of msg.toolCalls ?? []) {
        const outputs: AiAgentConversationToolMsg[] = []
        for (let j = i + 1; j < messages.length; j += 1) {
          const later = messages[j]
          if (later.role === 'assistant') {
            break
          }
          if (later.role === 'tool' && later.toolCallId === tc.id) {
            outputs.push(later)
          }
        }
        const body =
          outputs.length === 0 ? (
            toolOutput[tc.id] ? (
              <div className="ai-agent-tool-out">
                <div className="ai-agent-tool-meta">
                  <span className="ai-agent-tool-status">running</span>
                </div>
                <pre className="ai-agent-out">{toolOutput[tc.id]}</pre>
              </div>
            ) : (
              <span className="ai-agent-tool-status">ran</span>
            )
          ) : (
            outputs.map((out, k) => (
              <div key={k} className="ai-agent-tool-out">
                <div className="ai-agent-tool-meta">
                  <span className={`ai-agent-tool-outcome ${out.outcome}`}>
                    {outcomeLabel(out.outcome)}
                  </span>
                  {out.truncated ? <span>truncated</span> : null}
                </div>
                {out.content ? (
                  <pre className="ai-agent-out">{out.content}</pre>
                ) : (
                  <span className="ai-agent-tool-empty">(no output)</span>
                )}
              </div>
            ))
          )
        const badge = toolBadge(tc.name)
        toolChildren.push(
          <div key={tc.id} className="ai-agent-tool">
            <div className="ai-agent-tool-command">
              <span className={`ai-agent-tool-badge ${badge.className}`}>{badge.label}</span>
              <span className="ai-agent-tool-cmd-text">{tc.command.startsWith('$') ? tc.command : `$ ${tc.command}`}</span>
            </div>
            {body}
          </div>
        )
      }
      messageRows.push(
        <div key={i} className="ai-agent-msg ai-agent-assistant">
          {msg.text ? <div className="ai-agent-assistant-text"><MarkdownText text={msg.text} /></div> : null}
          {toolChildren}
          {msg.stopped ? <div className="ai-agent-interrupted">interrupted</div> : null}
        </div>
      )
      continue
    }
    const badge = toolBadge(msg.name)
    messageRows.push(
      <div key={i} className="ai-agent-tool">
        <div className="ai-agent-tool-command">
          <span className={`ai-agent-tool-badge ${badge.className}`}>{badge.label}</span>
          <span className="ai-agent-tool-cmd-text">{msg.command.startsWith('$') ? msg.command : `$ ${msg.command}`}</span>
        </div>
        <span className={`ai-agent-tool-outcome ${msg.outcome}`}>{outcomeLabel(msg.outcome)}</span>
      </div>
    )
  }

  const running = view.runPhase === 'running'
  if (running && stream) {
    messageRows.push(
      <div key="stream" className="ai-agent-msg ai-agent-assistant">
        <div className="ai-agent-assistant-text"><MarkdownText text={stream} /></div>
      </div>
    )
  } else if (running) {
    messageRows.push(
      <div key="thinking" className="ai-agent-msg ai-agent-assistant">
        <div className="ai-agent-thinking" aria-live="polite">
          Thinking
          <span className="ai-agent-thinking-dots" aria-hidden="true">
            <span>.</span>
            <span>.</span>
            <span>.</span>
          </span>
        </div>
      </div>
    )
  }

  useEffect(() => {
    const el = messagesRef.current
    if (el) {
      el.scrollTop = el.scrollHeight
    }
  }, [messages.length, stream, toolOutput, view.runPhase, historyOpen])

  useEffect(() => {
    const el = promptInputRef.current
    if (!el) {
      return
    }
    el.style.height = 'auto'
    el.style.height = `${el.scrollHeight}px`
  }, [input])


  const phaseClass = view.runPhase
  const providerOptions =
    providers.length > 0 ? (
      <>
        <select
          className="ai-agent-provider-select"
          value={activeProvider?.id ?? ''}
          onChange={(e) => selectProvider(e.target.value)}
          title="Model provider"
        >
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <select
          className="ai-agent-model-select"
          value={activeModel}
          onChange={(e) => selectModel(e.target.value)}
          title="Model"
        >
          {(activeProvider?.models ?? []).map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
      </>
    ) : null

  return (
    <div className="plugin-panel ai-agent">
      <>
          <div className="ai-agent-header">
            <div className="ai-agent-header-controls">
              <button
                type="button"
                className="ai-agent-gear-btn"
                title="Chat history"
                onClick={() => setHistoryOpen((prev) => !prev)}
              >
                History
              </button>
              <button
                type="button"
                onClick={() => {
                  if (activeProvider) {
                    setHistoryOpen(false)
                    send({ type: 'newChat', providerId: activeProvider.id, model: activeModel })
                  }
                }}
                disabled={busy}
                title="Start a new conversation"
              >
                New
              </button>
            </div>
          </div>

          {historyOpen ? (
            <div className="ai-agent-history-panel">
              <div className="ai-agent-pane-label">Chat history</div>
              {view.conversationSummaries.length === 0 ? (
                <div className="ai-agent-history-empty">No saved conversations yet.</div>
              ) : (
                <ul className="ai-agent-history-list">
                  {view.conversationSummaries.map((item) => {
                    const active = conv?.id === item.id
                    return (
                      <li key={item.id} className={`ai-agent-history-item${active ? ' active' : ''}`}>
                        <button
                          type="button"
                          className="ai-agent-history-open"
                          disabled={busy && !active}
                          title={item.title}
                          onClick={() => {
                            if (!active) {
                              send({ type: 'openChat', conversationId: item.id })
                            }
                            setHistoryOpen(false)
                          }}
                        >
                          <span className="ai-agent-history-title">
                            {item.title || AI_AGENT_DEFAULT_CHAT_TITLE}
                          </span>
                          <span className="ai-agent-history-time">{formatChatTime(item.updatedAt)}</span>
                        </button>
                        <button
                          type="button"
                          className="ai-agent-danger ai-agent-history-delete"
                          disabled={busy}
                          title="Delete conversation"
                          onClick={() => send({ type: 'deleteChat', conversationId: item.id })}
                        >
                          ×
                        </button>
                      </li>
                    )
                  })}
                </ul>
              )}
              <div className="ai-agent-history-actions">
                <button type="button" onClick={() => setHistoryOpen(false)}>
                  Close
                </button>
              </div>
            </div>
          ) : null}

          {view.lastError ? <div className="ai-agent-error-bar">{view.lastError}</div> : null}

          <div className="ai-agent-messages" ref={messagesRef}>
            {messageRows.length === 0 ? (
              <div className="ai-agent-empty">
                {providers.length === 0
                  ? 'No model providers configured yet. Configure one in Options.'
                  : view.ssh
                    ? 'Ask the agent to inspect or change something on this host. It can run commands when you approve them.'
                    : 'Connect an SSH session to use the AI agent.'}
              </div>
            ) : (
              messageRows
            )}
          </div>

          {view.runPhase === 'ask' && view.pendingApproval ? (
            <div className="ai-agent-approval">
              <div className="ai-agent-approval-title">
                {view.pendingApproval.kind === 'command' ? 'Approve command?' : 'Approve action?'}
              </div>
              <pre className="ai-agent-approval-command">{view.pendingApproval.subject}</pre>
              <div className="ai-agent-approval-actions">
                <button type="button" onClick={() => handleApproval('allow')}>
                  Approve once
                </button>
                <button type="button" onClick={() => handleApproval('deny')}>
                  Deny once
                </button>
                <button type="button" onClick={() => handleApproval('allowAlways')}>
                  Always allow
                </button>
                <button type="button" className="ai-agent-danger" onClick={() => handleApproval('denyAlways')}>
                  Always deny
                </button>
              </div>
            </div>
          ) : null}

          {view.runPhase === 'ask_sudo' && view.pendingSudo ? (
            <div className="ai-agent-sudo">
              <div className="ai-agent-sudo-title">Sudo password required</div>
              <pre className="ai-agent-sudo-command">{view.pendingSudo.command}</pre>
              <p className="ai-agent-sudo-hint">
                Cached in memory for a few minutes. Leave blank for passwordless sudo.
              </p>
              <form
                className="ai-agent-sudo-form"
                onSubmit={(e) => {
                  e.preventDefault()
                  submitSudoPassword(sudoPassword)
                }}
              >
                <input
                  ref={sudoInputRef}
                  type="password"
                  autoComplete="current-password"
                  placeholder="Sudo password"
                  value={sudoPassword}
                  onChange={(e) => setSudoPassword(e.target.value)}
                />
                <button type="submit">Continue</button>
                <button type="button" className="ai-agent-danger" onClick={() => submitSudoPassword(null)}>
                  Cancel
                </button>
              </form>
            </div>
          ) : null}

          {canResume ? (
            <div className="ai-agent-continue">
              <span>The previous run was interrupted.</span>
              <button type="button" onClick={() => send({ type: 'resume' })}>
                Continue
              </button>
            </div>
          ) : null}

          {toast ? (
            <div className={`ai-agent-toast ${toast.kind}`}>
              <span>{toast.text}</span>
              <button type="button" onClick={() => setToast(null)}>
                ×
              </button>
            </div>
          ) : null}

          {queued ? (
            <div className="ai-agent-queue">
              <span className="ai-agent-queue-label">Queued</span>
              <span className="ai-agent-queue-text" title={queued.text}>
                {queued.text.length > QUEUE_PREVIEW_MAX_CHARS
                  ? `${queued.text.slice(0, QUEUE_PREVIEW_MAX_CHARS)}…`
                  : queued.text || '(attached files)'}
              </span>
              <span className="ai-agent-queue-hint">
                {busy ? 'Enter again to send now' : 'Press Enter to send'}
              </span>
              <button type="button" title="Discard queued message" onClick={clearQueue}>
                ×
              </button>
            </div>
          ) : null}

          <div
            className={`ai-agent-composer${dropActive ? ' drop-active' : ''}`}
            onDragEnter={(e) => {
              if (!isFileDrag(e)) {
                return
              }
              e.preventDefault()
              e.stopPropagation()
              dragDepthRef.current += 1
              setDropActive(true)
            }}
            onDragLeave={(e) => {
              if (!isFileDrag(e)) {
                return
              }
              e.preventDefault()
              e.stopPropagation()
              dragDepthRef.current = Math.max(0, dragDepthRef.current - 1)
              if (dragDepthRef.current === 0) {
                setDropActive(false)
              }
            }}
            onDragOver={(e) => {
              if (!isFileDrag(e)) {
                return
              }
              e.preventDefault()
              e.stopPropagation()
              e.dataTransfer.dropEffect = 'copy'
            }}
            onDrop={(e) => {
              if (!isFileDrag(e)) {
                return
              }
              e.preventDefault()
              e.stopPropagation()
              dragDepthRef.current = 0
              setDropActive(false)
              void addFiles(collectDroppedFiles(e.dataTransfer))
            }}
          >
            {dropActive ? (
              <div className="ai-agent-drop-hint">
                <div className="ai-agent-drop-box">Drop files to attach</div>
              </div>
            ) : null}

            <div className={`ai-agent-phase-bar ${phaseClass}`}>
              {PHASE_LABELS[view.runPhase] ?? view.runPhase}
            </div>

            {attachments.length > 0 ? (
              <div className="ai-agent-file-chips">
                {attachments.map((file, index) => (
                  <span
                    key={`${file.name}-${index}`}
                    className={`ai-agent-file-chip${file.binary ? ' binary' : ''}${file.truncated ? ' truncated' : ''}`}
                    title={
                      file.binary
                        ? `${file.name} (binary — content omitted)`
                        : file.truncated
                          ? `${file.name} (truncated)`
                          : file.name
                    }
                  >
                    <span className="ai-agent-file-chip-name">{file.name}</span>
                    <button
                      type="button"
                      title="Remove attachment"
                      onClick={() =>
                        setAttachments((prev) => prev.filter((_, i) => i !== index))
                      }
                    >
                      ×
                    </button>
                  </span>
                ))}
              </div>
            ) : null}

            <input
              ref={fileInputRef}
              type="file"
              multiple
              hidden
              onChange={(e) => {
                const list = e.target.files
                if (list && list.length > 0) {
                  void addFiles(Array.from(list))
                }
                e.target.value = ''
              }}
            />

            <div className="ai-agent-inputbar">
              <div className="ai-agent-input-actions">
                <button
                  type="button"
                  className="ai-agent-file-btn"
                  title="Attach local files"
                  onClick={() => fileInputRef.current?.click()}
                  disabled={attachments.length >= AI_AGENT_MAX_ATTACHMENTS}
                >
                  📎
                </button>
                <button
                  type="button"
                  className={`ai-agent-terminal-btn${attach ? ' active' : ''}`}
                  title="Attach recent terminal output to the next message"
                  onClick={() => setAttach((prev) => !prev)}
                >
                  ⌁
                </button>
              </div>
              <textarea
                ref={promptInputRef}
                className="ai-agent-input"
                rows={1}
                value={input}
                placeholder={
                  busy
                    ? queued
                      ? 'Enter again to send queued message now…'
                      : 'Queue a follow-up… (Enter to queue)'
                    : 'Message the agent… (Shift+Enter for newline, or drop files)'
                }
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault()
                    handleSend()
                  }
                }}
              />
              <button
                type="button"
                className={busy ? 'ai-agent-danger' : undefined}
                onClick={busy ? () => send({ type: 'stop' }) : handleSend}
                disabled={
                  busy ? false : (!input.trim() && attachments.length === 0 && !queued) || !view.ssh
                }
                title={busy ? 'Stop the current run' : 'Send message'}
              >
                {busy ? 'Stop' : 'Send'}
              </button>
            </div>
            {providerOptions ? (
              <div className="ai-agent-footer-controls">{providerOptions}</div>
            ) : null}
          </div>
        </>
    </div>
  )
}
