import { useEffect, useMemo, useRef, useState, type ReactElement } from 'react'
import type { PluginViewProps } from '@plugin-api/renderer'
import { PluginButton } from '@plugin-api/renderer'
import {
  DAEMON_MONITOR_DEFAULT_RANGE,
  DAEMON_MONITOR_RANGES,
  type DaemonMonitorRange
} from './defaults'
import { PLUGIN_ID_DAEMON_MONITOR } from './id'
import {
  isDaemonMonitorActionResult,
  isDaemonMonitorMainMessage,
  type DaemonMonitorSample,
  type DaemonMonitorServiceStatus,
  type DaemonMonitorStateEvent
} from './protocol'
import './styles.css'

const CHART_WIDTH = 720
const CHART_HEIGHT = 180
const CHART_PADDING_X = 42
const CHART_PADDING_Y = 18
const CHART_MAX_POINTS = 720
const PERCENT_MAX = 100
const BYTES_PER_KIB = 1024
const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const

interface MetricDefinition {
  key: 'cpu' | 'memory' | 'disk'
  label: string
  color: string
  value: (sample: DaemonMonitorSample) => number
}

const METRICS: MetricDefinition[] = [
  {
    key: 'cpu',
    label: 'CPU',
    color: '#58a6ff',
    value: (sample) => sample.cpuPercent
  },
  {
    key: 'memory',
    label: 'RAM',
    color: '#3fb950',
    value: (sample) => ratioPercent(sample.memoryUsedBytes, sample.memoryTotalBytes)
  },
  {
    key: 'disk',
    label: 'Disk',
    color: '#d29922',
    value: (sample) => ratioPercent(sample.diskUsedBytes, sample.diskTotalBytes)
  }
]

const INITIAL_STATUS: DaemonMonitorServiceStatus = {
  state: 'probing',
  streamConnected: false
}

function ratioPercent(used: number, total: number): number {
  return total > 0 ? Math.max(0, Math.min(PERCENT_MAX, (used / total) * PERCENT_MAX)) : 0
}

function formatBytes(bytes: number): string {
  let value = Math.max(0, bytes)
  let unit = 0
  while (value >= BYTES_PER_KIB && unit < BYTE_UNITS.length - 1) {
    value /= BYTES_PER_KIB
    unit += 1
  }
  return `${value.toFixed(value >= 10 || unit === 0 ? 0 : 1)} ${BYTE_UNITS[unit]}`
}

function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleString()
}

function downsample(samples: DaemonMonitorSample[]): DaemonMonitorSample[] {
  if (samples.length <= CHART_MAX_POINTS) {
    return samples
  }
  const stride = Math.ceil(samples.length / CHART_MAX_POINTS)
  return samples.filter((_, index) => index % stride === 0 || index === samples.length - 1)
}

function linePoints(
  samples: DaemonMonitorSample[],
  metric: MetricDefinition,
  start: number,
  end: number
): string {
  const width = CHART_WIDTH - CHART_PADDING_X * 2
  const height = CHART_HEIGHT - CHART_PADDING_Y * 2
  const duration = Math.max(1, end - start)
  return samples
    .map((sample) => {
      const x = CHART_PADDING_X + ((sample.timestamp - start) / duration) * width
      const y =
        CHART_PADDING_Y + height - (Math.max(0, Math.min(PERCENT_MAX, metric.value(sample))) / PERCENT_MAX) * height
      return `${x.toFixed(1)},${y.toFixed(1)}`
    })
    .join(' ')
}

interface NetworkSegment {
  key: string
  name: string
  state: DaemonMonitorStateEvent['state'] | 'unknown'
  startX: number
  width: number
}

const NETWORK_ROW_HEIGHT = 26
const NETWORK_ROW_GAP = 6
const NETWORK_LABEL_WIDTH = CHART_PADDING_X + 60

function networkSegments(
  events: DaemonMonitorStateEvent[],
  start: number,
  end: number
): { targets: string[]; segments: NetworkSegment[] } {
  const width = CHART_WIDTH - NETWORK_LABEL_WIDTH - CHART_PADDING_X
  const duration = Math.max(1, end - start)
  const toX = (timestamp: number): number =>
    NETWORK_LABEL_WIDTH + (((Math.max(start, Math.min(end, timestamp)) - start) / duration) * width)

  const byTarget = new Map<string, DaemonMonitorStateEvent[]>()
  for (const event of events) {
    const key = `${event.kind}:${event.name}`
    const list = byTarget.get(key) ?? []
    list.push(event)
    byTarget.set(key, list)
  }

  const targets = Array.from(byTarget.keys()).sort()
  const segments: NetworkSegment[] = []
  for (const key of targets) {
    const sorted = [...(byTarget.get(key) ?? [])].sort((a, b) => a.timestamp - b.timestamp)
    const name = sorted[sorted.length - 1]?.name ?? key.split(':')[1]
    const dataStart = sorted[0]?.timestamp ?? end
    let cursor = start
    if (dataStart > start) {
      segments.push({
        key,
        name,
        state: 'unknown',
        startX: toX(start),
        width: Math.max(0, toX(Math.min(dataStart, end)) - toX(start))
      })
      cursor = dataStart
    }
    let state: DaemonMonitorStateEvent['state'] = sorted[0]?.state ?? 'up'
    for (const event of sorted) {
      if (event.timestamp > cursor) {
        segments.push({
          key,
          name,
          state,
          startX: toX(cursor),
          width: Math.max(0, toX(event.timestamp) - toX(cursor))
        })
      }
      cursor = event.timestamp
      state = event.state
    }
    segments.push({
      key,
      name,
      state,
      startX: toX(cursor),
      width: Math.max(0, toX(end) - toX(cursor))
    })
  }
  return { targets, segments }
}

function NetworkChart({
  events,
  range
}: {
  events: DaemonMonitorStateEvent[]
  range: DaemonMonitorRange
}): ReactElement {
  const end = Date.now()
  const start = end - DAEMON_MONITOR_RANGES[range] * 1000
  const { targets, segments } = useMemo(() => networkSegments(events, start, end), [events, start, end])
  const chartHeight = Math.max(1, targets.length) * (NETWORK_ROW_HEIGHT + NETWORK_ROW_GAP) + CHART_PADDING_Y
  return (
    <section className="daemon-monitor-chart-panel">
      <h3>Network state history</h3>
      {targets.length === 0 ? (
        <p>No interface or ping data in this range.</p>
      ) : (
        <>
          <svg
            className="daemon-monitor-network-chart"
            viewBox={`0 0 ${CHART_WIDTH} ${chartHeight}`}
            role="img"
            aria-label={`Interface and ping reachability over ${range}`}
          >
            {targets.map((key, index) => {
              const y = index * (NETWORK_ROW_HEIGHT + NETWORK_ROW_GAP)
              const name = segments.find((segment) => segment.key === key)?.name ?? key
              return (
                <text key={key} x={NETWORK_LABEL_WIDTH - 8} y={y + NETWORK_ROW_HEIGHT / 2 + 4}>
                  {name}
                </text>
              )
            })}
            {segments.map((segment, index) => {
              const rowIndex = targets.indexOf(segment.key)
              const y = rowIndex * (NETWORK_ROW_HEIGHT + NETWORK_ROW_GAP)
              return (
                <rect
                  key={`${segment.key}:${index}`}
                  className={
                    segment.state === 'up'
                      ? 'is-up-fill'
                      : segment.state === 'down'
                        ? 'is-down-fill'
                        : 'is-unknown-fill'
                  }
                  x={segment.startX}
                  y={y}
                  width={segment.width}
                  height={NETWORK_ROW_HEIGHT}
                />
              )
            })}
          </svg>
          <div className="daemon-monitor-chart-axis">
            <span>{new Date(start).toLocaleTimeString()}</span>
            <span>{new Date(end).toLocaleTimeString()}</span>
          </div>
        </>
      )}
    </section>
  )
}

function MetricsChart({
  samples,
  range
}: {
  samples: DaemonMonitorSample[]
  range: DaemonMonitorRange
}): ReactElement {
  const end = samples.at(-1)?.timestamp ?? Date.now()
  const start = end - DAEMON_MONITOR_RANGES[range] * 1000
  const visible = downsample(samples.filter((sample) => sample.timestamp >= start))
  return (
    <section className="daemon-monitor-chart-panel">
      <div className="daemon-monitor-legend">
        {METRICS.map((metric) => (
          <span key={metric.key}>
            <i style={{ backgroundColor: metric.color }} />
            {metric.label}
          </span>
        ))}
      </div>
      <svg
        className="daemon-monitor-chart"
        viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
        role="img"
        aria-label={`CPU, RAM, and disk usage over ${range}`}
      >
        {[0, 25, 50, 75, 100].map((percent) => {
          const y =
            CHART_PADDING_Y +
            (1 - percent / PERCENT_MAX) * (CHART_HEIGHT - CHART_PADDING_Y * 2)
          return (
            <g key={percent}>
              <line
                x1={CHART_PADDING_X}
                x2={CHART_WIDTH - CHART_PADDING_X}
                y1={y}
                y2={y}
              />
              <text x={CHART_PADDING_X - 8} y={y + 4}>
                {percent}%
              </text>
            </g>
          )
        })}
        {METRICS.map((metric) => (
          <polyline
            key={metric.key}
            points={linePoints(visible, metric, start, end)}
            stroke={metric.color}
          />
        ))}
      </svg>
      <div className="daemon-monitor-chart-axis">
        <span>{new Date(start).toLocaleTimeString()}</span>
        <span>{new Date(end).toLocaleTimeString()}</span>
      </div>
    </section>
  )
}

export default function DaemonMonitorView({ tabId }: PluginViewProps): ReactElement {
  const [status, setStatus] = useState<DaemonMonitorServiceStatus>(INITIAL_STATUS)
  const [samples, setSamples] = useState<DaemonMonitorSample[]>([])
  const [events, setEvents] = useState<DaemonMonitorStateEvent[]>([])
  const [range, setRange] = useState<DaemonMonitorRange>(DAEMON_MONITOR_DEFAULT_RANGE)
  const rangeRef = useRef<DaemonMonitorRange>(DAEMON_MONITOR_DEFAULT_RANGE)
  const [sudoAction, setSudoAction] = useState<'install' | 'uninstall' | null>(null)
  const [password, setPassword] = useState('')
  const [actionError, setActionError] = useState('')
  const operationRunning = status.state === 'installing' || status.state === 'uninstalling'

  useEffect(() => {
    const off = window.wassh.onPluginMessage((event) => {
      if (event.tabId !== tabId || event.pluginId !== PLUGIN_ID_DAEMON_MONITOR) {
        return
      }
      if (!isDaemonMonitorMainMessage(event.payload)) {
        return
      }
      const payload = event.payload
      if (payload.type === 'status') {
        setStatus(payload.status)
        return
      }
      const cutoff = Date.now() - DAEMON_MONITOR_RANGES[rangeRef.current] * 1000
      setSamples((current) => {
        const next = payload.reset ? payload.samples : [...current, ...payload.samples]
        return next.filter((sample) => sample.timestamp >= cutoff)
      })
      setEvents((current) => {
        const next = payload.reset ? payload.events : [...current, ...payload.events]
        return next.filter((item) => !item.transition || item.timestamp >= cutoff)
      })
    })
    void window.wassh.sendPluginMessage(tabId, PLUGIN_ID_DAEMON_MONITOR, { type: 'probe' })
    return off
  }, [tabId])

  const current = samples.at(-1)
  const currentStates = useMemo(() => {
    const states = new Map<string, DaemonMonitorStateEvent>()
    for (const event of events.filter((candidate) => !candidate.transition)) {
      const key = `${event.kind}:${event.name}`
      states.set(key, event)
    }
    for (const event of events.filter((candidate) => candidate.transition)) {
      const key = `${event.kind}:${event.name}`
      const current = states.get(key)
      if (current && event.timestamp >= current.timestamp) {
        states.set(key, event)
      }
    }
    return Array.from(states.values()).sort((a, b) =>
      `${a.kind}:${a.name}`.localeCompare(`${b.kind}:${b.name}`)
    )
  }, [events])
  const transitions = useMemo(() => events.filter((event) => event.transition), [events])

  const selectRange = (nextRange: DaemonMonitorRange): void => {
    rangeRef.current = nextRange
    setRange(nextRange)
    void window.wassh.sendPluginMessage(tabId, PLUGIN_ID_DAEMON_MONITOR, {
      type: 'setRange',
      range: nextRange
    })
  }

  const runSudoAction = async (): Promise<void> => {
    if (!sudoAction) {
      return
    }
    setActionError('')
    const result = await window.wassh.sendPluginMessage(tabId, PLUGIN_ID_DAEMON_MONITOR, {
      type: sudoAction,
      password
    })
    setPassword('')
    if (!isDaemonMonitorActionResult(result) || !result.ok) {
      setActionError(
        isDaemonMonitorActionResult(result) ? result.error || 'Operation failed' : 'Operation failed'
      )
      return
    }
    setSudoAction(null)
  }

  if (status.state === 'missing' && sudoAction !== 'install') {
    return (
      <div className="daemon-monitor daemon-monitor-empty">
        <PluginButton variant="primary" onClick={() => setSudoAction('install')}>
          Install WaSSH Service
        </PluginButton>
      </div>
    )
  }

  if (status.state !== 'ready') {
    return (
      <div className="daemon-monitor daemon-monitor-empty">
        <h2>Daemon monitor</h2>
        <p>
          {status.message ||
            (status.state === 'probing' ? 'Checking remote service…' : 'The remote service is not installed.')}
        </p>
        {actionError && <div className="daemon-monitor-error">{actionError}</div>}
        {sudoAction ? (
          <form
            className="daemon-monitor-sudo"
            onSubmit={(event) => {
              event.preventDefault()
              void runSudoAction()
            }}
          >
            <label>
              Sudo password
              <input
                autoFocus
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
            </label>
            <small>Kept in memory only for this operation. Leave blank for passwordless sudo.</small>
            <div>
              <PluginButton
                type="submit"
                disabled={operationRunning}
                variant={sudoAction === 'uninstall' ? 'danger' : 'primary'}
              >
                {operationRunning
                  ? 'Working…'
                  : sudoAction === 'install'
                    ? status.state === 'outdated'
                      ? 'Upgrade service'
                      : 'Install service'
                    : 'Uninstall completely'}
              </PluginButton>
              <PluginButton disabled={operationRunning} onClick={() => setSudoAction(null)}>
                Cancel
              </PluginButton>
            </div>
          </form>
        ) : (
          <div className="daemon-monitor-actions">
            <PluginButton
              variant="primary"
              disabled={status.state === 'probing' || operationRunning}
              onClick={() => setSudoAction('install')}
            >
              {status.state === 'outdated' ? 'Upgrade service' : 'Install service'}
            </PluginButton>
            {status.version && (
              <PluginButton variant="danger" onClick={() => setSudoAction('uninstall')}>
                Uninstall service
              </PluginButton>
            )}
          </div>
        )}
      </div>
    )
  }

  return (
    <div className="daemon-monitor">
      <header className="daemon-monitor-header">
        <div>
          <h2>Daemon monitor</h2>
          <span className={status.streamConnected ? 'is-up' : 'is-down'}>
            {status.streamConnected ? 'Live' : 'Stream disconnected'}
          </span>
          {status.version && <span className="daemon-monitor-version">v{status.version}</span>}
        </div>
        <select value={range} onChange={(event) => selectRange(event.target.value as DaemonMonitorRange)}>
          {Object.keys(DAEMON_MONITOR_RANGES).map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </select>
      </header>

      {status.message && <div className="daemon-monitor-error">{status.message}</div>}
      <section className="daemon-monitor-cards">
        {METRICS.map((metric) => (
          <article key={metric.key}>
            <span>{metric.label}</span>
            <strong>{current ? `${metric.value(current).toFixed(1)}%` : '—'}</strong>
            {current && metric.key === 'memory' && (
              <small>
                {formatBytes(current.memoryUsedBytes)} / {formatBytes(current.memoryTotalBytes)}
              </small>
            )}
            {current && metric.key === 'disk' && (
              <small>
                {formatBytes(current.diskUsedBytes)} / {formatBytes(current.diskTotalBytes)}
              </small>
            )}
          </article>
        ))}
      </section>

      <MetricsChart samples={samples} range={range} />

      <section className="daemon-monitor-grid daemon-monitor-grid-stacked">
        <article>
          <h3>Current network state</h3>
          <div className="daemon-monitor-states">
            {currentStates.length === 0 && <p>No state records in this range.</p>}
            {currentStates.map((event) => (
              <div key={`${event.kind}:${event.name}`}>
                <span className={event.state === 'up' ? 'is-up' : 'is-down'}>{event.state}</span>
                <strong>{event.name}</strong>
                <small>{event.kind}</small>
              </div>
            ))}
          </div>
        </article>
      </section>

      <NetworkChart events={events} range={range} />

      <section className="daemon-monitor-grid daemon-monitor-grid-stacked">
        <article>
          <h3>Outages and recoveries</h3>
          <div className="daemon-monitor-events">
            {[...transitions].reverse().map((event, index) => (
              <div key={`${event.timestamp}:${event.kind}:${event.name}:${index}`}>
                <time>{formatTime(event.timestamp)}</time>
                <span className={event.state === 'up' ? 'is-up' : 'is-down'}>{event.state}</span>
                <strong>{event.name}</strong>
              </div>
            ))}
          </div>
        </article>
      </section>

      <footer className="daemon-monitor-footer">
        {sudoAction === 'uninstall' ? (
          <form
            className="daemon-monitor-sudo"
            onSubmit={(event) => {
              event.preventDefault()
              void runSudoAction()
            }}
          >
            <label>
              Confirm complete removal with sudo password
              <input
                autoFocus
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
            </label>
            <div>
              <PluginButton type="submit" variant="danger">
                Uninstall completely
              </PluginButton>
              <PluginButton onClick={() => setSudoAction(null)}>Cancel</PluginButton>
            </div>
          </form>
        ) : (
          <PluginButton variant="danger" onClick={() => setSudoAction('uninstall')}>
            Uninstall service
          </PluginButton>
        )}
      </footer>
    </div>
  )
}
