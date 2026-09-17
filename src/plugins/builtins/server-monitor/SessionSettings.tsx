import { useState, type ReactElement } from 'react'
import type { PluginSettingsViewProps } from '@plugin-api/renderer'
import { PluginButton } from '@plugin-api/renderer'
import { PLUGIN_ID_SERVER_MONITOR } from './id'
import { isMonitorActionResult, type MonitorRendererMessage } from './serviceProtocol'

/** WaSSH Service uninstall editor; the host renders it in a modal. */
export default function SessionSettings({ tabId, onClose }: PluginSettingsViewProps): ReactElement {
  const [sudoPrompt, setSudoPrompt] = useState(false)
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  const uninstall = async (): Promise<void> => {
    if (!tabId) {
      return
    }
    setBusy(true)
    setError('')
    try {
      const active = await window.wassh.getActivePlugins(tabId)
      if (!active.includes(PLUGIN_ID_SERVER_MONITOR)) {
        await window.wassh.activatePlugin(tabId, PLUGIN_ID_SERVER_MONITOR)
      }
      const result = await window.wassh.sendPluginMessage(tabId, PLUGIN_ID_SERVER_MONITOR, {
        type: 'uninstallService',
        password
      } satisfies MonitorRendererMessage)
      setPassword('')
      if (!isMonitorActionResult(result) || !result.ok) {
        setError(isMonitorActionResult(result) ? result.error || 'Operation failed' : 'Operation failed')
        return
      }
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <div className="settings-dialog-body monitor-service-dialog-body">
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>WaSSH Service</strong>
            <span>
              {tabId
                ? 'Remote daemon that keeps monitor history across restarts.'
                : 'Open a session to uninstall the service.'}
            </span>
          </div>
          <PluginButton
            compact
            variant="danger"
            disabled={!tabId}
            onClick={() => setSudoPrompt(true)}
          >
            Uninstall service
          </PluginButton>
        </div>
        {sudoPrompt ? (
          <form
            className="monitor-service-sudo"
            onSubmit={(e) => {
              e.preventDefault()
              void uninstall()
            }}
          >
            <label>
              Sudo password
              <input
                autoFocus
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
            <small>Kept in memory only for this operation. Leave blank for passwordless sudo.</small>
            <div className="monitor-service-actions">
              <PluginButton type="submit" compact variant="danger" disabled={busy}>
                {busy ? 'Uninstalling…' : 'Uninstall completely'}
              </PluginButton>
              <PluginButton compact disabled={busy} onClick={() => setSudoPrompt(false)}>
                Cancel
              </PluginButton>
            </div>
          </form>
        ) : null}
        {error ? <div className="plugin-monitor-error">{error}</div> : null}
      </div>
      <div className="settings-footer">
        <button type="button" onClick={onClose}>
          Close
        </button>
      </div>
    </>
  )
}
