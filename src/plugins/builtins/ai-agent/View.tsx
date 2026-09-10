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
  type AiAgentToastPayload
} from './protocol'
import type { PluginViewProps } from '@plugin-api/renderer'

/** Streaming rows rendered between two state snapshots may not exceed this */
const STREAM_PLACEHOLDER_LIMIT = 1_000_000

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

/** Provider presets shown by the provider manager. */
const CUSTOM_OPENAI_TEMPLATE: AiAgentProviderConfig = {
  id: 'custom-openai',
  name: 'OpenAI compatible',
  protocol: AI_AGENT_PROTOCOL_OPENAI,
  baseUrl: '',
  models: []
}

const CUSTOM_ANTHROPIC_TEMPLATE: AiAgentProviderConfig = {
  id: 'custom-anthropic',
  name: 'Anthropic compatible',
  protocol: AI_AGENT_PROTOCOL_ANTHROPIC,
  baseUrl: AI_AGENT_ANTHROPIC_BASE_URL,
  models: []
}

const PROVIDER_TEMPLATES: AiAgentProviderConfig[] = [
  ...AI_AGENT_DEFAULT_PROVIDERS.map((p) => ({ ...p })),
  { ...CUSTOM_OPENAI_TEMPLATE },
  { ...CUSTOM_ANTHROPIC_TEMPLATE }
]

/** Built-in provider presets can only appear once in the provider list. */
const BUILTIN_TEMPLATE_IDS = new Set(AI_AGENT_DEFAULT_PROVIDERS.map((p) => p.id))

function newProviderId(): string {
  return crypto.randomUUID()
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

interface DraftProvider extends AiAgentProviderConfig {
  /** Runtime flag: a key is stored in the vault for this provider */
  hasKey: boolean
  /** transient key entry, never persisted in config */
  draftKey: string
}

function templateDraft(template: AiAgentProviderConfig): DraftProvider {
  return {
    ...template,
    id: BUILTIN_TEMPLATE_IDS.has(template.id) ? template.id : newProviderId(),
    models: [...template.models],
    hasKey: false,
    draftKey: ''
  }
}

function protocolLabel(protocol: AiAgentProviderProtocol): string {
  return protocol === AI_AGENT_PROTOCOL_ANTHROPIC ? 'anthropic' : 'openai-compatible'
}

function isOllamaLike(provider: {
  id: string
}): boolean {
  return provider.id === AI_AGENT_OLLAMA_PROVIDER_ID
}

function isOllamaDraft(draft: DraftProvider): boolean {
  return isOllamaLike(draft)
}

function baseHost(baseUrl: string): string {
  if (!baseUrl) {
    return 'no base URL'
  }
  try {
    return new URL(baseUrl).host
  } catch {
    return baseUrl
  }
}

interface ProviderDragState {
  id: string
  startX: number
  startY: number
  lastY: number
  active: boolean
}

type ProviderDropHint = { kind: 'before' | 'after'; id: string }

/** Locate the provider row under clientY and whether the drop lands before/after it. */
function findProviderDropHint(listEl: HTMLElement, clientY: number): ProviderDropHint | null {
  const items = listEl.querySelectorAll<HTMLElement>('.ai-agent-provider-item[data-provider-id]')
  for (const item of items) {
    const rect = item.getBoundingClientRect()
    const id = item.dataset.providerId
    if (!id) {
      continue
    }
    if (clientY >= rect.top && clientY <= rect.bottom) {
      const mid = rect.top + rect.height * PROVIDER_DROP_BEFORE_RATIO
      return clientY < mid ? { kind: 'before', id } : { kind: 'after', id }
    }
  }
  return null
}

/** Resolve the insertion index for a dragged provider id given a drop hint. */
function providerTargetInsertIndex(
  ids: string[],
  draggedId: string,
  hint: ProviderDropHint
): number {
  const without = ids.filter((id) => id !== draggedId)
  const refIdx = without.indexOf(hint.id)
  if (refIdx < 0) {
    return without.length
  }
  return hint.kind === 'before' ? refIdx : refIdx + 1
}

function providerDropClass(hint: ProviderDropHint | null, id: string): string {
  if (!hint || hint.id !== id) {
    return ''
  }
  return hint.kind === 'before' ? ' drop-before' : ' drop-after'
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
  const [input, setInput] = useState('')
  const [attach, setAttach] = useState(false)
  const [attachments, setAttachments] = useState<AiAgentChatAttachment[]>([])
  const [dropActive, setDropActive] = useState(false)
  const [gearOpen, setGearOpen] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [drafts, setDrafts] = useState<DraftProvider[]>([])
  const [expandedDraftId, setExpandedDraftId] = useState<string | null>(null)
  const [addProviderOpen, setAddProviderOpen] = useState(false)
  const [newTemplateId, setNewTemplateId] = useState(PROVIDER_TEMPLATES[0]?.id ?? '')
  const [rulesDraft, setRulesDraft] = useState('')
  const [toast, setToast] = useState<{ kind: 'error' | 'info'; text: string } | null>(null)
  const [queued, setQueued] = useState<QueuedMessage | null>(null)
  const [sudoPassword, setSudoPassword] = useState('')
  const [draggingProviderId, setDraggingProviderId] = useState<string | null>(null)
  const [providerDropHint, setProviderDropHint] = useState<ProviderDropHint | null>(null)
  const [providerDragGhost, setProviderDragGhost] = useState<{
    id: string
    x: number
    y: number
  } | null>(null)
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const sudoInputRef = useRef<HTMLInputElement>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const promptInputRef = useRef<HTMLTextAreaElement>(null)
  const providerListRef = useRef<HTMLDivElement>(null)
  const providerDragRef = useRef<ProviderDragState | null>(null)
  const providerDropHintRef = useRef<ProviderDropHint | null>(null)
  const suppressProviderClickRef = useRef(false)
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
        if (payload.runPhase !== 'ask_sudo') {
          setSudoPassword('')
        }
        return
      }
      if (payload.type === 'delta') {
        setStream((prev) => (prev + payload.text).slice(0, STREAM_PLACEHOLDER_LIMIT))
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

  providerDropHintRef.current = providerDropHint

  const conv = view.conversation
  const providers = view.providers
  const activeProvider =
    providers.find((p) => p.id === conv?.activeProviderId) ?? providers[0]
  const activeModel = conv?.activeModel || activeProvider?.models[0] || ''
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
      showToast('error', 'No model providers configured — open the gear menu and add one.')
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
    const model = provider?.models[0] ?? ''
    send({ type: 'select', providerId, model })
    if (provider && isOllamaLike(provider)) {
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
                activeModel: model
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
                  activeModel: model
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

  const openGear = (): void => {
    setDrafts(
      providers.map((p) => ({
        ...p,
        models: [...p.models],
        hasKey: view.providerKeys.includes(p.id),
        draftKey: ''
      }))
    )
    setExpandedDraftId(null)
    setAddProviderOpen(false)
    setRulesDraft(view.rules)
    setGearOpen(true)
  }

  const updateDraft = (index: number, patch: Partial<DraftProvider>): void => {
    setDrafts((prev) => prev.map((d, i) => (i === index ? { ...d, ...patch } : d)))
  }

  const addDraft = (templateId: string): void => {
    const template = PROVIDER_TEMPLATES.find((t) => t.id === templateId) ?? PROVIDER_TEMPLATES[0]
    if (!template) {
      return
    }
    if (BUILTIN_TEMPLATE_IDS.has(template.id)) {
      const existing = drafts.find((d) => d.id === template.id)
      if (existing) {
        setExpandedDraftId(existing.id)
        setAddProviderOpen(false)
        return
      }
    }
    const draft = templateDraft(template)
    setDrafts((prev) => [...prev, draft])
    setExpandedDraftId(draft.id)
    setAddProviderOpen(false)
  }

  const removeDraft = (index: number): void => {
    const removed = drafts[index]
    if (removed?.hasKey) {
      void window.wassh.deleteSecret(aiAgentVaultId(removed.id))
    }
    setDrafts((prev) => prev.filter((_, i) => i !== index))
  }

  const reorderDrafts = (draggedId: string, hint: ProviderDropHint): void => {
    setDrafts((prev) => {
      const ids = prev.map((d) => d.id)
      const insertIndex = providerTargetInsertIndex(ids, draggedId, hint)
      const next = prev.filter((d) => d.id !== draggedId)
      const dragged = prev.find((d) => d.id === draggedId)
      if (!dragged) {
        return prev
      }
      next.splice(insertIndex, 0, dragged)
      return next
    })
  }

  const saveDraftKey = async (index: number): Promise<void> => {
    const draft = drafts[index]
    const key = draft.draftKey.trim()
    if (!key) {
      return
    }
    await window.wassh.setSecret(aiAgentVaultId(draft.id), key)
    updateDraft(index, { hasKey: true, draftKey: '' })
    send({ type: 'sync' })
    showToast('info', 'API key saved (encrypted).')
  }

  const removeDraftKey = async (index: number): Promise<void> => {
    const draft = drafts[index]
    await window.wassh.deleteSecret(aiAgentVaultId(draft.id))
    updateDraft(index, { hasKey: false, draftKey: '' })
    send({ type: 'sync' })
  }

  const saveRules = (): void => {
    send({ type: 'rulesChanged', rules: rulesDraft })
    showToast('info', 'Rules saved.')
  }

  const saveProviders = (): void => {
    const next: AiAgentProviderConfig[] = drafts.map((d) => ({
      id: d.id || newProviderId(),
      name: d.name.trim() || 'Provider',
      protocol: d.protocol,
      baseUrl: d.baseUrl.trim(),
      models: d.models.map((m) => m.trim()).filter((m) => m.length > 0)
    }))
    send({ type: 'providersChanged', providers: next })
    const ollama = next.find((p) => isOllamaLike(p))
    if (ollama) {
      send({ type: 'refreshModels', providerId: ollama.id })
    }
    setGearOpen(false)
  }

  const closeGear = (): void => {
    if (gearOpen) {
      const ollama = providers.find((p) => isOllamaLike(p))
      if (ollama) {
        send({ type: 'refreshModels', providerId: ollama.id })
      }
    }
    setGearOpen(false)
  }

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
            <span className="ai-agent-tool-status">ran</span>
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
  }, [messages.length, stream, view.runPhase, gearOpen, historyOpen])

  useEffect(() => {
    const el = promptInputRef.current
    if (!el) {
      return
    }
    el.style.height = 'auto'
    el.style.height = `${el.scrollHeight}px`
  }, [input])

  useEffect(() => {
    const onMove = (e: PointerEvent): void => {
      const drag = providerDragRef.current
      if (!drag) {
        return
      }
      if (!drag.active) {
        const dx = e.clientX - drag.startX
        const dy = e.clientY - drag.startY
        if (dx * dx + dy * dy < PROVIDER_DRAG_THRESHOLD_PX * PROVIDER_DRAG_THRESHOLD_PX) {
          return
        }
        drag.active = true
        setDraggingProviderId(drag.id)
        setProviderDragGhost({ id: drag.id, x: e.clientX, y: e.clientY })
      }
      drag.lastY = e.clientY
      setProviderDragGhost({ id: drag.id, x: e.clientX, y: e.clientY })
      const list = providerListRef.current
      if (!list) {
        return
      }
      const hint = findProviderDropHint(list, e.clientY)
      providerDropHintRef.current = hint
      setProviderDropHint(hint)
    }

    const onUp = (): void => {
      const drag = providerDragRef.current
      if (drag) {
        if (drag.active) {
          suppressProviderClickRef.current = true
          const list = providerListRef.current
          const hint =
            providerDropHintRef.current ?? (list ? findProviderDropHint(list, drag.lastY) : null)
          if (hint) {
            reorderDrafts(drag.id, hint)
          }
          window.setTimeout(() => {
            suppressProviderClickRef.current = false
          }, 0)
        }
        providerDragRef.current = null
        setDraggingProviderId(null)
        setProviderDragGhost(null)
        setProviderDropHint(null)
      }
    }

    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', onUp)
    return () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      window.removeEventListener('pointercancel', onUp)
    }
  }, [])

  const phaseClass = view.runPhase
  const providerOptions =
    providers.length > 0 ? (
      <>
        <select
          className="ai-agent-provider-select"
          value={activeProvider?.id ?? ''}
          onChange={(e) => selectProvider(e.target.value)}
          onClick={() => {
            if (activeProvider && isOllamaLike(activeProvider)) {
              send({ type: 'refreshModels', providerId: activeProvider.id })
            }
          }}
          onMouseDown={() => {
            if (activeProvider && isOllamaLike(activeProvider)) {
              send({ type: 'refreshModels', providerId: activeProvider.id })
            }
          }}
          title="Model provider"
        >
          {providers.map((p) => (
            <option
              key={p.id}
              value={p.id}
              onClick={() => {
                if (isOllamaLike(p)) {
                  send({ type: 'refreshModels', providerId: p.id })
                }
              }}
            >
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

  const draggingDraft = draggingProviderId
    ? drafts.find((d) => d.id === draggingProviderId)
    : undefined

  return (
    <div className="plugin-panel ai-agent">
      {gearOpen ? (
        <div className="ai-agent-gear">
          <div className="ai-agent-pane-label">Model providers</div>
          <div className="ai-agent-provider-add">
            {addProviderOpen ? (
              <>
                <select
                  aria-label="Provider type"
                  value={newTemplateId}
                  onChange={(e) => setNewTemplateId(e.target.value)}
                >
                  {PROVIDER_TEMPLATES.map((template) => (
                    <option key={template.id} value={template.id}>
                      {template.name}
                    </option>
                  ))}
                </select>
                <button type="button" onClick={() => addDraft(newTemplateId)}>
                  Add
                </button>
                <button type="button" onClick={() => setAddProviderOpen(false)}>
                  Cancel
                </button>
              </>
            ) : (
              <button type="button" onClick={() => setAddProviderOpen(true)}>
                + Add provider
              </button>
            )}
          </div>
          <div
            className={`ai-agent-provider-list${draggingProviderId ? ' is-dragging' : ''}`}
            ref={providerListRef}
          >
            {drafts.length === 0 ? (
              <div className="ai-agent-provider-empty">
                No providers configured yet. Add an OpenAI-compatible or Anthropic provider.
              </div>
            ) : (
              drafts.map((draft, index) => {
                const open = expandedDraftId === draft.id
                const dropClass = providerDropClass(providerDropHint, draft.id)
                return (
                  <div
                    key={draft.id}
                    className={`ai-agent-provider-item${open ? ' open' : ''}${draggingProviderId === draft.id ? ' dragging' : ''}${dropClass}`}
                    data-provider-id={draft.id}
                  >
                    <div className="ai-agent-provider-item-row">
                      <div
                        className="ai-agent-provider-item-main"
                        role="button"
                        tabIndex={0}
                        onPointerDown={(e) => {
                          if (e.button !== 0) {
                            return
                          }
                          providerDragRef.current = {
                            id: draft.id,
                            startX: e.clientX,
                            startY: e.clientY,
                            lastY: e.clientY,
                            active: false
                          }
                          e.currentTarget.setPointerCapture(e.pointerId)
                        }}
                        onPointerUp={(e) => {
                          if (e.currentTarget.hasPointerCapture(e.pointerId)) {
                            e.currentTarget.releasePointerCapture(e.pointerId)
                          }
                        }}
                        onPointerCancel={(e) => {
                          if (e.currentTarget.hasPointerCapture(e.pointerId)) {
                            e.currentTarget.releasePointerCapture(e.pointerId)
                          }
                        }}
                        onClick={() => {
                          if (suppressProviderClickRef.current) {
                            return
                          }
                          setExpandedDraftId(open ? null : draft.id)
                        }}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter' || e.key === ' ') {
                            e.preventDefault()
                            setExpandedDraftId(open ? null : draft.id)
                          }
                        }}
                        title="Click to edit, drag to reorder"
                      >
                        <span className="ai-agent-provider-item-name">
                          {draft.name || 'Unnamed provider'}
                        </span>
                        <span className="ai-agent-provider-item-meta">
                          {protocolLabel(draft.protocol)} · {baseHost(draft.baseUrl)} ·{' '}
                          {draft.models.length} model{draft.models.length === 1 ? '' : 's'}
                          {draft.hasKey ? ' · key set' : ''}
                        </span>
                      </div>
                      <span className="ai-agent-provider-item-actions">
                        <button
                          type="button"
                          className="ai-agent-danger"
                          title="Remove provider"
                          onClick={() => removeDraft(index)}
                        >
                          Remove
                        </button>
                      </span>
                    </div>
                    {open ? (
                      <div className="ai-agent-provider-editor">
                <div className="field-row">
                  <label htmlFor={`ai-agent-provider-name-${draft.id}`}>Name</label>
                  <input
                    id={`ai-agent-provider-name-${draft.id}`}
                    value={draft.name}
                    placeholder="Provider name"
                    onChange={(e) => updateDraft(index, { name: e.target.value })}
                  />
                </div>
                {isOllamaDraft(draft) ? null : (
                  <div className="field-row">
                    <label htmlFor={`ai-agent-provider-protocol-${draft.id}`}>Protocol</label>
                    <select
                      id={`ai-agent-provider-protocol-${draft.id}`}
                      value={draft.protocol}
                      onChange={(e) =>
                        updateDraft(index, {
                          protocol:
                            e.target.value === AI_AGENT_PROTOCOL_ANTHROPIC
                              ? AI_AGENT_PROTOCOL_ANTHROPIC
                              : AI_AGENT_PROTOCOL_OPENAI
                        })
                      }
                    >
                      <option value={AI_AGENT_PROTOCOL_OPENAI}>OpenAI compatible</option>
                      <option value={AI_AGENT_PROTOCOL_ANTHROPIC}>Anthropic</option>
                    </select>
                  </div>
                )}
                <div className="field-row">
                  <label htmlFor={`ai-agent-provider-baseurl-${draft.id}`}>Base URL</label>
                  <input
                    id={`ai-agent-provider-baseurl-${draft.id}`}
                    value={draft.baseUrl}
                    placeholder={
                      draft.protocol === AI_AGENT_PROTOCOL_ANTHROPIC
                        ? AI_AGENT_ANTHROPIC_BASE_URL
                        : 'http://127.0.0.1:11434/v1'
                    }
                    onChange={(e) => updateDraft(index, { baseUrl: e.target.value })}
                  />
                </div>
                {isOllamaDraft(draft) ? null : (
                  <div className="field-row">
                    <label htmlFor={`ai-agent-provider-models-${draft.id}`}>Models</label>
                    <textarea
                      id={`ai-agent-provider-models-${draft.id}`}
                      rows={2}
                      value={draft.models.join('\n')}
                      placeholder={'Model id, one per line'}
                      onChange={(e) =>
                        updateDraft(index, {
                          models: e.target.value
                            .split('\n')
                            .map((l) => l.trim())
                            .filter((l) => l.length > 0)
                        })
                      }
                    />
                  </div>
                )}
                {isOllamaDraft(draft) ? null : (
                  <div className="field-row">
                    <label htmlFor={`ai-agent-provider-key-${draft.id}`}>API key</label>
                    <div className="ai-agent-provider-key">
                      <input
                        id={`ai-agent-provider-key-${draft.id}`}
                        type="password"
                        value={draft.draftKey}
                        placeholder={draft.hasKey ? 'Key stored — type to replace' : 'API key'}
                        autoComplete="off"
                        onChange={(e) => updateDraft(index, { draftKey: e.target.value })}
                      />
                      <button
                        type="button"
                        onClick={() => void saveDraftKey(index)}
                        disabled={!draft.draftKey}
                      >
                        Save
                      </button>
                      {draft.hasKey ? (
                        <button
                          type="button"
                          className="ai-agent-danger"
                          onClick={() => void removeDraftKey(index)}
                        >
                          Clear key
                        </button>
                      ) : null}
                    </div>
                  </div>
                )}
                      </div>
                    ) : null}
                  </div>
                )
              })
            )}
          </div>
          {providerDragGhost && draggingDraft ? (
            <div
              className="ai-agent-provider-drag-ghost"
              style={{
                left: providerDragGhost.x + PROVIDER_DRAG_GHOST_OFFSET_X_PX,
                top: providerDragGhost.y + PROVIDER_DRAG_GHOST_OFFSET_Y_PX
              }}
            >
              {draggingDraft.name || 'Unnamed provider'}
            </div>
          ) : null}
          <div className="ai-agent-rules">
            <div className="ai-agent-pane-label">Agent rules (all providers)</div>
            <textarea
              aria-label="Agent rules"
              rows={6}
              value={rulesDraft}
              placeholder={'One rule per line or free-form instructions, e.g.\n- Always run tests after changes\n- Never modify files outside the project'}
              onChange={(e) => setRulesDraft(e.target.value)}
            />
            <div className="ai-agent-rules-hint">
              Added to every prompt. A remote <code>.wasshrules</code> file in the working
              directory is included automatically.
            </div>
            <button
              type="button"
              onClick={saveRules}
              disabled={rulesDraft === view.rules}
            >
              Save rules
            </button>
          </div>
          <div className="ai-agent-gear-actions">
            <button type="button" onClick={saveProviders}>
              Done
            </button>
            <button type="button" onClick={closeGear}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <>
          <div className="ai-agent-header">
            <div className="ai-agent-header-controls">
              <button
                type="button"
                className="ai-agent-gear-btn"
                title="Chat history"
                onClick={() => {
                  setGearOpen(false)
                  setHistoryOpen((prev) => !prev)
                }}
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
              <button
                type="button"
                className="ai-agent-gear-btn ai-agent-config-btn"
                title="Providers & API keys"
                onClick={() => {
                  setHistoryOpen(false)
                  openGear()
                }}
              >
                ⚙
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
                  ? 'No model providers configured yet. Open the gear menu (⚙) and add one.'
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
      )}
    </div>
  )
}
