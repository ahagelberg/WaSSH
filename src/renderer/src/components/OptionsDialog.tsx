import type { AppSettings } from '@shared/types'
import SettingsDialog, { type SettingsSection } from './SettingsDialog'

interface Props {
  settings: AppSettings
  onChange: (partial: Partial<AppSettings>) => void
  onClose: () => void
}

export default function OptionsDialog({ settings, onChange, onClose }: Props) {
  const sections: SettingsSection[] = [
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
            checked={settings.reconnectOnStartup}
            onChange={(e) => onChange({ reconnectOnStartup: e.target.checked })}
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
              checked={settings.autoReconnectOnDrop}
              onChange={(e) => onChange({ autoReconnectOnDrop: e.target.checked })}
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
              value={settings.reconnectMaxAttempts}
              onChange={(e) =>
                onChange({ reconnectMaxAttempts: Number(e.target.value) || 1 })
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
        <>
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Scrollback lines</strong>
              <span>Primary-buffer history (PuTTY-like). Alt-screen apps manage their own view.</span>
            </div>
            <input
              type="number"
              min={100}
              max={100000}
              value={settings.scrollbackLines}
              onChange={(e) =>
                onChange({ scrollbackLines: Number(e.target.value) || 100 })
              }
            />
          </div>
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Font size</strong>
              <span>Terminal font size in pixels.</span>
            </div>
            <input
              type="number"
              min={8}
              max={48}
              value={settings.fontSizePx}
              onChange={(e) => onChange({ fontSizePx: Number(e.target.value) || 8 })}
            />
          </div>
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>TERM</strong>
              <span>PTY terminal type sent to the remote host.</span>
            </div>
            <input
              type="text"
              value={settings.termType}
              onChange={(e) => onChange({ termType: e.target.value })}
            />
          </div>
        </>
      )
    },
    {
      id: 'appearance',
      title: 'Appearance',
      content: (
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>Font family</strong>
            <span>CSS font-family for the terminal.</span>
          </div>
          <input
            type="text"
            value={settings.fontFamily}
            onChange={(e) => onChange({ fontFamily: e.target.value })}
          />
        </div>
      )
    }
  ]

  return (
    <SettingsDialog
      title="Options"
      sections={sections}
      onClose={onClose}
      footer={
        <button type="button" className="primary" onClick={onClose}>
          Close
        </button>
      }
    />
  )
}
