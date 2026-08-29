import { useEffect, useState } from 'react'
import type { PluginViewProps } from '../registry'

interface Snapshot {
  raw: string
  updatedAt: number
  error?: string
}

export default function ServerMonitorView({ tabId, pluginId }: PluginViewProps) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null)

  useEffect(() => {
    return window.wassh.onPluginMessage((ev) => {
      if (ev.tabId !== tabId || ev.pluginId !== pluginId) {
        return
      }
      const payload = ev.payload as { type?: string; snapshot?: Snapshot }
      if (payload?.type === 'stats' && payload.snapshot) {
        setSnapshot(payload.snapshot)
      }
    })
  }, [tabId, pluginId])

  return (
    <div className="plugin-panel plugin-server-monitor">
      <div className="plugin-panel-header">Server monitor</div>
      {snapshot?.error ? (
        <div className="plugin-monitor-error">{snapshot.error}</div>
      ) : null}
      <pre className="plugin-monitor-output">{snapshot?.raw || 'Waiting for stats…'}</pre>
      {snapshot?.updatedAt ? (
        <div className="plugin-monitor-meta">
          Updated {new Date(snapshot.updatedAt).toLocaleTimeString()}
        </div>
      ) : null}
    </div>
  )
}
