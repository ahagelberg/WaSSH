import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from 'react'
import { PluginButton } from '@plugin-api/renderer'
import { DOCKER_MAX_LOG_CHARS, DOCKER_STATS_HISTORY } from './defaults'
import {
  isDockerMainMessage,
  type DockerContainerAction,
  type DockerContainerDetailView,
  type DockerContainerView,
  type DockerStatsView
} from './protocol'
import { formatBytes, formatPorts, formatTimestamp, stateLabel } from './viewUtils'

type DetailTab = 'logs' | 'inspect' | 'stats'

const TABS: Array<{ id: DetailTab; label: string }> = [
  { id: 'logs', label: 'Logs' },
  { id: 'stats', label: 'Stats' },
  { id: 'inspect', label: 'Inspect' }
]

interface Props {
  tabId: string
  pluginId: string
  container: DockerContainerView
  onClose: () => void
  onAction: (container: DockerContainerView, action: DockerContainerAction) => void
  onShell: () => void
  onUpdate: () => void
  onOpenCreateFromImage: () => void
}

export default function ContainerDetail({
  tabId,
  pluginId,
  container,
  onClose,
  onAction,
  onShell,
  onUpdate,
  onOpenCreateFromImage
}: Props): ReactElement {
  const [tab, setTab] = useState<DetailTab>('logs')
  const [detail, setDetail] = useState<DockerContainerDetailView | null>(null)
  const [logs, setLogs] = useState('')
  const [logsRunning, setLogsRunning] = useState(true)
  const [samples, setSamples] = useState<DockerStatsView[]>([])
  const logRef = useRef<HTMLPreElement | null>(null)

  const send = useCallback(
    (payload: Parameters<typeof window.wassh.sendPluginMessage>[2]): Promise<unknown> =>
      window.wassh.sendPluginMessage(tabId, pluginId, payload),
    [tabId, pluginId]
  )

  useEffect(() => {
    return window.wassh.onPluginMessage((event) => {
      if (event.tabId !== tabId || event.pluginId !== pluginId) {
        return
      }
      if (!isDockerMainMessage(event.payload)) {
        return
      }
      const message = event.payload
      if (message.type === 'detail' && message.detail.id === container.id) {
        setDetail(message.detail)
        return
      }
      if (message.type === 'logs' && message.id === container.id) {
        setLogs((previous) => {
          const next = previous + message.chunks.map((chunk) => chunk.text).join('')
          return next.length > DOCKER_MAX_LOG_CHARS ? next.slice(next.length - DOCKER_MAX_LOG_CHARS) : next
        })
        return
      }
      if (message.type === 'stats' && message.id === container.id) {
        setSamples((previous) => {
          const next = [...previous, message.stats]
          return next.length > DOCKER_STATS_HISTORY ? next.slice(next.length - DOCKER_STATS_HISTORY) : next
        })
      }
    })
  }, [tabId, pluginId, container.id])

  // Fresh data whenever the drawer opens.
  useEffect(() => {
    setDetail(null)
    setLogs('')
    setSamples([])
    setLogsRunning(true)
    void send({ type: 'inspect', id: container.id })
    void send({ type: 'logsOpen', id: container.id, tail: 200, timestamps: false })
    return () => {
      void send({ type: 'logsClose' })
      void send({ type: 'statsClose' })
    }
  }, [send, container.id])

  const pauseLogs = useCallback((): void => {
    if (logsRunning) {
      void send({ type: 'logsClose' })
      setLogsRunning(false)
    } else {
      void send({ type: 'logsOpen', id: container.id, tail: 0, timestamps: false })
      setLogsRunning(true)
    }
  }, [container.id, logsRunning, send])

  const toggleStats = useCallback(
    (next: DetailTab): void => {
      if (next === 'stats') {
        void send({ type: 'statsOpen', id: container.id })
      } else if (tab === 'stats') {
        void send({ type: 'statsClose' })
      }
      setTab(next)
    },
    [container.id, send, tab]
  )

  // Keep the log view pinned to the bottom as output streams in.
  useEffect(() => {
    const node = logRef.current
    if (node && tab === 'logs') {
      node.scrollTop = node.scrollHeight
    }
  }, [logs, tab])

  const latest = samples[samples.length - 1]
  const cpuHistory = useMemo(() => samples.map((sample) => sample.cpuPercent), [samples])
  const memoryHistory = useMemo(() => samples.map((sample) => sample.memoryPercent), [samples])

  return (
    <div className="docker-overlay" role="dialog" aria-modal="true" aria-label={`${container.name} details`}>
      <div className="plugin-ui-toolbar docker-overlay-head">
        <span className={`docker-state docker-state-${container.state}`}>{stateLabel(container.state)}</span>
        <strong className="docker-overlay-title">{container.name || container.shortId}</strong>
        <span className="plugin-ui-muted">{container.image}</span>
        <div className="plugin-ui-spacer" />
        <PluginButton compact onClick={onShell}>
          Shell
        </PluginButton>
        <PluginButton
          compact
          title="Pull the newest image and recreate this container with the same settings"
          onClick={onUpdate}
        >
          Update
        </PluginButton>
        {container.state === 'running' ? (
          <>
            <PluginButton compact onClick={() => onAction(container, 'restart')}>
              Restart
            </PluginButton>
            <PluginButton compact onClick={() => onAction(container, 'pause')}>
              Pause
            </PluginButton>
            <PluginButton compact onClick={() => onAction(container, 'stop')}>
              Stop
            </PluginButton>
          </>
        ) : (
          <PluginButton compact onClick={() => onAction(container, 'start')}>
            Start
          </PluginButton>
        )}
        <PluginButton variant="danger" compact onClick={() => onAction(container, 'remove')}>
          Remove
        </PluginButton>
        <PluginButton compact onClick={onClose} title="Close details">
          Close
        </PluginButton>
      </div>

      <div className="docker-subtabs" role="tablist">
        {TABS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            role="tab"
            aria-selected={tab === entry.id}
            className={`docker-tab${tab === entry.id ? ' active' : ''}`}
            onClick={() => toggleStats(entry.id)}
          >
            {entry.label}
          </button>
        ))}
        <div className="plugin-ui-spacer" />
        {tab === 'logs' ? (
          <>
            <PluginButton compact onClick={pauseLogs}>
              {logsRunning ? 'Pause' : 'Resume'}
            </PluginButton>
            <PluginButton compact onClick={() => setLogs('')}>
              Clear
            </PluginButton>
          </>
        ) : null}
      </div>

      <div className="docker-overlay-body">
        {tab === 'logs' ? (
          logs ? (
            <pre ref={logRef} className="docker-logs">
              {logs}
            </pre>
          ) : (
            <div className="plugin-ui-empty">No log output</div>
          )
        ) : null}

        {tab === 'stats' ? (
          <div className="docker-stats">
            <div className="docker-stat-cards">
              <StatCard
                label="CPU"
                value={latest ? `${latest.cpuPercent.toFixed(1)}%` : '—'}
                history={cpuHistory}
                max={100}
              />
              <StatCard
                label="Memory"
                value={latest ? `${formatBytes(latest.memoryUsed)} / ${formatBytes(latest.memoryLimit)}` : '—'}
                hint={latest ? `${latest.memoryPercent.toFixed(1)}%` : undefined}
                history={memoryHistory}
                max={100}
              />
              <StatCard label="PIDs" value={latest ? String(latest.pids) : '—'} />
              <StatCard
                label="Network"
                value={latest ? `↓ ${formatBytes(latest.netRx)} ↑ ${formatBytes(latest.netTx)}` : '—'}
              />
              <StatCard
                label="Block I/O"
                value={latest ? `R ${formatBytes(latest.blockRead)} W ${formatBytes(latest.blockWrite)}` : '—'}
              />
            </div>
            {!latest ? <div className="plugin-ui-hint">Waiting for the first sample…</div> : null}
          </div>
        ) : null}

        {tab === 'inspect' ? (
          detail ? (
            <div className="docker-inspect">
              <dl className="docker-kv">
                <Row label="Id" value={detail.id} mono />
                <Row label="Image" value={detail.image} mono />
                <Row label="Created" value={formatTimestamp(detail.created)} />
                <Row label="Started" value={formatTimestamp(detail.startedAt)} />
                <Row label="Finished" value={formatTimestamp(detail.finishedAt)} />
                <Row label="Exit code" value={detail.exitCode === null ? '—' : String(detail.exitCode)} />
                <Row label="PID" value={detail.pid === null ? '—' : String(detail.pid)} />
                <Row label="Restarts" value={String(detail.restartCount)} />
                <Row label="Platform" value={detail.platform || '—'} />
                <Row label="Command" value={detail.command || '—'} mono />
                <Row label="Entrypoint" value={detail.entrypoint || '—'} mono />
                <Row label="Working dir" value={detail.workingDir || '—'} mono />
                <Row label="User" value={detail.user || '—'} mono />
                <Row label="TTY" value={detail.tty ? 'yes' : 'no'} />
                <Row label="Restart policy" value={detail.restartPolicy || '—'} />
                <Row label="Network mode" value={detail.networkMode || '—'} />
                <Row label="Privileged" value={detail.privileged ? 'yes' : 'no'} />
                <Row label="Auto remove" value={detail.autoRemove ? 'yes' : 'no'} />
                <Row label="Ports" value={formatPorts(container.ports)} />
                <Row label="Port bindings" value={detail.portBindings.join(', ') || '—'} />
                <Row
                  label="Networks"
                  value={detail.networks.map((network) => `${network.name} (${network.ipAddress || '—'})`).join(', ') || '—'}
                />
                <Row
                  label="Mounts"
                  value={
                    detail.mounts.map((mount) => `${mount.source} → ${mount.destination}`).join(', ') || '—'
                  }
                />
              </dl>

              <details className="docker-raw">
                <summary>Labels &amp; environment</summary>
                <div className="docker-env-grid">
                  <div>
                    <h3 className="plugin-ui-heading">Environment</h3>
                    <pre className="docker-kv-pre">{detail.env.join('\n') || '—'}</pre>
                  </div>
                  <div>
                    <h3 className="plugin-ui-heading">Labels</h3>
                    <pre className="docker-kv-pre">
                      {Object.entries(container.labels)
                        .map(([key, value]) => `${key}=${value}`)
                        .join('\n') || '—'}
                    </pre>
                  </div>
                </div>
              </details>

              <details className="docker-raw">
                <summary>Raw JSON</summary>
                <pre className="docker-json">{detail.raw}</pre>
              </details>

              <div className="plugin-ui-actions">
                <PluginButton compact onClick={onOpenCreateFromImage}>
                  Run another from this image
                </PluginButton>
              </div>
            </div>
          ) : (
            <div className="plugin-ui-empty">Loading…</div>
          )
        ) : null}
      </div>
    </div>
  )
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }): ReactElement {
  return (
    <div className="docker-kv-row">
      <dt className="plugin-ui-muted">{label}</dt>
      <dd className={mono ? 'docker-mono' : undefined}>{value}</dd>
    </div>
  )
}

function StatCard({
  label,
  value,
  hint,
  history,
  max
}: {
  label: string
  value: string
  hint?: string
  history?: number[]
  max?: number
}): ReactElement {
  return (
    <div className="plugin-ui-card docker-stat-card">
      <span className="plugin-ui-muted docker-stat-label">{label}</span>
      <span className="docker-stat-value">{value}</span>
      {hint ? <span className="plugin-ui-muted">{hint}</span> : null}
      {history && history.length > 1 ? <Sparkline values={history} max={max} /> : null}
    </div>
  )
}

/** Minimal SVG sparkline; the panel layer owns no charting dependency. */
function Sparkline({ values, max }: { values: number[]; max?: number }): ReactElement {
  const width = 120
  const height = 28
  const peak = Math.max(max ?? 0, ...values, 1)
  const step = values.length > 1 ? width / (values.length - 1) : width
  const points = values
    .map((value, index) => {
      const x = index * step
      const y = height - Math.max(0, Math.min(1, value / peak)) * height
      return `${x.toFixed(1)},${y.toFixed(1)}`
    })
    .join(' ')
  return (
    <svg
      className="docker-spark"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      role="img"
      aria-label="History"
    >
      <polyline points={points} fill="none" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
    </svg>
  )
}
