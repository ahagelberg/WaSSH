import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactElement } from 'react'
import type { PluginViewProps } from '@plugin-api/renderer'
import { PluginButton } from '@plugin-api/renderer'
import {
  LADDER_TIER_ARCHIVE,
  LADDER_TIER_FAST,
  LADDER_TIER_MEDIUM,
  LADDER_TIER_SLOW
} from '@shared/monitorLadder'
import {
  SERIES_CPU,
  SERIES_DISK_READ,
  SERIES_DISK_TOTAL,
  SERIES_DISK_USED,
  SERIES_DISK_WRITE,
  SERIES_MEM_TOTAL,
  SERIES_MEM_USED,
  SERIES_PERCENT_MAX,
  type MonitorSeries
} from '@shared/monitorSeries'
import {
  BITS_PER_BYTE,
  BYTES_PER_KIB,
  SERVER_MONITOR_MEGABIT_BITS,
  SERVER_MONITOR_SHOW_GAUGES_DEFAULT,
  SERVER_MONITOR_SHOW_NETWORK_DEFAULT,
  SERVER_MONITOR_SHOW_PROCESSES_DEFAULT,
  SERVER_MONITOR_SHOW_SPARKS_DEFAULT,
  SERVER_MONITOR_SHOW_STATUS_DEFAULT
} from './defaults'
import type {
  ServerMonitorNetIface,
  ServerMonitorProcess,
  ServerMonitorProcessSignal,
  ServerMonitorProcessSort,
  ServerMonitorRendererMessage,
  ServerMonitorSnapshot,
  ServerMonitorTemp
} from './protocol'
import {
  isServerMonitorActionResult,
  isServerMonitorStatsEvent,
  SERVER_MONITOR_PROCESS_SORT_DEFAULT,
  SERVER_MONITOR_PROCESS_SORT_DESC_DEFAULT
} from './protocol'
import {
  isMonitorActionResult,
  isMonitorMainMessage,
  MONITOR_SERVICE_INITIAL_STATUS,
  type MonitorServiceStatus
} from './serviceProtocol'
import './styles.css'

/** History ranges the view can request, keyed by the label shown in the picker */
const HISTORY_RANGES = {
  '5m': { seconds: 5 * 60, tier: LADDER_TIER_FAST },
  '1h': { seconds: 60 * 60, tier: LADDER_TIER_MEDIUM },
  '24h': { seconds: 24 * 60 * 60, tier: LADDER_TIER_SLOW },
  '7d': { seconds: 7 * 24 * 60 * 60, tier: LADDER_TIER_ARCHIVE }
} as const

type HistoryRange = keyof typeof HISTORY_RANGES

/** Range selected when the panel opens */
const HISTORY_RANGE_DEFAULT: HistoryRange = '5m'

/** Milliseconds per second (history range math) */
const MS_PER_SEC = 1000

/** SVG viewBox width for sparklines */
const SPARK_WIDTH = 120

/** SVG viewBox height for sparklines */
const SPARK_HEIGHT = 36

/** Vertical inset so the line and the scale gridline never touch the plot edges */
const SPARK_INSET = 1

/** Ring gauge viewBox size */
const GAUGE_SIZE = 72

/** Ring stroke width */
const GAUGE_STROKE = 7

/** Seconds in a day / hour / minute (uptime) */
const SEC_PER_DAY = 86400
const SEC_PER_HOUR = 3600
const SEC_PER_MIN = 60

/** Byte-format unit ladder */
const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const

/** Decimal bytes in one megabyte (net fallback scale rungs) */
const BYTES_PER_MB = 1_000_000

/**
 * Assumed net scale rungs (bytes/sec) used when no interface reports a link
 * speed: start at 10 MB/s and jump 100 → 250 → 1000 MB/s as traffic exceeds
 * each rung.
 */
const NET_ASSUMED_SCALE_RUNGS = [10, 100, 250, 1000].map((mb) => mb * BYTES_PER_MB)

/** Rate-bar fill floor so tiny traffic stays visible */
const RATE_BAR_MIN_PCT = 2

/** Em-dash placeholder for missing samples */
const MISSING = '—'

/** How long process action status stays visible (ms) */
const PROC_STATUS_MS = 4000

/** Edge padding when clamping the process context menu */
const CONTEXT_MENU_EDGE_PAD = 4

/** Max command chars shown in process action status */
const PROC_STATUS_CMD_MAX = 40

type GaugeTone = 'cpu' | 'mem' | 'disk' | 'swap' | 'net'
type SparkTone = GaugeTone

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

/** Process table columns (header click sorts remotely) */
const PROCESS_COLUMNS: Array<{
  key: ServerMonitorProcessSort
  label: string
  numeric: boolean
}> = [
  { key: 'pid', label: 'PID', numeric: true },
  { key: 'user', label: 'User', numeric: false },
  { key: 'state', label: 'S', numeric: false },
  { key: 'nice', label: 'NI', numeric: true },
  { key: 'threads', label: 'THR', numeric: true },
  { key: 'cpu', label: 'CPU', numeric: true },
  { key: 'mem', label: 'Mem', numeric: true },
  { key: 'command', label: 'Command', numeric: false }
]

/** First-click direction when switching to a column */
function defaultDescending(sort: ServerMonitorProcessSort): boolean {
  return sort === 'cpu' || sort === 'mem' || sort === 'threads' || sort === 'nice'
}

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

function ratioPercent(used: number, total: number): number | null {
  if (!(total > 0)) {
    return null
  }
  return (used / total) * 100
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return `0 ${BYTE_UNITS[0]}`
  }
  let value = bytes
  let unit = 0
  while (value >= BYTES_PER_KIB && unit < BYTE_UNITS.length - 1) {
    value /= BYTES_PER_KIB
    unit += 1
  }
  const digits = value >= 100 || unit === 0 ? 0 : value >= 10 ? 1 : 2
  return `${value.toFixed(digits)} ${BYTE_UNITS[unit]}`
}

function formatRate(bytesPerSec: number | null | undefined): string {
  if (bytesPerSec == null || !Number.isFinite(bytesPerSec)) {
    return MISSING
  }
  return `${formatBytes(bytesPerSec)}/s`
}

function formatUptime(sec: number): string {
  if (!Number.isFinite(sec) || sec <= 0) {
    return MISSING
  }
  const s = Math.floor(sec)
  const days = Math.floor(s / SEC_PER_DAY)
  const hours = Math.floor((s % SEC_PER_DAY) / SEC_PER_HOUR)
  const mins = Math.floor((s % SEC_PER_HOUR) / SEC_PER_MIN)
  if (days > 0) {
    return `${days}d ${hours}h`
  }
  if (hours > 0) {
    return `${hours}h ${mins}m`
  }
  return `${mins}m`
}

function formatLoad(n: number): string {
  return Number.isFinite(n) ? n.toFixed(2) : MISSING
}

function formatPercent(n: number): string {
  return Number.isFinite(n) ? n.toFixed(1) : MISSING
}

function formatTemp(celsius: number): string {
  return Number.isFinite(celsius) ? `${celsius.toFixed(0)}°C` : MISSING
}

/** One aggregated history point for a series. */
interface HistoryPoint {
  timestamp: number
  min: number
  max: number
  avg: number
}

/** Buckets received from the main process, keyed by series id. */
type HistoryStore = Record<string, HistoryPoint[]>

function sparkCoords(
  points: HistoryPoint[],
  maxValue: number,
  fromMs: number,
  toMs: number
): Array<{ x: number; y: number }> {
  const max = maxValue > 0 ? maxValue : 1
  const span = Math.max(1, toMs - fromMs)
  return points.map((point) => ({
    x: ((point.timestamp - fromMs) / span) * SPARK_WIDTH,
    y:
      SPARK_HEIGHT -
      (Math.max(0, Math.min(point.avg, max)) / max) * (SPARK_HEIGHT - SPARK_INSET * 2) -
      SPARK_INSET
  }))
}

function sparkPath(
  points: HistoryPoint[],
  maxValue: number,
  fromMs: number,
  toMs: number
): string {
  if (points.length === 0) {
    return ''
  }
  return sparkCoords(points, maxValue, fromMs, toMs)
    .map((p, i) => `${i === 0 ? 'M' : 'L'}${p.x.toFixed(1)} ${p.y.toFixed(1)}`)
    .join(' ')
}

function sparkArea(
  points: HistoryPoint[],
  maxValue: number,
  fromMs: number,
  toMs: number
): string {
  const line = sparkPath(points, maxValue, fromMs, toMs)
  if (!line || points.length === 0) {
    return ''
  }
  const coords = sparkCoords(points, maxValue, fromMs, toMs)
  const lastX = coords[coords.length - 1].x
  const firstX = coords[0].x
  return `${line} L${lastX.toFixed(1)} ${SPARK_HEIGHT} L${firstX.toFixed(1)} ${SPARK_HEIGHT} Z`
}

function historyMax(points: HistoryPoint[]): number {
  let max = 0
  for (const point of points) {
    if (point.max > max) {
      max = point.max
    }
  }
  return max
}

/** Byte history as a percentage of `total` (empty while the total is unknown). */
function percentPoints(points: HistoryPoint[], total: number | undefined): HistoryPoint[] {
  if (!total) {
    return []
  }
  const scale = SERIES_PERCENT_MAX / total
  return points.map((point) => ({
    ...point,
    min: point.min * scale,
    max: point.max * scale,
    avg: point.avg * scale
  }))
}

function processSortValue(row: ServerMonitorProcess, sort: ServerMonitorProcessSort): string | number {
  switch (sort) {
    case 'pid':
      return row.pid
    case 'user':
      return row.user.toLowerCase()
    case 'state':
      return row.state.toLowerCase()
    case 'nice':
      return row.nice
    case 'threads':
      return row.threads
    case 'cpu':
      return row.cpuPercent
    case 'mem':
      return row.memPercent
    case 'command':
      return row.command.toLowerCase()
  }
}

/** Client reorder while waiting for the next remote sample */
function sortProcesses(
  rows: ServerMonitorProcess[],
  sort: ServerMonitorProcessSort,
  descending: boolean
): ServerMonitorProcess[] {
  const dir = descending ? -1 : 1
  return rows.slice().sort((a, b) => {
    const av = processSortValue(a, sort)
    const bv = processSortValue(b, sort)
    if (typeof av === 'number' && typeof bv === 'number') {
      return (av - bv) * dir
    }
    return String(av).localeCompare(String(bv)) * dir
  })
}

function netTotals(ifaces: ServerMonitorNetIface[]): {
  rxBytes: number
  txBytes: number
  rxRate: number | null
  txRate: number | null
  /** Combined link capacity in bytes/sec; null if no iface reports speed */
  capacityBytesPerSec: number | null
} {
  let rxBytes = 0
  let txBytes = 0
  let rxRate = 0
  let txRate = 0
  let capacityBits = 0
  let haveRx = false
  let haveTx = false
  let haveSpeed = false
  for (const iface of ifaces) {
    rxBytes += iface.rxBytes
    txBytes += iface.txBytes
    if (iface.rxRate != null) {
      rxRate += iface.rxRate
      haveRx = true
    }
    if (iface.txRate != null) {
      txRate += iface.txRate
      haveTx = true
    }
    if (iface.speedBitsPerSec != null && iface.speedBitsPerSec > 0) {
      capacityBits += iface.speedBitsPerSec
      haveSpeed = true
    }
  }
  return {
    rxBytes,
    txBytes,
    rxRate: haveRx ? rxRate : null,
    txRate: haveTx ? txRate : null,
    capacityBytesPerSec: haveSpeed ? capacityBits / BITS_PER_BYTE : null
  }
}

/** Smallest assumed rung covering the peak rate; the top rung caps the scale. */
function assumedNetScale(peakBytesPerSec: number): number {
  for (const rung of NET_ASSUMED_SCALE_RUNGS) {
    if (peakBytesPerSec <= rung) {
      return rung
    }
  }
  return NET_ASSUMED_SCALE_RUNGS[NET_ASSUMED_SCALE_RUNGS.length - 1]
}

function formatLinkSpeed(bitsPerSec: number | null): string {
  if (bitsPerSec == null || !Number.isFinite(bitsPerSec) || bitsPerSec <= 0) {
    return MISSING
  }
  const mbps = bitsPerSec / SERVER_MONITOR_MEGABIT_BITS
  if (mbps >= 1000) {
    const gbps = mbps / 1000
    return `${gbps >= 10 ? gbps.toFixed(0) : gbps.toFixed(1)} Gbps`
  }
  return `${mbps.toFixed(0)} Mbps`
}

function memSegments(snapshot: ServerMonitorSnapshot): {
  app: number
  buffers: number
  cached: number
  free: number
} {
  const total = snapshot.memTotalBytes
  const buffers = snapshot.memBuffersBytes
  const cached = snapshot.memCachedBytes
  const free = snapshot.memFreeBytes
  const app = Math.max(0, total - free - buffers - cached)
  return { app, buffers, cached, free }
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
  tone: GaugeTone
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
          {value == null ? MISSING : `${Math.round(value)}%`}
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
  points,
  tone,
  current,
  fromMs,
  toMs,
  scaleMax,
  formatScale
}: {
  title: string
  points: HistoryPoint[]
  tone: SparkTone
  current: string
  /** Left edge of the drawn window (ms) */
  fromMs: number
  /** Right edge of the drawn window (ms) */
  toMs: number
  /** When set, the sparkline scales to this ceiling (or the observed max if 0); else 0–100 */
  scaleMax?: number
  /** Formats the scale max; defaults to a percentage */
  formatScale?: (value: number) => string
}): ReactElement {
  const max = scaleMax != null ? Math.max(scaleMax, historyMax(points), 1) : SERIES_PERCENT_MAX
  const line = sparkPath(points, max, fromMs, toMs)
  const area = sparkArea(points, max, fromMs, toMs)
  return (
    <div className={`monitor-spark monitor-spark-${tone}`}>
      <div className="monitor-spark-head">
        <span>{title}</span>
        <strong>{current}</strong>
      </div>
      <div className="monitor-spark-plot">
        <span className="monitor-spark-scale">
          {formatScale ? formatScale(max) : `${Math.round(max)}%`}
        </span>
        <svg
          className="monitor-spark-svg"
          viewBox={`0 0 ${SPARK_WIDTH} ${SPARK_HEIGHT}`}
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          <path className="monitor-spark-grid" d={`M0 ${SPARK_INSET} H${SPARK_WIDTH}`} />
          {area ? <path className="monitor-spark-area" d={area} /> : null}
          {line ? <path className="monitor-spark-line" d={line} fill="none" /> : null}
        </svg>
      </div>
    </div>
  )
}

/** SVG viewBox units for bar geometry (avoids inline CSS widths) */
const BAR_VIEW = 100

function CoreBars({ cores }: { cores: Array<number | null> }): ReactElement | null {
  if (cores.length <= 1) {
    return null
  }
  return (
    <div className="monitor-cores" aria-label="Per-core CPU">
      <div className="monitor-section-title">CPU cores</div>
      <div className="monitor-cores-grid">
        {cores.map((pct, i) => {
          const value = pct == null ? 0 : clampPercent(pct)
          const fillH = value
          const fillY = BAR_VIEW - fillH
          return (
            <div
              key={i}
              className="monitor-core"
              title={pct == null ? MISSING : `${formatPercent(pct)}%`}
            >
              <svg
                className="monitor-core-svg"
                viewBox={`0 0 ${BAR_VIEW} ${BAR_VIEW}`}
                preserveAspectRatio="none"
                aria-hidden="true"
              >
                <rect className="monitor-core-track" x="0" y="0" width={BAR_VIEW} height={BAR_VIEW} />
                <rect
                  className="monitor-core-fill"
                  x="0"
                  y={fillY}
                  width={BAR_VIEW}
                  height={fillH}
                />
              </svg>
              <span className="monitor-core-label">{i}</span>
            </div>
          )
        })}
      </div>
    </div>
  )
}

function MemBreakdown({ snapshot }: { snapshot: ServerMonitorSnapshot }): ReactElement | null {
  if (!(snapshot.memTotalBytes > 0)) {
    return null
  }
  const segs = memSegments(snapshot)
  const total = snapshot.memTotalBytes
  const parts: Array<{ key: string; bytes: number; tone: string }> = [
    { key: 'App', bytes: segs.app, tone: 'app' },
    { key: 'Buf', bytes: segs.buffers, tone: 'buffers' },
    { key: 'Cache', bytes: segs.cached, tone: 'cached' },
    { key: 'Free', bytes: segs.free, tone: 'free' }
  ]
  let cursor = 0
  const rects = parts
    .map((p) => {
      const pct = total > 0 ? (p.bytes / total) * BAR_VIEW : 0
      if (pct <= 0) {
        return null
      }
      const x = cursor
      cursor += pct
      return { ...p, x, w: pct }
    })
    .filter((r): r is NonNullable<typeof r> => r != null)

  return (
    <div className="monitor-mem-breakdown">
      <div className="monitor-section-title">Memory</div>
      <svg
        className="monitor-mem-stack"
        viewBox={`0 0 ${BAR_VIEW} ${BAR_VIEW}`}
        preserveAspectRatio="none"
        role="img"
        aria-label="Memory breakdown"
      >
        {rects.map((r) => (
          <rect
            key={r.key}
            className={`monitor-mem-seg monitor-mem-seg-${r.tone}`}
            x={r.x}
            y="0"
            width={r.w}
            height={BAR_VIEW}
          >
            <title>{`${r.key}: ${formatBytes(r.bytes)}`}</title>
          </rect>
        ))}
      </svg>
      <div className="monitor-mem-legend">
        {parts.map((p) => (
          <span key={p.key} className={`monitor-mem-legend-item monitor-mem-legend-${p.tone}`}>
            {p.key} {formatBytes(p.bytes)}
          </span>
        ))}
      </div>
    </div>
  )
}

function RateBar({
  label,
  rate,
  maxRate,
  tone
}: {
  label: string
  rate: number | null
  maxRate: number
  tone: 'rx' | 'tx'
}): ReactElement {
  let pct = 0
  if (rate != null && rate > 0 && maxRate > 0) {
    pct = Math.max(RATE_BAR_MIN_PCT, Math.min(BAR_VIEW, (rate / maxRate) * BAR_VIEW))
  }
  return (
    <div className={`monitor-rate-bar monitor-rate-bar-${tone}`}>
      <span className="monitor-rate-bar-label">{label}</span>
      <svg
        className="monitor-rate-bar-svg"
        viewBox={`0 0 ${BAR_VIEW} ${BAR_VIEW}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        <rect className="monitor-rate-bar-track" x="0" y="0" width={BAR_VIEW} height={BAR_VIEW} />
        <rect className="monitor-rate-bar-fill" x="0" y="0" width={pct} height={BAR_VIEW} />
      </svg>
      <span className="monitor-rate-bar-value">{formatRate(rate)}</span>
    </div>
  )
}

function ProcessTable({
  rows,
  sort,
  descending,
  onSort,
  onSignal,
  status,
  statusError
}: {
  rows: ServerMonitorProcess[]
  sort: ServerMonitorProcessSort
  descending: boolean
  onSort: (sort: ServerMonitorProcessSort) => void
  onSignal: (pid: number, signal: ServerMonitorProcessSignal, command: string) => void
  status: string | null
  statusError: boolean
}): ReactElement {
  const [contextMenu, setContextMenu] = useState<{
    x: number
    y: number
    process: ServerMonitorProcess
  } | null>(null)
  const contextMenuRef = useRef<HTMLDivElement>(null)
  const sorted = sortProcesses(rows, sort, descending)

  useEffect(() => {
    if (!contextMenu) {
      return
    }
    const onMouseDown = (e: MouseEvent): void => {
      if (contextMenuRef.current && contextMenuRef.current.contains(e.target as Node)) {
        return
      }
      setContextMenu(null)
    }
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        setContextMenu(null)
      }
    }
    const onBlur = (): void => setContextMenu(null)
    window.addEventListener('mousedown', onMouseDown)
    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('blur', onBlur)
    return () => {
      window.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('blur', onBlur)
    }
  }, [contextMenu])

  useLayoutEffect(() => {
    const el = contextMenuRef.current
    if (!contextMenu || !el) {
      return
    }
    const rect = el.getBoundingClientRect()
    let x = contextMenu.x
    let y = contextMenu.y
    if (x + rect.width > window.innerWidth - CONTEXT_MENU_EDGE_PAD) {
      x = Math.max(CONTEXT_MENU_EDGE_PAD, window.innerWidth - rect.width - CONTEXT_MENU_EDGE_PAD)
    }
    if (y + rect.height > window.innerHeight - CONTEXT_MENU_EDGE_PAD) {
      y = Math.max(CONTEXT_MENU_EDGE_PAD, window.innerHeight - rect.height - CONTEXT_MENU_EDGE_PAD)
    }
    el.style.left = `${x}px`
    el.style.top = `${y}px`
  }, [contextMenu])

  return (
    <div className="monitor-section monitor-processes">
      <div className="monitor-section-head">
        <div className="monitor-section-title">Processes</div>
        {status ? (
          <div
            className={`monitor-proc-status${statusError ? ' monitor-proc-status-error' : ''}`}
            role="status"
          >
            {status}
          </div>
        ) : null}
      </div>
      {sorted.length === 0 ? (
        <div className="monitor-section-empty">No process data</div>
      ) : (
        <table className="monitor-table">
          <thead>
            <tr>
              {PROCESS_COLUMNS.map((col) => {
                const active = sort === col.key
                const ariaSort = active
                  ? descending
                    ? 'descending'
                    : 'ascending'
                  : 'none'
                return (
                  <th key={col.key} scope="col" aria-sort={ariaSort}>
                    <button
                      type="button"
                      className={`monitor-th-btn${active ? ' active' : ''}${col.numeric ? ' monitor-th-num' : ''}`}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => onSort(col.key)}
                    >
                      <span>{col.label}</span>
                      {active ? (
                        <span className="monitor-th-dir" aria-hidden="true">
                          {descending ? '▼' : '▲'}
                        </span>
                      ) : null}
                    </button>
                  </th>
                )
              })}
            </tr>
          </thead>
          <tbody>
            {sorted.map((p) => {
              const menuOpen = contextMenu?.process.pid === p.pid
              return (
                <tr
                  key={`${p.pid}-${p.command}`}
                  className={menuOpen ? 'monitor-row-menu' : undefined}
                  onContextMenu={(e) => {
                    e.preventDefault()
                    setContextMenu({ x: e.clientX, y: e.clientY, process: p })
                  }}
                >
                  <td className="monitor-num">{p.pid}</td>
                  <td className="monitor-user" title={p.user}>
                    {p.user}
                  </td>
                  <td className="monitor-num monitor-state">{p.state}</td>
                  <td className="monitor-num">{p.nice}</td>
                  <td className="monitor-num">{p.threads}</td>
                  <td className="monitor-num">{formatPercent(p.cpuPercent)}%</td>
                  <td className="monitor-num">{formatPercent(p.memPercent)}%</td>
                  <td className="monitor-cmd" title={p.command}>
                    {p.command}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      )}
      {contextMenu ? (
        <div
          ref={contextMenuRef}
          className="monitor-context-menu"
          role="menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
        >
          <button
            type="button"
            className="monitor-context-item"
            role="menuitem"
            onClick={() => {
              const { process } = contextMenu
              setContextMenu(null)
              onSignal(process.pid, 'TERM', process.command)
            }}
          >
            Terminate (SIGTERM)
          </button>
          <button
            type="button"
            className="monitor-context-item danger"
            role="menuitem"
            onClick={() => {
              const { process } = contextMenu
              setContextMenu(null)
              onSignal(process.pid, 'KILL', process.command)
            }}
          >
            Kill (SIGKILL)
          </button>
        </div>
      ) : null}
    </div>
  )
}

function NetworkPanel({
  ifaces,
  showGauges,
  showSparks,
  rxPoints,
  txPoints,
  fromMs,
  toMs
}: {
  ifaces: ServerMonitorNetIface[]
  showGauges: boolean
  showSparks: boolean
  rxPoints: HistoryPoint[]
  txPoints: HistoryPoint[]
  fromMs: number
  toMs: number
}): ReactElement {
  const totals = netTotals(ifaces)
  const peakRate = Math.max(
    historyMax(rxPoints),
    historyMax(txPoints),
    totals.rxRate ?? 0,
    totals.txRate ?? 0
  )
  // Real combined capacity when known; otherwise assume a link ladder rung that
  // covers the observed peak (10 → 100 → 250 → 1000 MB/s).
  const capacity =
    totals.capacityBytesPerSec != null && totals.capacityBytesPerSec > 0
      ? totals.capacityBytesPerSec
      : assumedNetScale(peakRate)

  return (
    <div className="monitor-section monitor-network">
      <div className="monitor-section-title">Network</div>
      {showSparks ? (
        <div className="monitor-panel-sparks monitor-panel-sparks-2">
          <SparkCard
            title="Net ↓"
            points={rxPoints}
            tone="net"
            scaleMax={capacity}
            formatScale={formatRate}
            fromMs={fromMs}
            toMs={toMs}
            current={formatRate(totals.rxRate)}
          />
          <SparkCard
            title="Net ↑"
            points={txPoints}
            tone="net"
            scaleMax={capacity}
            formatScale={formatRate}
            fromMs={fromMs}
            toMs={toMs}
            current={formatRate(totals.txRate)}
          />
        </div>
      ) : null}
      {ifaces.length === 0 ? (
        <div className="monitor-section-empty">No interface data</div>
      ) : (
        <>
          <div className="monitor-net-totals">
            {showGauges ? (
              <>
                <RateBar label="↓ RX" rate={totals.rxRate} maxRate={capacity} tone="rx" />
                <RateBar label="↑ TX" rate={totals.txRate} maxRate={capacity} tone="tx" />
              </>
            ) : null}
            <div className="monitor-net-total-bytes">
              <span>Total RX {formatBytes(totals.rxBytes)}</span>
              <span>Total TX {formatBytes(totals.txBytes)}</span>
              <span>
                Cap{' '}
                {totals.capacityBytesPerSec != null
                  ? formatLinkSpeed(totals.capacityBytesPerSec * BITS_PER_BYTE)
                  : `~${formatRate(capacity)}`}
              </span>
            </div>
          </div>
          <table className="monitor-table">
            <thead>
              <tr>
                <th scope="col">Iface</th>
                <th scope="col">Speed</th>
                <th scope="col">RX</th>
                <th scope="col">TX</th>
                <th scope="col">Total RX</th>
                <th scope="col">Total TX</th>
              </tr>
            </thead>
            <tbody>
              {ifaces.map((iface) => (
                <tr key={iface.name}>
                  <td
                    className="monitor-iface"
                    title={iface.ips.length > 0 ? iface.ips.join(', ') : undefined}
                  >
                    {iface.name}
                  </td>
                  <td className="monitor-num">{formatLinkSpeed(iface.speedBitsPerSec)}</td>
                  <td className="monitor-num">{formatRate(iface.rxRate)}</td>
                  <td className="monitor-num">{formatRate(iface.txRate)}</td>
                  <td className="monitor-num">{formatBytes(iface.rxBytes)}</td>
                  <td className="monitor-num">{formatBytes(iface.txBytes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </div>
  )
}

function DiskPanel({
  snapshot,
  showGauge,
  showSparks,
  usedPoints,
  readPoints,
  writePoints,
  fromMs,
  toMs
}: {
  snapshot: ServerMonitorSnapshot | null
  showGauge: boolean
  showSparks: boolean
  usedPoints: HistoryPoint[]
  readPoints: HistoryPoint[]
  writePoints: HistoryPoint[]
  fromMs: number
  toMs: number
}): ReactElement {
  const diskPct = snapshot ? ratioPercent(snapshot.diskUsedBytes, snapshot.diskTotalBytes) : null
  const diskFree =
    snapshot && snapshot.diskTotalBytes > 0
      ? Math.max(0, snapshot.diskTotalBytes - snapshot.diskUsedBytes)
      : 0

  return (
    <div className="monitor-section monitor-disk">
      <div className="monitor-section-title">Disk</div>
      {showGauge ? (
        <div className="monitor-panel-gauge">
          <Gauge
            label="Disk /"
            percent={diskPct}
            detail={
              snapshot
                ? `${formatBytes(snapshot.diskUsedBytes)} / ${formatBytes(snapshot.diskTotalBytes)}`
                : MISSING
            }
            tone="disk"
          />
        </div>
      ) : null}
      {showSparks ? (
        <div className="monitor-panel-sparks monitor-panel-sparks-3">
          <SparkCard
            title="Used"
            points={usedPoints}
            tone="disk"
            fromMs={fromMs}
            toMs={toMs}
            current={diskPct == null ? MISSING : `${Math.round(diskPct)}%`}
          />
          <SparkCard
            title="Read"
            points={readPoints}
            tone="disk"
            fromMs={fromMs}
            toMs={toMs}
            scaleMax={historyMax(readPoints)}
            formatScale={formatRate}
            current={formatRate(snapshot?.diskReadRate)}
          />
          <SparkCard
            title="Write"
            points={writePoints}
            tone="disk"
            fromMs={fromMs}
            toMs={toMs}
            scaleMax={historyMax(writePoints)}
            formatScale={formatRate}
            current={formatRate(snapshot?.diskWriteRate)}
          />
        </div>
      ) : null}
      <div className="monitor-panel-facts">
        <div className="monitor-fact">
          <span>Free</span>
          <strong>
            {snapshot && snapshot.diskTotalBytes > 0 ? formatBytes(diskFree) : MISSING}
          </strong>
        </div>
        <div className="monitor-fact">
          <span>Read</span>
          <strong>{formatRate(snapshot?.diskReadRate)}</strong>
        </div>
        <div className="monitor-fact">
          <span>Write</span>
          <strong>{formatRate(snapshot?.diskWriteRate)}</strong>
        </div>
        <div className="monitor-fact">
          <span>Read total</span>
          <strong>{snapshot ? formatBytes(snapshot.diskReadBytes) : MISSING}</strong>
        </div>
        <div className="monitor-fact">
          <span>Write total</span>
          <strong>{snapshot ? formatBytes(snapshot.diskWriteBytes) : MISSING}</strong>
        </div>
      </div>
    </div>
  )
}

function TempFacts({ temps }: { temps: ServerMonitorTemp[] }): ReactElement | null {  if (temps.length === 0) {
    return null
  }
  return (
    <>
      {temps.map((t) => (
        <div key={t.name} className="monitor-fact">
          <span title={t.name}>{`Temp (${t.name}):`}</span>
          <strong>{formatTemp(t.celsius)}</strong>
        </div>
      ))}
    </>
  )
}

export default function ServerMonitorView({
  tabId,
  pluginId,
  settings,
  onSettingsPatch
}: PluginViewProps) {
  const [snapshot, setSnapshot] = useState<ServerMonitorSnapshot | null>(null)
  const [history, setHistory] = useState<HistoryStore>({})
  const [series, setSeries] = useState<MonitorSeries[]>([])
  const [range, setRange] = useState<HistoryRange>(HISTORY_RANGE_DEFAULT)
  const [service, setService] = useState<MonitorServiceStatus>(MONITOR_SERVICE_INITIAL_STATUS)
  const [sudoPrompt, setSudoPrompt] = useState(false)
  const [password, setPassword] = useState('')
  const [actionError, setActionError] = useState('')
  const [procSort, setProcSort] = useState<ServerMonitorProcessSort>(
    SERVER_MONITOR_PROCESS_SORT_DEFAULT
  )
  const [procSortDesc, setProcSortDesc] = useState(SERVER_MONITOR_PROCESS_SORT_DESC_DEFAULT)
  const [procStatus, setProcStatus] = useState<string | null>(null)
  const [procStatusError, setProcStatusError] = useState(false)
  const lastAt = useRef(0)
  /** Newest bucket timestamp drawn; refreshes only fetch past this edge. */
  const newestBucketAt = useRef(0)
  const rangeRef = useRef<HistoryRange>(HISTORY_RANGE_DEFAULT)
  const statusTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const showGauges = settingBool(settings, 'showGauges', SERVER_MONITOR_SHOW_GAUGES_DEFAULT)
  const showSparks = settingBool(settings, 'showSparks', SERVER_MONITOR_SHOW_SPARKS_DEFAULT)
  const showStatus = settingBool(settings, 'showStatus', SERVER_MONITOR_SHOW_STATUS_DEFAULT)
  const showProcesses = settingBool(
    settings,
    'showProcesses',
    SERVER_MONITOR_SHOW_PROCESSES_DEFAULT
  )
  const showNetwork = settingBool(settings, 'showNetwork', SERVER_MONITOR_SHOW_NETWORK_DEFAULT)
  const showDiskPanel = showGauges || showSparks || showStatus
  const operationRunning = service.state === 'installing' || service.state === 'uninstalling'
  const serviceNeedsAttention =
    service.state === 'missing' || service.state === 'outdated' || service.state === 'error'

  const showProcStatus = (message: string, isError: boolean): void => {
    setProcStatus(message)
    setProcStatusError(isError)
    if (statusTimer.current) {
      clearTimeout(statusTimer.current)
    }
    statusTimer.current = setTimeout(() => {
      setProcStatus(null)
      setProcStatusError(false)
      statusTimer.current = null
    }, PROC_STATUS_MS)
  }

  useEffect(() => {
    return () => {
      if (statusTimer.current) {
        clearTimeout(statusTimer.current)
      }
    }
  }, [])

  /**
   * Ask the main process for the buckets covering the selected range.
   * `sinceMs` merges into the drawn history instead of reloading it, so
   * periodic refreshes never blank the graphs.
   */
  const requestHistory = (nextRange: HistoryRange, sinceMs?: number): void => {
    const toMs = Date.now()
    const windowFromMs = toMs - HISTORY_RANGES[nextRange].seconds * MS_PER_SEC
    if (sinceMs == null) {
      setHistory({})
      newestBucketAt.current = 0
    }
    void window.wassh.sendPluginMessage(tabId, pluginId, {
      type: 'requestHistory',
      bucketSeconds: HISTORY_RANGES[nextRange].tier.bucketSeconds,
      fromMs: Math.max(windowFromMs, sinceMs ?? 0),
      toMs
    } satisfies ServerMonitorRendererMessage)
  }

  useEffect(() => {
    requestHistory(rangeRef.current)
    void window.wassh.sendPluginMessage(tabId, pluginId, { type: 'probeService' })
  }, [tabId, pluginId])

  useEffect(() => {
    return window.wassh.onPluginMessage((ev) => {
      if (ev.tabId !== tabId || ev.pluginId !== pluginId) {
        return
      }
      if (isMonitorMainMessage(ev.payload)) {
        if (ev.payload.type === 'service') {
          setService(ev.payload.status)
          return
        }
        if (ev.payload.type === 'historyReady') {
          setSeries(ev.payload.series)
          return
        }
        const bucket = ev.payload
        if (bucket.timestamp > newestBucketAt.current) {
          newestBucketAt.current = bucket.timestamp
        }
        setHistory((current) => {
          const points = current[bucket.seriesId] ?? []
          const point = {
            timestamp: bucket.timestamp,
            min: bucket.min,
            max: bucket.max,
            avg: bucket.avg
          }
          const index = points.findIndex((existing) => existing.timestamp === bucket.timestamp)
          const next = points.slice()
          if (index < 0) {
            next.push(point)
          } else {
            next[index] = point
          }
          // Buckets arrive oldest first; drop the ones that left the drawn window.
          const windowFromMs = Date.now() - HISTORY_RANGES[rangeRef.current].seconds * MS_PER_SEC
          const kept =
            next.length > 0 && next[0].timestamp < windowFromMs
              ? next.filter((existing) => existing.timestamp >= windowFromMs)
              : next
          return { ...current, [bucket.seriesId]: kept }
        })
        return
      }
      if (!isServerMonitorStatsEvent(ev.payload)) {
        return
      }
      const next = ev.payload.snapshot
      setSnapshot(next)
      if (next.error || next.updatedAt === lastAt.current) {
        return
      }
      lastAt.current = next.updatedAt
      requestHistory(rangeRef.current, newestBucketAt.current)
    })
  }, [tabId, pluginId])

  const handleProcSort = (key: ServerMonitorProcessSort): void => {
    let nextDesc: boolean
    if (key === procSort) {
      nextDesc = !procSortDesc
    } else {
      nextDesc = defaultDescending(key)
    }
    setProcSort(key)
    setProcSortDesc(nextDesc)
    void window.wassh.sendPluginMessage(tabId, pluginId, {
      type: 'setProcessSort',
      sort: key,
      descending: nextDesc
    } satisfies ServerMonitorRendererMessage)
  }

  const handleProcSignal = (
    pid: number,
    signal: ServerMonitorProcessSignal,
    command: string
  ): void => {
    const label = signal === 'KILL' ? 'SIGKILL' : 'SIGTERM'
    const shortCmd =
      command.length > PROC_STATUS_CMD_MAX
        ? `${command.slice(0, PROC_STATUS_CMD_MAX)}…`
        : command
    void (async () => {
      const response = await window.wassh.sendPluginMessage(tabId, pluginId, {
        type: 'signalProcess',
        pid,
        signal
      } satisfies ServerMonitorRendererMessage)
      const result = isServerMonitorActionResult(response) ? response : null
      if (result?.ok) {
        showProcStatus(`${label} → ${pid} ${shortCmd}`, false)
        return
      }
      showProcStatus(result?.error || `Failed to send ${label} to ${pid}`, true)
    })()
  }

  const memPct = snapshot ? ratioPercent(snapshot.memUsedBytes, snapshot.memTotalBytes) : null
  const swapPct = snapshot ? ratioPercent(snapshot.swapUsedBytes, snapshot.swapTotalBytes) : null
  const showSwapGauge = Boolean(snapshot && snapshot.swapTotalBytes > 0)
  const showCpuBlock = showGauges || showSparks

  const selectRange = (nextRange: HistoryRange): void => {
    rangeRef.current = nextRange
    setRange(nextRange)
    requestHistory(nextRange)
  }

  const runInstall = async (): Promise<void> => {
    setActionError('')
    const result = await window.wassh.sendPluginMessage(tabId, pluginId, {
      type: 'installService',
      password
    } satisfies ServerMonitorRendererMessage)
    setPassword('')
    if (!isMonitorActionResult(result) || !result.ok) {
      setActionError(isMonitorActionResult(result) ? result.error || 'Operation failed' : 'Operation failed')
      return
    }
    setSudoPrompt(false)
  }

  const toMs = Date.now()
  const fromMs = toMs - HISTORY_RANGES[range].seconds * MS_PER_SEC
  const pointsFor = (seriesId: string): HistoryPoint[] => history[seriesId] ?? []
  const netRxPoints = useMemo(() => {
    const totals: Record<number, HistoryPoint> = {}
    for (const item of series) {
      if (item.kind !== 'rate' || !item.id.startsWith('net:') || !item.id.endsWith(':rx')) {
        continue
      }
      for (const point of pointsFor(item.id)) {
        const existing = totals[point.timestamp]
        totals[point.timestamp] = existing
          ? { ...existing, avg: existing.avg + point.avg, max: existing.max + point.max }
          : point
      }
    }
    return Object.values(totals).sort((a, b) => a.timestamp - b.timestamp)
  }, [series, history])
  const netTxPoints = useMemo(() => {
    const totals: Record<number, HistoryPoint> = {}
    for (const item of series) {
      if (item.kind !== 'rate' || !item.id.startsWith('net:') || !item.id.endsWith(':tx')) {
        continue
      }
      for (const point of pointsFor(item.id)) {
        const existing = totals[point.timestamp]
        totals[point.timestamp] = existing
          ? { ...existing, avg: existing.avg + point.avg, max: existing.max + point.max }
          : point
      }
    }
    return Object.values(totals).sort((a, b) => a.timestamp - b.timestamp)
  }, [series, history])

  return (
    <div className="plugin-panel plugin-server-monitor">
      <div className="monitor-scroll">
        <div className="monitor-header">
          <div className="monitor-section-toggles" role="group" aria-label="Visible sections">
            {showSparks ? (
              <select
                className="monitor-range"
                value={range}
                aria-label="History range"
                onChange={(e) => selectRange(e.target.value as HistoryRange)}
              >
                {(Object.keys(HISTORY_RANGES) as HistoryRange[]).map((value) => (
                  <option key={value} value={value}>
                    {value}
                  </option>
                ))}
              </select>
            ) : null}
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
          {showCpuBlock ? (
            <div className="monitor-overview monitor-cpu-block">
              {showGauges ? (
                <div className="monitor-gauges-wrap">
                  <div className={`monitor-gauges${showSwapGauge ? ' monitor-gauges-3' : ''}`}>
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
                          : MISSING
                      }
                      tone="mem"
                    />
                    {showSwapGauge ? (
                      <Gauge
                        label="Swap"
                        percent={swapPct}
                        detail={`${formatBytes(snapshot!.swapUsedBytes)} / ${formatBytes(snapshot!.swapTotalBytes)}`}
                        tone="swap"
                      />
                    ) : null}
                  </div>
                  {snapshot ? <CoreBars cores={snapshot.cpuCores ?? []} /> : null}
                  {snapshot ? <MemBreakdown snapshot={snapshot} /> : null}
                </div>
              ) : null}

              {showSparks ? (
                <div className="monitor-sparks monitor-panel-sparks-2">
                  <SparkCard
                    title="CPU"
                    points={pointsFor(SERIES_CPU)}
                    tone="cpu"
                    fromMs={fromMs}
                    toMs={toMs}
                    current={
                      snapshot?.cpuPercent == null
                        ? MISSING
                        : `${Math.round(snapshot.cpuPercent)}%`
                    }
                  />
                  <SparkCard
                    title="Memory"
                    points={percentPoints(pointsFor(SERIES_MEM_USED), snapshot?.memTotalBytes)}
                    tone="mem"
                    fromMs={fromMs}
                    toMs={toMs}
                    current={memPct == null ? MISSING : `${Math.round(memPct)}%`}
                  />
                </div>
              ) : null}
            </div>
          ) : null}

          {showNetwork ? (
            <NetworkPanel
              ifaces={snapshot?.network ?? []}
              showGauges={showGauges}
              showSparks={showSparks}
              rxPoints={netRxPoints}
              txPoints={netTxPoints}
              fromMs={fromMs}
              toMs={toMs}
            />
          ) : null}

          {showDiskPanel ? (
            <DiskPanel
              snapshot={snapshot}
              showGauge={showGauges}
              showSparks={showSparks}
              usedPoints={percentPoints(pointsFor(SERIES_DISK_USED), snapshot?.diskTotalBytes)}
              readPoints={pointsFor(SERIES_DISK_READ)}
              writePoints={pointsFor(SERIES_DISK_WRITE)}
              fromMs={fromMs}
              toMs={toMs}
            />
          ) : null}

          {showStatus ? (
            <div className="monitor-facts">
              <div className="monitor-fact">
                <span>Uptime</span>
                <strong>{formatUptime(snapshot?.uptimeSec ?? 0)}</strong>
              </div>
              <div className="monitor-fact">
                <span>Load avg</span>
                <strong>
                  {snapshot
                    ? `${formatLoad(snapshot.load1)} · ${formatLoad(snapshot.load5)} · ${formatLoad(snapshot.load15)}`
                    : MISSING}
                </strong>
              </div>
              <div className="monitor-fact">
                <span title="Free + reclaimable RAM">RAM free</span>
                <strong>
                  {snapshot ? formatBytes(snapshot.memAvailableBytes) : MISSING}
                </strong>
              </div>
              <div className="monitor-fact">
                <span>Tasks</span>
                <strong>
                  {snapshot && snapshot.procsTotal > 0
                    ? `${snapshot.procsRunning} / ${snapshot.procsTotal}`
                    : MISSING}
                </strong>
              </div>
              <div className="monitor-fact">
                <span>CPUs</span>
                <strong>
                  {snapshot && snapshot.cpuCount > 0 ? String(snapshot.cpuCount) : MISSING}
                </strong>
              </div>
              <TempFacts temps={snapshot?.temperatures ?? []} />
              <div className="monitor-fact monitor-fact-wide">
                <span>OS</span>
                <strong title={snapshot?.distro || ''}>{snapshot?.distro || MISSING}</strong>
              </div>
              <div className="monitor-fact monitor-fact-wide">
                <span>Kernel</span>
                <strong title={snapshot?.kernel || ''}>{snapshot?.kernel || MISSING}</strong>
              </div>
              <div className="monitor-fact monitor-fact-wide">
                <span>IP address</span>
                <strong
                  title={snapshot && snapshot.ips.length > 0 ? snapshot.ips.join(', ') : ''}
                >
                  {snapshot && snapshot.ips.length > 0 ? snapshot.ips.join(', ') : MISSING}
                </strong>
              </div>
            </div>
          ) : null}

          {showProcesses ? (
            <ProcessTable
              rows={snapshot?.processes ?? []}
              sort={procSort}
              descending={procSortDesc}
              onSort={handleProcSort}
              onSignal={handleProcSignal}
              status={procStatus}
              statusError={procStatusError}
            />
          ) : null}
        </div>
      </div>

      {snapshot?.updatedAt ? (
        <div className="plugin-monitor-meta">
          Updated {new Date(snapshot.updatedAt).toLocaleTimeString()}
        </div>
      ) : null}

      {serviceNeedsAttention ? (
        <div className="monitor-service-panel">
          <div className="monitor-section-title">WaSSH Service</div>
          <p className="monitor-service-note">
            {service.message ||
              'Install WaSSH Service for retained, multi-resolution history that survives restarts.'}
          </p>
          {actionError ? <div className="plugin-monitor-error">{actionError}</div> : null}
          {sudoPrompt ? (
            <form
              className="monitor-service-sudo"
              onSubmit={(e) => {
                e.preventDefault()
                void runInstall()
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
              <small>
                Kept in memory only for this operation. Leave blank for passwordless sudo.
              </small>
              <div className="monitor-service-actions">
                <PluginButton type="submit" compact variant="primary" disabled={operationRunning}>
                  {operationRunning
                    ? 'Working…'
                    : service.state === 'outdated'
                      ? 'Upgrade service'
                      : 'Install service'}
                </PluginButton>
                <PluginButton compact disabled={operationRunning} onClick={() => setSudoPrompt(false)}>
                  Cancel
                </PluginButton>
              </div>
            </form>
          ) : (
            <div className="monitor-service-actions">
              <PluginButton
                compact
                variant="primary"
                disabled={operationRunning}
                onClick={() => setSudoPrompt(true)}
              >
                {service.state === 'outdated' ? 'Upgrade service' : 'Install service'}
              </PluginButton>
            </div>
          )}
        </div>
      ) : null}
    </div>
  )
}
