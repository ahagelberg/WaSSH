import type { ReactElement } from 'react'
import { useEffect, useMemo, useState } from 'react'
import type { AppSettings, AppTheme } from '@shared/types'
import type { PluginListItem, PluginMacroButton, PluginSettingsField } from '@shared/plugins'
import { mergePluginSettings } from '@shared/plugins'
import SettingsDialog, { type SettingsSection } from './SettingsDialog'

interface Props {
  settings: AppSettings
  onChange: (partial: Partial<AppSettings>) => void
  onClose: () => void
}

function optionsPayload(draft: AppSettings): Partial<AppSettings> {
  return {
    reconnectOnStartup: draft.reconnectOnStartup,
    autoReconnectOnDrop: draft.autoReconnectOnDrop,
    reconnectMaxAttempts: draft.reconnectMaxAttempts,
    termType: draft.termType,
    theme: draft.theme,
    enabledPlugins: draft.enabledPlugins,
    pluginSettings: draft.pluginSettings
  }
}

function MacroListEditor({
  value,
  onChange
}: {
  value: PluginMacroButton[]
  onChange: (next: PluginMacroButton[]) => void
}): ReactElement {
  return (
    <div className="plugin-settings-macros">
      {value.map((btn, index) => (
        <div key={btn.id} className="plugin-settings-macro-row">
          <input
            aria-label="Label"
            placeholder="Label"
            value={btn.label}
            onChange={(e) => {
              const next = value.slice()
              next[index] = { ...btn, label: e.target.value }
              onChange(next)
            }}
          />
          <input
            aria-label="Text to send"
            placeholder="Text to send"
            value={btn.text}
            onChange={(e) => {
              const next = value.slice()
              next[index] = { ...btn, text: e.target.value }
              onChange(next)
            }}
          />
          <input
            aria-label="Hotkey"
            placeholder="Hotkey"
            value={btn.hotkey}
            onChange={(e) => {
              const next = value.slice()
              next[index] = { ...btn, hotkey: e.target.value }
              onChange(next)
            }}
          />
          <button
            type="button"
            onClick={() => onChange(value.filter((_, i) => i !== index))}
          >
            Remove
          </button>
        </div>
      ))}
      <button
        type="button"
        onClick={() =>
          onChange([
            ...value,
            {
              id: crypto.randomUUID(),
              label: 'New',
              text: '',
              hotkey: ''
            }
          ])
        }
      >
        Add button
      </button>
    </div>
  )
}

function PluginFieldEditor({
  field,
  value,
  onChange
}: {
  field: PluginSettingsField
  value: unknown
  onChange: (value: unknown) => void
}): ReactElement {
  if (field.type === 'boolean') {
    return (
      <input
        type="checkbox"
        checked={Boolean(value)}
        onChange={(e) => onChange(e.target.checked)}
      />
    )
  }
  if (field.type === 'number') {
    return (
      <input
        type="number"
        value={typeof value === 'number' ? value : Number(value) || 0}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    )
  }
  if (field.type === 'macroList') {
    const list = Array.isArray(value) ? (value as PluginMacroButton[]) : []
    return <MacroListEditor value={list} onChange={onChange} />
  }
  if (field.type === 'stringList') {
    const text = Array.isArray(value) ? (value as string[]).join('\n') : ''
    return (
      <textarea
        rows={4}
        value={text}
        onChange={(e) =>
          onChange(
            e.target.value
              .split('\n')
              .map((l) => l.trimEnd())
              .filter((l, i, arr) => l.length > 0 || i < arr.length - 1)
          )
        }
      />
    )
  }
  return (
    <input
      type="text"
      value={typeof value === 'string' ? value : String(value ?? '')}
      onChange={(e) => onChange(e.target.value)}
    />
  )
}

export default function OptionsDialog({ settings, onChange, onClose }: Props) {
  const [draft, setDraft] = useState<AppSettings>(settings)
  const [plugins, setPlugins] = useState<PluginListItem[]>([])

  const patch = (partial: Partial<AppSettings>): void => {
    setDraft((prev) => ({ ...prev, ...partial }))
  }

  useEffect(() => {
    document.documentElement.dataset.theme = draft.theme
  }, [draft.theme])

  useEffect(() => {
    void window.wassh.listPlugins().then(setPlugins)
  }, [])

  const handleCancel = (): void => {
    document.documentElement.dataset.theme = settings.theme
    onClose()
  }

  const handleSave = (): void => {
    onChange(optionsPayload(draft))
    onClose()
  }

  const setPluginEnabled = (pluginId: string, enabled: boolean): void => {
    const set = new Set(draft.enabledPlugins)
    if (enabled) {
      set.add(pluginId)
    } else {
      set.delete(pluginId)
    }
    patch({ enabledPlugins: Array.from(set) })
  }

  const patchPluginSetting = (pluginId: string, key: string, value: unknown): void => {
    const current = mergePluginSettings(
      plugins.find((p) => p.id === pluginId)?.contributes.settingsSchema,
      draft.pluginSettings[pluginId]
    )
    patch({
      pluginSettings: {
        ...draft.pluginSettings,
        [pluginId]: { ...current, [key]: value }
      }
    })
  }

  const sections: SettingsSection[] = useMemo(() => {
    const base: SettingsSection[] = [
      {
        id: 'appearance',
        title: 'Appearance',
        content: (
          <>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Theme</strong>
                <span>Applies to the whole app. Font and colors are set per host or session.</span>
              </div>
              <select
                value={draft.theme}
                onChange={(e) => patch({ theme: e.target.value as AppTheme })}
              >
                <option value="dark">Dark</option>
                <option value="light">Light</option>
              </select>
            </div>
          </>
        )
      },
      {
        id: 'general',
        title: 'General',
        content: (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Reconnect sessions on startup</strong>
              <span>Re-open and reconnect all tabs that were open when WaSSH exited or crashed.</span>
            </div>
            <input
              type="checkbox"
              checked={draft.reconnectOnStartup}
              onChange={(e) => patch({ reconnectOnStartup: e.target.checked })}
            />
          </div>
        )
      },
      {
        id: 'connection',
        title: 'Connection',
        content: (
          <>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Auto-reconnect on drop</strong>
                <span>When an established session drops mid-session, retry with backoff.</span>
              </div>
              <input
                type="checkbox"
                checked={draft.autoReconnectOnDrop}
                onChange={(e) => patch({ autoReconnectOnDrop: e.target.checked })}
              />
            </div>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Max reconnect attempts</strong>
                <span>Stops auto-reconnect after this many failures.</span>
              </div>
              <input
                type="number"
                min={1}
                max={100}
                value={draft.reconnectMaxAttempts}
                onChange={(e) =>
                  patch({ reconnectMaxAttempts: Number(e.target.value) || 1 })
                }
              />
            </div>
          </>
        )
      },
      {
        id: 'terminal',
        title: 'Terminal',
        content: (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>TERM</strong>
              <span>PTY terminal type sent to the remote host. Scrollback is set per host or session.</span>
            </div>
            <input
              type="text"
              value={draft.termType}
              onChange={(e) => patch({ termType: e.target.value })}
            />
          </div>
        )
      },
      {
        id: 'plugins',
        title: 'Plugins',
        content: (
          <>
            {plugins.map((p) => (
              <div key={p.id} className="settings-row">
                <div className="settings-row-label">
                  <strong>
                    {p.name}
                    {p.source === 'external' ? ' (external)' : ''}
                  </strong>
                  <span>{p.description}</span>
                </div>
                <input
                  type="checkbox"
                  checked={draft.enabledPlugins.includes(p.id)}
                  onChange={(e) => setPluginEnabled(p.id, e.target.checked)}
                  aria-label={`Enable ${p.name}`}
                />
              </div>
            ))}
            <p className="plugin-settings-external-note">
              External plugins: none loaded. Future versions can scan userData/plugins/.
            </p>
          </>
        )
      }
    ]

    for (const plugin of plugins) {
      if (!draft.enabledPlugins.includes(plugin.id)) {
        continue
      }
      const schema = plugin.contributes.settingsSchema
      if (!schema || schema.length === 0) {
        continue
      }
      const values = mergePluginSettings(schema, draft.pluginSettings[plugin.id])
      base.push({
        id: `plugin-${plugin.id}`,
        title: plugin.contributes.settingsHeading || plugin.name,
        content: (
          <>
            {schema.map((field) => (
              <div key={field.key} className="settings-row">
                <div className="settings-row-label">
                  <strong>{field.label}</strong>
                  {field.description ? <span>{field.description}</span> : null}
                </div>
                <PluginFieldEditor
                  field={field}
                  value={values[field.key]}
                  onChange={(value) => patchPluginSetting(plugin.id, field.key, value)}
                />
              </div>
            ))}
          </>
        )
      })
    }

    return base
  }, [draft, plugins])

  return (
    <SettingsDialog
      title="Options"
      sections={sections}
      onClose={handleCancel}
      footer={
        <>
          <button type="button" onClick={handleCancel}>
            Cancel
          </button>
          <button type="button" className="primary" onClick={handleSave}>
            Save
          </button>
        </>
      }
    />
  )
}
