import { useEffect, useMemo, useState } from 'react'
import type { AppSettings, AppTheme } from '@shared/types'
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
    theme: draft.theme
  }
}

export default function OptionsDialog({ settings, onChange, onClose }: Props) {
  const [draft, setDraft] = useState<AppSettings>(settings)

  const patch = (partial: Partial<AppSettings>): void => {
    setDraft((prev) => ({ ...prev, ...partial }))
  }

  useEffect(() => {
    document.documentElement.dataset.theme = draft.theme
  }, [draft.theme])

  const handleCancel = (): void => {
    document.documentElement.dataset.theme = settings.theme
    onClose()
  }

  const handleSave = (): void => {
    onChange(optionsPayload(draft))
    onClose()
  }

  const sections: SettingsSection[] = useMemo(
    () => [
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
      }
    ],
    [draft]
  )

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
