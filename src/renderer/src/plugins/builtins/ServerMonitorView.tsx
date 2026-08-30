import { useEffect, useRef, useState, type ReactElement } from 'react'
import type { ServerMonitorNetIface, ServerMonitorProcess, ServerMonitorSnapshot } from '@shared/plugins'
import {
  SERVER_MONITOR_SHOW_GAUGES_DEFAULT,
  SERVER_MONITOR_SHOW_NETWORK_DEFAULT,
  SERVER_MONITOR_SHOW_PROCESSES_DEFAULT,
  SERVER_MONITOR_SHOW_SPARKS_DEFAULT,
  SERVER_MONITOR_SHOW_STATUS_DEFAULT
} from '@shared/plugins'
import type { PluginViewProps } from '../registry'

/** Samples kept for sparkline history */
const HISTORY_POINTS = 60

/** SVG viewBox width for sparklines */
const SPARK_WIDTH = 120

/** SVG viewBox height for sparklines */
const SPARK_HEIGHT = 36

/** Ring gauge viewBox size */
const GAUGE_SIZE = 72

/** Ring stroke width */
const GAUGE_STROKE = 7

/** Bytes in one kibibyte */
const BYTES_PER_KIB = 1024

/** Section toggle definitions (persisted via plugin settings) */
const SECTION_TOGGLES: Array<{
  key: string
  label: string
  fallback: boolean
}> = [
  { key: 'showGauges', label: 'Gauges', fallback: SERVER_MONITOR_SHOW_GAUGES_DEFAULT },
  { key: 'showSparks', label: 'History', fallback: SERVER_MONITOR_SHOW_SPARKS_DEFAULT },
  { key: 'showStatus', label: 'Status', fallback: SERVER_MONITOR_SHOW_STATUS_DEFAULT },
  { key: 'showProcesses', label: 'Procs', fallback: SERVER_MONITOR_SHOW_PROCESSES_DEFAULT },
  { key: 'showNetwork', label: 'Net', fallback: SERVER_MONITOR_SHOW_NETWORK_DEFAULT }
]

function settingBool(
  settings: Record<string, unknown>,
  key: string,
  fallback: boolean
): boolean {
  const v = settings[key]
  return typeof v === 'boolean' ? v : fallback
}

function clampPercent(n: number | null | undefined): number {
  if (n == null || !Number.isFinite(n)) {
    return 0
  }
  return Math.max(0, Math.min(100, n))
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return '0 B'
  }
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unit = 0
  while (value >= BYTES_PER_KIB && unit < units.length - 1) {
    value /= BYTES_PER_KIB
    unit += 1
  }
  const digits = value >= 100 || unit === 0 ? 0 : value >= 10 ? 1 : 2
  return `${value.toFixed(digits)} ${units[unit]}`
}

function formatRate(bytesPerSec: number | null): string {
  if (bytesPerSec == null || !Number.isFinite(bytesPerSec)) {
    return '—'
  }
  return `${formatBytes(bytesPerSec)}/s`
}

function formatUptime(sec: number): string {
  if (!Number.isFinite(sec) || sec <= 0) {
    return '—'
  }
  const s = Math.floor(sec)
  const days = Math.floor(s / 86400)
  const hours = Math.floor((s % 86400) / 3600)
  const mins = Math.floor((s % 3600) / 60)
  if (days > 0) {
    return `${days}d ${hours}h`
  }
  if (hours > 0) {
    return `${hours}h ${mins}m`
  }
  return `${mins}m`
}

function formatLoad(n: number): string {
  return Number.isFinite(n) ? n.toFixed(2) : '—'
}

function formatPercent(n: number): string {
  return Number.isFinite(n) ? n.toFixed(1) : '—'
}

function pushHistory(prev: number[], value: number): number[] {
  const next = prev.length >= HISTORY_POINTS ? prev.slice(prev.length - HISTORY_POINTS + 1) : prev.slice()
  next.push(value)
  return next
}

function sparkPath(values: number[]): string {
  if (values.length === 0) {
    return ''
  }
  const max = 100
  const step = values.length > 1 ? SPARK_WIDTH / (values.length - 1) : SPARK_WIDTH
  return values
    .map((v, i) => {
      const x = i * step
      const y = SPARK_HEIGHT - (clampPercent(v) / max) * (SPARK_HEIGHT - 2) - 1
      return `${i === 0 ? 'M' : 'L'}${x.toFixed(1)} ${y.toFixed(1)}`
    })
    .join(' ')
}

function sparkArea(values: number[]): string {
  const line = sparkPath(values)
  if (!line || values.length === 0) {
    return ''
  }
  const step = values.length > 1 ? SPARK_WIDTH / (values.length - 1) : SPARK_WIDTH
  const lastX = (values.length - 1) * step
  return `${line} L${lastX.toFixed(1)} ${SPARK_HEIGHT} L0 ${SPARK_HEIGHT} Z`
}

function Gauge({
  label,
  percent,
  detail,
  tone
}: {
  label: string
  percent: number | null
  detail: string
  tone: 'cpu' | 'mem' | 'disk'
}): ReactElement {
  const value = percent == null ? null : clampPercent(percent)
  const radius = (GAUGE_SIZE - GAUGE_STROKE) / 2
  const circumference = 2 * Math.PI * radius
  const offset =
    value == null ? circumference : circumference - (value / 100) * circumference

  return (
    <div className={`monitor-gauge monitor-gauge-${tone}`}>
      <svg
        className="monitor-gauge-svg"
        viewBox={`0 0 ${GAUGE_SIZE} ${GAUGE_SIZE}`}
        aria-hidden="true"
      >
        <circle
          className="monitor-gauge-track"
          cx={GAUGE_SIZE / 2}
          cy={GAUGE_SIZE / 2}
          r={radius}
          strokeWidth={GAUGE_STROKE}
        />
        <circle
          className="monitor-gauge-value"
          cx={GAUGE_SIZE / 2}
          cy={GAUGE_SIZE / 2}
          r={radius}
          strokeWidth={GAUGE_STROKE}
          strokeDasharray={circumference}
          strokeDashoffset={offset}
          transform={`rotate(-90 ${GAUGE_SIZE / 2} ${GAUGE_SIZE / 2})`}
        />
        <text
          className="monitor-gauge-text"
          x="50%"
          y="50%"
          dominantBaseline="central"
          textAnchor="middle"
        >
          {value == null ? '—' : `${Math.round(value)}%`}
        </text>
      </svg>
      <div className="monitor-gauge-meta">
        <div className="monitor-gauge-label">{label}</div>
        <div className="monitor-gauge-detail">{detail}</div>
      </div>
    </div>
  )
}

function SparkCard({
  title,
  values,
  tone,
  current
}: {
  title: string
  values: number[]
  tone: 'cpu' | 'mem' | 'disk'
  current: string
}): ReactElement {
  const line = sparkPath(values)
  const area = sparkArea(values)
  return (
    <div className={`monitor-spark monitor-spark-${tone}`}>
      <div className="monitor-spark-head">
        <span>{title}</span>
        <strong>{current}</strong>
      </div>
      <svg
        className="monitor-spark-svg"
        viewBox={`0 0 ${SPARK_WIDTH} ${SPARK_HEIGHT}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        {area ? <path className="monitor-spark-area" d={area} /> : null}
        {line ? <path className="monitor-spark-line" d={line} fill="none" /> : null}
      </svg>
    </div>
  )
}

function ProcessTable({ rows }: { rows: ServerMonitorProcess[] }): ReactElement {
  return (
    <div className="monitor-section monitor-processes">
      <div className="monitor-section-title">Processes</div>
      {rows.length === 0 ? (
        <div className="monitor-section-empty">No process data</div>
      ) : (
        <table className="monitor-table">
          <thead>
            <tr>
              <th scope="col">PID</th>
              <th scope="col">User</th>
              <th scope="col">CPU</th>
              <th scope="col">Mem</th>
              <th scope="col">Command</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => (
              <tr key={`${p.pid}-${p.command}`}>
                <td className="monitor-num">{p.pid}</td>
                <td className="monitor-user" title={p.user}>
                  {p.user}
                </td>
                <td className="monitor-num">{formatPercent(p.cpuPercent)}%</td>
                <td className="monitor-num">{formatPercent(p.memPercent)}%</td>
                <td className="monitor-cmd" title={p.command}>
                  {p.command}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}

function NetworkTable({ ifaces }: { ifaces: ServerMonitorNetIface[] }): ReactElement {
  return (
    <div className="monitor-section monitor-network">
      <div className="monitor-section-title">Network</div>
      {ifaces.length === 0 ? (
        <div className="monitor-section-empty">No interface data</div>
      ) : (
        <table className="monitor-table">
          <thead>
            <tr>
              <th scope="col">Iface</th>
              <th scope="col">RX</th>
              <th scope="col">TX</th>
              <th scope="col">Total RX</th>
              <th scope="col">Total TX</th>
            </tr>
          </thead>
          <tbody>
            {ifaces.map((iface) => (
              <tr key={iface.name}>
                <td className="monitor-iface" title={iface.name}>
                  {iface.name}
                </td>
                <td className="monitor-num">{formatRate(iface.rxRate)}</td>
                <td className="monitor-num">{formatRate(iface.txRate)}</td>
                <td className="monitor-num">{formatBytes(iface.rxBytes)}</td>
                <td className="monitor-num">{formatBytes(iface.txBytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}

export default function ServerMonitorView({
  tabId,
  pluginId,
  settings,
  onSettingsPatch
}: PluginViewProps) {
  const [snapshot, setSnapshot] = useState<ServerMonitorSnapshot | null>(null)
  const [cpuHistory, setCpuHistory] = useState<number[]>([])
  const [memHistory, setMemHistory] = useState<number[]>([])
  const [diskHistory, setDiskHistory] = useState<number[]>([])
  const lastAt = useRef(0)

  const showGauges = settingBool(settings, 'showGauges', SERVER_MONITOR_SHOW_GAUGES_DEFAULT)
  const showSparks = settingBool(settings, 'showSparks', SERVER_MONITOR_SHOW_SPARKS_DEFAULT)
  const showStatus = settingBool(settings, 'showStatus', SERVER_MONITOR_SHOW_STATUS_DEFAULT)
  const showProcesses = settingBool(
    settings,
    'showProcesses',
    SERVER_MONITOR_SHOW_PROCESSES_DEFAULT
  )
  const showNetwork = settingBool(settings, 'showNetwork', SERVER_MONITOR_SHOW_NETWORK_DEFAULT)

  useEffect(() => {
    return window.wassh.onPluginMessage((ev) => {
      if (ev.tabId !== tabId || ev.pluginId !== pluginId) {
        return
      }
      const payload = ev.payload as { type?: string; snapshot?: ServerMonitorSnapshot }
      if (payload?.type !== 'stats' || !payload.snapshot) {
        return
      }
      const next = payload.snapshot
      setSnapshot(next)
      if (next.error || next.updatedAt === lastAt.current) {
        return
      }
      lastAt.current = next.updatedAt
      if (next.cpuPercent != null) {
        setCpuHistory((h) => pushHistory(h, next.cpuPercent as number))
      }
      if (next.memTotalBytes > 0) {
        setMemHistory((h) =>
          pushHistory(h, (next.memUsedBytes / next.memTotalBytes) * 100)
        )
      }
      if (next.diskTotalBytes > 0) {
        setDiskHistory((h) =>
          pushHistory(h, (next.diskUsedBytes / next.diskTotalBytes) * 100)
        )
      }
    })
  }, [tabId, pluginId])

  const memPct =
    snapshot && snapshot.memTotalBytes > 0
      ? (snapshot.memUsedBytes / snapshot.memTotalBytes) * 100
      : null
  const diskPct =
    snapshot && snapshot.diskTotalBytes > 0
      ? (snapshot.diskUsedBytes / snapshot.diskTotalBytes) * 100
      : null
  const diskFree =
    snapshot && snapshot.diskTotalBytes > 0
      ? Math.max(0, snapshot.diskTotalBytes - snapshot.diskUsedBytes)
      : 0

  return (
    <div className="plugin-panel plugin-server-monitor">
      <div className="monitor-header">
        <div className="monitor-header-top">
          <div className="monitor-title">Server</div>
          <div className="monitor-host" title={snapshot?.hostname || ''}>
            {snapshot?.hostname || 'Connecting…'}
          </div>
        </div>
        <div className="monitor-section-toggles" role="group" aria-label="Visible sections">
          {SECTION_TOGGLES.map((tog) => {
            const on = settingBool(settings, tog.key, tog.fallback)
            return (
              <button
                key={tog.key}
                type="button"
                className={`monitor-section-toggle${on ? ' active' : ''}`}
                aria-pressed={on}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => onSettingsPatch({ [tog.key]: !on })}
              >
                {tog.label}
              </button>
            )
          })}
        </div>
      </div>

      {snapshot?.error ? <div className="plugin-monitor-error">{snapshot.error}</div> : null}

      <div className="monitor-body">
        {showGauges || showSparks ? (
          <div className="monitor-overview">
            {showGauges ? (
              <div className="monitor-gauges">
                <Gauge
                  label="CPU"
                  percent={snapshot?.cpuPercent ?? null}
                  detail={
                    snapshot
                      ? `Load ${formatLoad(snapshot.load1)}${
                          snapshot.cpuCount > 0 ? ` · ${snapshot.cpuCount} CPU` : ''
                        }`
                      : 'Sampling…'
                  }
                  tone="cpu"
                />
                <Gauge
                  label="Memory"
                  percent={memPct}
                  detail={
                    snapshot
                      ? `${formatBytes(snapshot.memUsedBytes)} / ${formatBytes(snapshot.memTotalBytes)}`
                      : '—'
                  }
                  tone="mem"
                />
                <Gauge
                  label="Disk /"
                  percent={diskPct}
                  detail={
                    snapshot
                      ? `${formatBytes(snapshot.diskUsedBytes)} / ${formatBytes(snapshot.diskTotalBytes)}`
                      : '—'
                  }
                  tone="disk"
                />
              </div>
            ) : null}

            {showSparks ? (
              <div className="monitor-sparks">
                <SparkCard
                  title="CPU"
                  values={cpuHistory}
                  tone="cpu"
                  current={
                    snapshot?.cpuPercent == null
                      ? '—'
                      : `${Math.round(snapshot.cpuPercent)}%`
                  }
                />
                <SparkCard
                  title="Memory"
                  values={memHistory}
                  tone="mem"
                  current={memPct == null ? '—' : `${Math.round(memPct)}%`}
                />
                <SparkCard
                  title="Disk"
                  values={diskHistory}
                  tone="disk"
                  current={diskPct == null ? '—' : `${Math.round(diskPct)}%`}
                />
              </div>
            ) : null}
          </div>
        ) : null}

        {showStatus ? (
          <div className="monitor-facts">
            <div className="monitor-fact">
              <span>Uptime</span>
              <strong>{formatUptime(snapshot?.uptimeSec ?? 0)}</strong>
            </div>
            <div className="monitor-fact">
              <span>Load</span>
              <strong>
                {snapshot
                  ? `${formatLoad(snapshot.load1)} · ${formatLoad(snapshot.load5)} · ${formatLoad(snapshot.load15)}`
                  : '—'}
              </strong>
            </div>
            <div className="monitor-fact">
              <span>Swap</span>
              <strong>
                {snapshot && snapshot.swapTotalBytes > 0
                  ? `${formatBytes(snapshot.swapUsedBytes)} / ${formatBytes(snapshot.swapTotalBytes)}`
                  : '—'}
              </strong>
            </div>
            <div className="monitor-fact">
              <span>Available</span>
              <strong>
                {snapshot ? formatBytes(snapshot.memAvailableBytes) : '—'}
              </strong>
            </div>
            <div className="monitor-fact">
              <span>Disk free</span>
              <strong>{snapshot && snapshot.diskTotalBytes > 0 ? formatBytes(diskFree) : '—'}</strong>
            </div>
            <div className="monitor-fact">
              <span>Tasks</span>
              <strong>
                {snapshot && snapshot.procsTotal > 0
                  ? `${snapshot.procsRunning} / ${snapshot.procsTotal}`
                  : '—'}
              </strong>
            </div>
            <div className="monitor-fact">
              <span>CPUs</span>
              <strong>{snapshot && snapshot.cpuCount > 0 ? String(snapshot.cpuCount) : '—'}</strong>
            </div>
            <div className="monitor-fact monitor-fact-wide">
              <span>Kernel</span>
              <strong title={snapshot?.kernel || ''}>{snapshot?.kernel || '—'}</strong>
            </div>
          </div>
        ) : null}

        {showProcesses || showNetwork ? (
          <div className="monitor-tables">
            {showProcesses ? <ProcessTable rows={snapshot?.processes ?? []} /> : null}
            {showNetwork ? <NetworkTable ifaces={snapshot?.network ?? []} /> : null}
          </div>
        ) : null}
      </div>

      {snapshot?.updatedAt ? (
        <div className="plugin-monitor-meta">
          Updated {new Date(snapshot.updatedAt).toLocaleTimeString()}
        </div>
      ) : null}
    </div>
  )
}
