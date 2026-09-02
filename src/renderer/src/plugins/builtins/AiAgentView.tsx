import { useEffect, useMemo, useRef, useState, type ReactElement } from 'react'
import {
  AI_AGENT_ANTHROPIC_BASE_URL,
  AI_AGENT_ANTHROPIC_PROVIDER_ID,
  AI_AGENT_DEFAULT_PROVIDERS,
  AI_AGENT_PROTOCOL_ANTHROPIC,
  AI_AGENT_PROTOCOL_OPENAI,
  AI_AGENT_SETTING_HOST_ALLOW_RULES,
  AI_AGENT_SETTING_HOST_DENY_RULES,
  PLUGIN_ID_AI_AGENT,
  type AiAgentApprovalDecision,
  type AiAgentConversation,
  type AiAgentConversationMsg,
  type AiAgentConversationToolMsg,
  type AiAgentProviderConfig,
  type AiAgentProviderProtocol,
  type AiAgentStateSnapshot
} from '@shared/plugins'
import type { PluginViewProps } from '../registry'

/** Vault id namespace for provider API keys */
const AI_AGENT_KEY_VAULT_PREFIX = 'ai-agent:'

/** Streaming rows rendered between two state snapshots may not exceed this */
const STREAM_PLACEHOLDER_LIMIT = 1_000_000

/** New provider templates: presets plus generic types */
const CUSTOM_OPENAI_TEMPLATE: AiAgentProviderConfig = {
  id: 'custom-openai',
  name: 'OpenAI compatible',
  protocol: AI_AGENT_PROTOCOL_OPENAI,
  baseUrl: '',
  models: [],
  hasKey: false
}

const CUSTOM_ANTHROPIC_TEMPLATE: AiAgentProviderConfig = {
  id: 'custom-anthropic',
  name: 'Anthropic compatible',
  protocol: AI_AGENT_PROTOCOL_ANTHROPIC,
  baseUrl: AI_AGENT_ANTHROPIC_BASE_URL,
  models: [],
  hasKey: false
}

const PROVIDER_TEMPLATES: AiAgentProviderConfig[] = [
  ...AI_AGENT_DEFAULT_PROVIDERS.map((p) => ({ ...p })),
  { ...CUSTOM_OPENAI_TEMPLATE },
  { ...CUSTOM_ANTHROPIC_TEMPLATE }
]

/** Generic id factory for provider manager drafts */
function newProviderId(): string {
  return crypto.randomUUID()
}

type PhaseLabel = Record<string, string>

const PHASE_LABELS: PhaseLabel = {
  idle: 'Ready',
  running: 'Running…',
  ask: 'Waiting for approval',
  paused: 'Paused',
  no_session: 'No SSH session'
}

interface DraftProvider extends AiAgentProviderConfig {
  /** transient key entry, never persisted in config */
  draftKey: string
}

function templateDraft(template: AiAgentProviderConfig): DraftProvider {
  return {
    ...template,
    id: newProviderId(),
    models: [...template.models],
    draftKey: ''
  }
}

function protocolLabel(protocol: AiAgentProviderProtocol): string {
  return protocol === AI_AGENT_PROTOCOL_ANTHROPIC ? 'anthropic' : 'openai-compatible'
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

function vaultIdFor(providerId: string): string {
  return `${AI_AGENT_KEY_VAULT_PREFIX}${providerId}`
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

/** Render simple inline code / code fences without a markdown dependency */
function formatText(text: string): ReactElement[] {
  const parts = text.split(/```/)
  const out: ReactElement[] = []
  for (let i = 0; i < parts.length; i += 1) {
    if (i % 2 === 1) {
      out.push(
        <pre key={i} className="ai-agent-code">
          {parts[i]}
        </pre>
      )
    } else if (parts[i]) {
      out.push(<span key={i}>{parts[i]}</span>)
    }
  }
  return out
}

interface ViewState {
  providers: AiAgentProviderConfig[]
  conversation: AiAgentConversation | null
  runPhase: AiAgentStateSnapshot['runPhase']
  hostLabel: string
  ssh: boolean
  pendingApproval: AiAgentStateSnapshot['pendingApproval']
  rules: string
  lastError?: string
}

function emptyViewState(): ViewState {
  return {
    providers: [],
    conversation: null,
    runPhase: 'no_session',
    hostLabel: '',
    ssh: false,
    pendingApproval: null,
    rules: ''
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
  const [gearOpen, setGearOpen] = useState(false)
  const [drafts, setDrafts] = useState<DraftProvider[]>([])
  const [expandedDraftId, setExpandedDraftId] = useState<string | null>(null)
  const [newTemplateId, setNewTemplateId] = useState(PROVIDER_TEMPLATES[0]?.id ?? '')
  const [rulesDraft, setRulesDraft] = useState('')
  const [toast, setToast] = useState<{ kind: 'error' | 'info'; text: string } | null>(null)
  const secretCache = useRef(new Map<string, string>())
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

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

  const sendCachedKeys = (providers: AiAgentProviderConfig[]): void => {
    for (const provider of providers) {
      if (provider.hasKey) {
        continue
      }
      const cached = secretCache.current.get(provider.id)
      if (cached) {
        send({ type: 'setApiKey', providerId: provider.id, apiKey: cached })
      }
    }
  }

  const loadSecrets = async (providers: AiAgentProviderConfig[]): Promise<void> => {
    for (const provider of providers) {
      const secret = await window.wassh.getSecret(vaultIdFor(provider.id))
      if (secret) {
        secretCache.current.set(provider.id, secret)
      }
    }
    sendCachedKeys(providers)
  }

  useEffect(() => {
    const off = window.wassh.onPluginMessage((ev) => {
      if (ev.tabId !== tabId || ev.pluginId !== pluginId) {
        return
      }
      const payload = ev.payload as
        | AiAgentStateSnapshot
        | { type: 'delta'; text: string }
        | { type: 'toast'; kind: 'error' | 'info'; text: string }
        | null
      if (!payload || typeof payload !== 'object' || !('type' in payload)) {
        return
      }
      if (payload.type === 'state') {
        setView({
          providers: payload.providers,
          conversation: payload.conversation,
          runPhase: payload.runPhase,
          hostLabel: payload.hostLabel,
          ssh: payload.ssh,
          pendingApproval: payload.pendingApproval,
          rules: payload.rules,
          lastError: payload.lastError
        })
        setStream('')
        void loadSecrets(payload.providers)
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

  const conv = view.conversation
  const providers = view.providers
  const activeProvider =
    providers.find((p) => p.id === conv?.activeProviderId) ?? providers[0]
  const activeModel = conv?.activeModel || activeProvider?.models[0] || ''

  const selectProvider = (providerId: string): void => {
    const provider = providers.find((p) => p.id === providerId)
    const model = provider?.models[0] ?? ''
    send({ type: 'select', providerId, model })
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
    if (!text) {
      return
    }
    if (providers.length === 0) {
      showToast('error', 'No model providers configured — open the gear menu and add one.')
      return
    }
    if (!activeProvider) {
      return
    }
    if (!activeModel) {
      showToast('error', 'Pick a model first.')
      return
    }
    if (!view.ssh) {
      showToast('error', 'The AI agent needs an SSH session to run commands.')
      return
    }
    send({
      type: 'chat',
      providerId: activeProvider.id,
      model: activeModel,
      text,
      attachTerminal: attach
    })
    setInput('')
    setAttach(false)
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
    if (decision === 'allowAlways') {
      patchHostRules(AI_AGENT_SETTING_HOST_ALLOW_RULES, request.command)
    } else if (decision === 'denyAlways') {
      patchHostRules(AI_AGENT_SETTING_HOST_DENY_RULES, request.command)
    }
    send({ type: 'approval', requestId: request.requestId, decision })
  }

  const canResume =
    view.runPhase === 'idle' &&
    conv !== null &&
    conv.messages.length > 0 &&
    conv.messages[conv.messages.length - 1].role === 'user'

  const openGear = (): void => {
    setDrafts(providers.map((p) => ({ ...p, models: [...p.models], draftKey: '' })))
    setExpandedDraftId(null)
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
    const draft = templateDraft(template)
    setDrafts((prev) => [...prev, draft])
    setExpandedDraftId(draft.id)
  }

  const removeDraft = (index: number): void => {
    const removed = drafts[index]
    if (removed?.hasKey) {
      void window.wassh.deleteSecret(vaultIdFor(removed.id))
      secretCache.current.delete(removed.id)
    }
    setDrafts((prev) => prev.filter((_, i) => i !== index))
  }

  const moveDraft = (index: number, delta: number): void => {
    const target = index + delta
    if (target < 0 || target >= drafts.length) {
      return
    }
    setDrafts((prev) => {
      const next = prev.slice()
      const [item] = next.splice(index, 1)
      next.splice(target, 0, item)
      return next
    })
  }

  const saveDraftKey = async (index: number): Promise<void> => {
    const draft = drafts[index]
    const key = draft.draftKey.trim()
    if (!key) {
      return
    }
    await window.wassh.setSecret(vaultIdFor(draft.id), key)
    secretCache.current.set(draft.id, key)
    send({ type: 'setApiKey', providerId: draft.id, apiKey: key })
    updateDraft(index, { hasKey: true, draftKey: '' })
    showToast('info', 'API key saved (encrypted).')
  }

  const removeDraftKey = async (index: number): Promise<void> => {
    const draft = drafts[index]
    await window.wassh.deleteSecret(vaultIdFor(draft.id))
    secretCache.current.delete(draft.id)
    send({ type: 'clearApiKey', providerId: draft.id })
    updateDraft(index, { hasKey: false, draftKey: '' })
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
      models: d.models.map((m) => m.trim()).filter((m) => m.length > 0),
      hasKey: d.hasKey
    }))
    send({ type: 'providersChanged', providers: next })
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
            {msg.usedTerminalContext ? (
              <span className="ai-agent-ctx-tag" title="Recent terminal output was attached">
                +terminal
              </span>
            ) : null}
          </div>
          <div className="ai-agent-user-text">{formatText(msg.text)}</div>
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
        toolChildren.push(
          <div key={tc.id} className="ai-agent-tool">
            <div className="ai-agent-tool-command">$ {tc.command}</div>
            {body}
          </div>
        )
      }
      messageRows.push(
        <div key={i} className="ai-agent-msg ai-agent-assistant">
          {msg.text ? <div className="ai-agent-assistant-text">{formatText(msg.text)}</div> : null}
          {toolChildren}
          {msg.stopped ? <div className="ai-agent-interrupted">interrupted</div> : null}
        </div>
      )
      continue
    }
    messageRows.push(
      <div key={i} className="ai-agent-tool">
        <div className="ai-agent-tool-command">$ {msg.command}</div>
        <span className={`ai-agent-tool-outcome ${msg.outcome}`}>{outcomeLabel(msg.outcome)}</span>
      </div>
    )
  }

  const running = view.runPhase === 'running'
  if (running && stream) {
    messageRows.push(
      <div key="stream" className="ai-agent-msg ai-agent-assistant">
        <div className="ai-agent-assistant-text">{formatText(stream)}</div>
      </div>
    )
  }

  useEffect(() => {
    const el = messagesRef.current
    if (el) {
      el.scrollTop = el.scrollHeight
    }
  }, [messages.length, stream, view.runPhase, gearOpen])

  const phaseClass = view.runPhase
  const busy = view.runPhase === 'running' || view.runPhase === 'ask'
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
      {gearOpen ? (
        <div className="ai-agent-gear">
          <div className="ai-agent-pane-label">Model providers</div>
          <div className="ai-agent-provider-add">
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
              Add provider
            </button>
          </div>
          <div className="ai-agent-provider-list">
            {drafts.length === 0 ? (
              <div className="ai-agent-provider-empty">
                No providers configured yet. Add an OpenAI-compatible or Anthropic provider.
              </div>
            ) : (
              drafts.map((draft, index) => {
                const open = expandedDraftId === draft.id
                return (
                  <div key={draft.id} className={`ai-agent-provider-item${open ? ' open' : ''}`}>
                    <div className="ai-agent-provider-item-row">
                      <div
                        className="ai-agent-provider-item-main"
                        role="button"
                        tabIndex={0}
                        onClick={() => setExpandedDraftId(open ? null : draft.id)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter' || e.key === ' ') {
                            e.preventDefault()
                            setExpandedDraftId(open ? null : draft.id)
                          }
                        }}
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
                          title="Move up"
                          disabled={index === 0}
                          onClick={() => moveDraft(index, -1)}
                        >
                          ↑
                        </button>
                        <button
                          type="button"
                          title="Move down"
                          disabled={index === drafts.length - 1}
                          onClick={() => moveDraft(index, 1)}
                        >
                          ↓
                        </button>
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
                <input
                  aria-label="Name"
                  value={draft.name}
                  placeholder="Provider name"
                  onChange={(e) => updateDraft(index, { name: e.target.value })}
                />
                <select
                  aria-label="Protocol"
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
                <input
                  aria-label="Base URL"
                  value={draft.baseUrl}
                  placeholder={
                    draft.protocol === AI_AGENT_PROTOCOL_ANTHROPIC
                      ? AI_AGENT_ANTHROPIC_BASE_URL
                      : 'http://127.0.0.1:11434/v1'
                  }
                  onChange={(e) => updateDraft(index, { baseUrl: e.target.value })}
                />
                <textarea
                  aria-label="Models"
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
                <div className="ai-agent-provider-key">
                  <input
                    aria-label="API key"
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
                    ) : null}
                  </div>
                )
              })
            )}
          </div>
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
            <button type="button" onClick={() => setGearOpen(false)}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <>
          <div className="ai-agent-header">
            <span className="ai-agent-host" title={view.hostLabel}>
              {view.hostLabel || 'No session'}
            </span>
            <span className={`ai-agent-phase ${phaseClass}`}>
              {PHASE_LABELS[view.runPhase] ?? view.runPhase}
            </span>
            <div className="ai-agent-header-controls">
              {providerOptions}
              <button
                type="button"
                className="ai-agent-gear-btn"
                title="Providers & API keys"
                onClick={openGear}
              >
                ⚙
              </button>
              <button
                type="button"
                onClick={() => {
                  if (activeProvider) {
                    send({ type: 'newChat', providerId: activeProvider.id, model: activeModel })
                  }
                }}
                disabled={busy}
                title="Start a new conversation"
              >
                New
              </button>
              {busy ? (
                <button type="button" className="ai-agent-danger" onClick={() => send({ type: 'stop' })}>
                  Stop
                </button>
              ) : null}
            </div>
          </div>

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
            {canResume ? (
              <div className="ai-agent-resume">
                The previous run was interrupted.
                <button type="button" onClick={() => send({ type: 'resume' })}>
                  Continue
                </button>
              </div>
            ) : null}
          </div>

          {view.runPhase === 'ask' && view.pendingApproval ? (
            <div className="ai-agent-approval">
              <div className="ai-agent-approval-title">Approve command?</div>
              <pre className="ai-agent-approval-command">{view.pendingApproval.command}</pre>
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

          {toast ? (
            <div className={`ai-agent-toast ${toast.kind}`}>
              <span>{toast.text}</span>
              <button type="button" onClick={() => setToast(null)}>
                ×
              </button>
            </div>
          ) : null}

          <div className="ai-agent-inputbar">
            <button
              type="button"
              className={`ai-agent-attach${attach ? ' active' : ''}`}
              title="Attach recent terminal output to the next message"
              onClick={() => setAttach((prev) => !prev)}
            >
              ⌁
            </button>
            <textarea
              className="ai-agent-input"
              value={input}
              placeholder="Message the agent… (Shift+Enter for newline)"
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  handleSend()
                }
              }}
            />
            <button type="button" onClick={handleSend} disabled={!input.trim() || !view.ssh || busy}>
              Send
            </button>
          </div>
        </>
      )}
    </div>
  )
}


