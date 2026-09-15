import { useEffect, useState, type ReactElement } from 'react'
import {
  AI_AGENT_ANTHROPIC_BASE_URL,
  AI_AGENT_DEFAULT_PROVIDERS
} from '../../../plugins/builtins/ai-agent/defaults'
import { aiAgentVaultId } from '../../../plugins/builtins/ai-agent/id'
import {
  AI_AGENT_PROTOCOL_ANTHROPIC,
  AI_AGENT_PROTOCOL_OPENAI,
  type AiAgentProviderConfig,
  type AiAgentProviderProtocol
} from '../../../plugins/builtins/ai-agent/protocol'

const AI_AGENT_PLUGIN_ID = 'ai-agent'

interface DraftProvider extends AiAgentProviderConfig {
  hasKey: boolean
  draftKey: string
}

interface Props {
  tabId: string | null
  onClose: () => void
}

type ProviderCheckMessage = {
  kind: 'checking' | 'success' | 'error'
  text: string
}

function newProviderId(): string {
  return crypto.randomUUID()
}

const CUSTOM_TEMPLATES: AiAgentProviderConfig[] = [
  { id: 'custom-openai', name: 'OpenAI compatible', protocol: AI_AGENT_PROTOCOL_OPENAI, baseUrl: '', models: [] },
  { id: 'custom-anthropic', name: 'Anthropic compatible', protocol: AI_AGENT_PROTOCOL_ANTHROPIC, baseUrl: AI_AGENT_ANTHROPIC_BASE_URL, models: [] }
]

const PROVIDER_TEMPLATES: AiAgentProviderConfig[] = [
  ...AI_AGENT_DEFAULT_PROVIDERS.map((provider) => ({
    ...provider,
    models: [...provider.models]
  })),
  ...CUSTOM_TEMPLATES
]

const BUILTIN_PROVIDER_IDS = new Set(AI_AGENT_DEFAULT_PROVIDERS.map((provider) => provider.id))

function protocolLabel(protocol: AiAgentProviderProtocol): string {
  return protocol === AI_AGENT_PROTOCOL_ANTHROPIC ? 'Anthropic' : 'OpenAI compatible'
}

function isProvider(value: unknown): value is AiAgentProviderConfig {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  const item = value as Record<string, unknown>
  return typeof item.id === 'string' && typeof item.name === 'string' &&
    (item.protocol === AI_AGENT_PROTOCOL_OPENAI || item.protocol === AI_AGENT_PROTOCOL_ANTHROPIC) &&
    typeof item.baseUrl === 'string' && Array.isArray(item.models)
}

function toDraft(provider: AiAgentProviderConfig, hasKey: boolean): DraftProvider {
  return {
    ...provider,
    models: [...provider.models],
    hasKey,
    draftKey: ''
  }
}

export default function AiAgentProviderDialog({ tabId, onClose }: Props): ReactElement {
  const [drafts, setDrafts] = useState<DraftProvider[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [templateId, setTemplateId] = useState(PROVIDER_TEMPLATES[0].id)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [checkingProviderIds, setCheckingProviderIds] = useState<Set<string>>(() => new Set())
  const [checkMessages, setCheckMessages] = useState<Record<string, ProviderCheckMessage>>({})

  useEffect(() => {
    let cancelled = false
    void window.wassh.getPluginData(AI_AGENT_PLUGIN_ID).then((raw) => {
      if (cancelled) return
      const data = raw && typeof raw === 'object' && !Array.isArray(raw) ? raw as Record<string, unknown> : {}
      const stored = Array.isArray(data.providers) ? data.providers.filter(isProvider) : []
      const providers = stored.length > 0 ? stored : AI_AGENT_DEFAULT_PROVIDERS
      void Promise.all(providers.map(async (provider) =>
        toDraft(provider, Boolean(await window.wassh.getSecret(aiAgentVaultId(provider.id))))
      )).then((next) => {
        if (!cancelled) {
          setDrafts(next)
          setSelectedId(next[0]?.id ?? null)
          setLoading(false)
        }
      })
    })
    return () => { cancelled = true }
  }, [])

  const selected = drafts.find((provider) => provider.id === selectedId) ?? null
  const selectedIsChecking = selected ? checkingProviderIds.has(selected.id) : false
  const selectedCheckMessage = selected ? checkMessages[selected.id] : undefined
  const updateSelected = (patch: Partial<DraftProvider>): void => {
    if (!selectedId) return
    setDrafts((current) => current.map((provider) =>
      provider.id === selectedId ? { ...provider, ...patch } : provider
    ))
  }

  const checkSelected = async (): Promise<void> => {
    if (!selected || checkingProviderIds.has(selected.id)) return
    const providerId = selected.id
    setCheckingProviderIds((current) => new Set(current).add(providerId))
    setCheckMessages((current) => ({
      ...current,
      [providerId]: { kind: 'checking', text: `Checking ${selected.name || 'provider'}…` }
    }))
    try {
      if (!tabId) {
        throw new Error('Open an active session before checking a provider.')
      }
      const activePlugins = await window.wassh.getActivePlugins(tabId)
      if (!activePlugins.includes(AI_AGENT_PLUGIN_ID)) {
        await window.wassh.activatePlugin(tabId, AI_AGENT_PLUGIN_ID)
      }
      const result = await window.wassh.sendPluginMessage(tabId, AI_AGENT_PLUGIN_ID, {
        type: 'checkProvider',
        provider: { ...selected, baseUrl: selected.baseUrl.trim() }
      }) as { ok: boolean; models: string[]; message?: string }
      if (!result || typeof result.ok !== 'boolean') {
        throw new Error('The AI-agent plugin did not return a provider check result.')
      }
      if (result.ok) {
        setDrafts((current) => current.map((provider) =>
          provider.id === providerId ? { ...provider, models: result.models } : provider
        ))
        setCheckMessages((current) => ({
          ...current,
          [providerId]: {
            kind: 'success',
            text: `Provider is working. ${result.models.length} model${result.models.length === 1 ? '' : 's'} available.`
          }
        }))
      } else {
        setCheckMessages((current) => ({
          ...current,
          [providerId]: { kind: 'error', text: result.message || 'Provider check failed.' }
        }))
      }
    } catch (error) {
      setCheckMessages((current) => ({
        ...current,
        [providerId]: {
          kind: 'error',
          text: error instanceof Error ? error.message : String(error)
        }
      }))
    } finally {
      setCheckingProviderIds((current) => {
        const next = new Set(current)
        next.delete(providerId)
        return next
      })
    }
  }

  const refreshSelected = async (): Promise<void> => {
    if (!selected || !tabId || checkingProviderIds.has(selected.id)) return
    const providerId = selected.id
    setCheckingProviderIds((current) => new Set(current).add(providerId))
    setCheckMessages((current) => ({
      ...current,
      [providerId]: { kind: 'checking', text: `Refreshing ${selected.name || 'provider'} models…` }
    }))
    try {
      const result = await window.wassh.sendPluginMessage(tabId, AI_AGENT_PLUGIN_ID, {
        type: 'refreshProvider',
        provider: { ...selected, baseUrl: selected.baseUrl.trim() }
      }) as { ok: boolean; models: string[]; message?: string }
      if (!result || typeof result.ok !== 'boolean') {
        throw new Error('The AI-agent plugin did not return a model refresh result.')
      }
      if (result.ok) {
        setDrafts((current) => current.map((provider) =>
          provider.id === providerId ? { ...provider, models: result.models } : provider
        ))
        setCheckMessages((current) => ({
          ...current,
          [providerId]: { kind: 'success', text: `Model list refreshed. ${result.models.length} model${result.models.length === 1 ? '' : 's'} available.` }
        }))
      } else {
        setCheckMessages((current) => ({
          ...current,
          [providerId]: { kind: 'error', text: result.message || 'Model refresh failed.' }
        }))
      }
    } catch (error) {
      setCheckMessages((current) => ({
        ...current,
        [providerId]: { kind: 'error', text: error instanceof Error ? error.message : String(error) }
      }))
    } finally {
      setCheckingProviderIds((current) => {
        const next = new Set(current)
        next.delete(providerId)
        return next
      })
    }
  }

  const addProvider = (): void => {
    const template = PROVIDER_TEMPLATES.find((item) => item.id === templateId) ?? PROVIDER_TEMPLATES[0]
    const existing = BUILTIN_PROVIDER_IDS.has(template.id)
      ? drafts.find((provider) => provider.id === template.id)
      : undefined
    if (existing) {
      setSelectedId(existing.id)
      return
    }
    const provider = toDraft({ ...template, id: BUILTIN_PROVIDER_IDS.has(template.id) ? template.id : newProviderId() }, false)
    setDrafts((current) => [...current, provider])
    setSelectedId(provider.id)
  }

  const removeProvider = async (): Promise<void> => {
    if (!selected) return
    if (selected.hasKey) await window.wassh.deleteSecret(aiAgentVaultId(selected.id))
    setDrafts((current) => current.filter((provider) => provider.id !== selected.id))
    setSelectedId(drafts.find((provider) => provider.id !== selected.id)?.id ?? null)
  }

  const save = async (): Promise<void> => {
    setSaving(true)
    try {
      for (const provider of drafts) {
        const key = provider.draftKey.trim()
        if (key) await window.wassh.setSecret(aiAgentVaultId(provider.id), key)
      }
      const data = await window.wassh.getPluginData(AI_AGENT_PLUGIN_ID)
      const current = data && typeof data === 'object' && !Array.isArray(data) ? data as Record<string, unknown> : {}
      const providers = drafts.map(({ hasKey: _hasKey, draftKey: _draftKey, ...provider }) => ({
        ...provider,
        name: provider.name.trim() || 'Provider',
        baseUrl: provider.baseUrl.trim(),
        models: provider.models.map((model) => model.trim()).filter(Boolean)
      }))
      await window.wassh.setPluginData(AI_AGENT_PLUGIN_ID, { ...current, providers })
      window.dispatchEvent(new CustomEvent('ai-agent-providers-changed', { detail: providers }))
      onClose()
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="settings-overlay" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div className="settings-dialog ai-agent-provider-dialog" role="dialog" aria-modal="true" aria-labelledby="ai-agent-provider-title">
        <div className="settings-dialog-header">
          <h2 id="ai-agent-provider-title">AI model providers</h2>
          <button type="button" onClick={onClose} aria-label="Close">✕</button>
        </div>
        {loading ? <div className="settings-dialog-body"><p>Loading providers…</p></div> : (
          <div className="settings-dialog-body ai-agent-provider-dialog-body">
            <div className="ai-agent-provider-dialog-list">
              {drafts.map((provider) => (
                <button
                  type="button"
                  key={provider.id}
                  className={provider.id === selectedId ? 'active' : ''}
                  onClick={() => setSelectedId(provider.id)}
                >
                  <strong>{provider.name || 'Unnamed provider'}</strong>
                  <span>{protocolLabel(provider.protocol)}</span>
                </button>
              ))}
              {drafts.length === 0 ? <p>No providers configured.</p> : null}
            </div>
            <div className="ai-agent-provider-dialog-editor">
              {selected ? (
                <>
                  <label>Name<input value={selected.name} onChange={(event) => updateSelected({ name: event.target.value })} /></label>
                  <label>Protocol<select value={selected.protocol} onChange={(event) => updateSelected({ protocol: event.target.value as AiAgentProviderProtocol })}>
                    <option value={AI_AGENT_PROTOCOL_OPENAI}>OpenAI compatible</option>
                    <option value={AI_AGENT_PROTOCOL_ANTHROPIC}>Anthropic</option>
                  </select></label>
                  <label>Base URL<input value={selected.baseUrl} placeholder={selected.protocol === AI_AGENT_PROTOCOL_ANTHROPIC ? AI_AGENT_ANTHROPIC_BASE_URL : 'http://127.0.0.1:11434/v1'} onChange={(event) => updateSelected({ baseUrl: event.target.value })} /></label>
                  <label>API key<input type="password" value={selected.draftKey} placeholder={selected.hasKey ? 'Key stored — type to replace' : 'API key'} autoComplete="off" onChange={(event) => updateSelected({ draftKey: event.target.value })} /></label>
                  <div className="ai-agent-provider-model-row">
                    <label>Available models<output className="ai-agent-provider-models">{selected.models.length > 0 ? selected.models.join(', ') : 'Models load automatically when available.'}</output></label>
                    <button type="button" className="primary" disabled={selectedIsChecking || !tabId || !selected.baseUrl.trim()} onClick={() => void refreshSelected()}>
                      Refresh
                    </button>
                  </div>
                  <div className="ai-agent-provider-actions">
                    <button type="button" className="primary" disabled={selectedIsChecking || !tabId || !selected.baseUrl.trim()} onClick={() => void checkSelected()}>
                      Check
                    </button>
                    <button type="button" className="ai-agent-danger" onClick={() => void removeProvider()}>Remove provider</button>
                  </div>
                  {selectedCheckMessage ? (
                    <output className={`ai-agent-provider-check-message is-${selectedCheckMessage.kind}`}>
                      {selectedCheckMessage.text}
                    </output>
                  ) : null}
                </>
              ) : <p>Select a provider to configure it.</p>}
            </div>
          </div>
        )}
        <div className="settings-footer">
          <select aria-label="Provider template" value={templateId} onChange={(event) => setTemplateId(event.target.value)}>
            {PROVIDER_TEMPLATES.map((template) => <option key={template.id} value={template.id}>{template.name}</option>)}
          </select>
          <button type="button" onClick={addProvider}>Add provider</button>
          <span />
          <button type="button" onClick={onClose}>Cancel</button>
          <button type="button" className="primary" disabled={loading || saving} onClick={() => void save()}>Save</button>
        </div>
      </div>
    </div>
  )
}
