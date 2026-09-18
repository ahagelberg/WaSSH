/** Process table sort column (remote `ps --sort`) */
export type ServerMonitorProcessSort =
  | 'pid'
  | 'user'
  | 'state'
  | 'nice'
  | 'threads'
  | 'cpu'
  | 'mem'
  | 'command'

/** Default process list sort column */
export const SERVER_MONITOR_PROCESS_SORT_DEFAULT: ServerMonitorProcessSort = 'cpu'

/** Default direction for CPU sort (highest first) */
export const SERVER_MONITOR_PROCESS_SORT_DESC_DEFAULT = true

/** Signals the monitor can send to a remote process */
export type ServerMonitorProcessSignal = 'TERM' | 'KILL'

/** Signals offered in the details dialog's selection list (0–15) */
export const SERVER_MONITOR_COMMON_SIGNALS: Array<{ name: string; number: number }> = [
  { name: 'HUP', number: 1 },
  { name: 'INT', number: 2 },
  { name: 'QUIT', number: 3 },
  { name: 'ILL', number: 4 },
  { name: 'TRAP', number: 5 },
  { name: 'ABRT', number: 6 },
  { name: 'BUS', number: 7 },
  { name: 'FPE', number: 8 },
  { name: 'KILL', number: 9 },
  { name: 'USR1', number: 10 },
  { name: 'SEGV', number: 11 },
  { name: 'USR2', number: 12 },
  { name: 'PIPE', number: 13 },
  { name: 'ALRM', number: 14 },
  { name: 'TERM', number: 15 }
]

/** Signal name or number accepted by `kill -s` (e.g. `TERM`, `SIGTERM`, `15`) */
const SIGNAL_RE = /^(?:SIG)?[A-Z][A-Z0-9]*$|^\d{1,2}$/i

/** Whether a signal is safe to pass to `kill -s` on the remote host */
export function isKillSignal(value: unknown): value is string {
  return typeof value === 'string' && SIGNAL_RE.test(value.trim())
}

/** Valid process sort columns (for message validation) */
export const SERVER_MONITOR_PROCESS_SORT_KEYS: ServerMonitorProcessSort[] = [
  'pid',
  'user',
  'state',
  'nice',
  'threads',
  'cpu',
  'mem',
  'command'
]

export function isServerMonitorProcessSort(value: unknown): value is ServerMonitorProcessSort {
  return typeof value === 'string' && SERVER_MONITOR_PROCESS_SORT_KEYS.some((key) => key === value)
}

export function isServerMonitorProcessSignal(value: unknown): value is ServerMonitorProcessSignal {
  return value === 'TERM' || value === 'KILL'
}

/** One labelled field in the process details dialog */
export interface ServerMonitorProcessDetail {
  label: string
  value: string
}

/** One CPU-percent sample for the details dialog's history graph */
export interface ServerMonitorProcessCpuSample {
  /** Sample timestamp (ms since epoch) */
  at: number
  cpuPercent: number
}
/** Extra process info fetched on demand for the details dialog */
export interface ServerMonitorProcessDetails {
  pid: number
  /** Fields in display order; empty values are omitted */
  fields: ServerMonitorProcessDetail[]
  /** CPU history from past samples (oldest first); empty when none recorded */
  cpuHistory: ServerMonitorProcessCpuSample[]
}

/** One row from remote `ps` */
export interface ServerMonitorProcess {
  pid: number
  user: string
  /** Single-letter process state from `ps` */
  state: string
  nice: number
  threads: number
  cpuPercent: number
  memPercent: number
  rssBytes: number
  command: string
}

/** Per-interface counters and derived rates */
export interface ServerMonitorNetIface {
  name: string
  /** Non-loopback IP addresses configured on this interface */
  ips: string[]
  rxBytes: number
  txBytes: number
  /** Bytes/sec; null until two samples exist */
  rxRate: number | null
  /** Bytes/sec; null until two samples exist */
  txRate: number | null
  /** Nominal link speed (bits/sec); null if unknown or down */
  speedBitsPerSec: number | null
}

/** Thermal zone reading when /sys exposes it */
export interface ServerMonitorTemp {
  name: string
  celsius: number
}

/** Structured stats pushed from the server-monitor main module to the UI */
export interface ServerMonitorSnapshot {
  updatedAt: number
  hostname: string
  /** Global (non-loopback) IPv4 addresses seen on the remote host */
  ips: string[]
  uptimeSec: number
  load1: number
  load5: number
  load15: number
  /** Best-effort distro name parsed from /etc/os-release and friends */
  distro: string
  /** Kernel / OS summary from `uname` */
  kernel: string
  /** Logical CPU count */
  cpuCount: number
  /** Runnable threads (from loadavg) */
  procsRunning: number
  /** Total threads (from loadavg) */
  procsTotal: number
  /** Aggregate 0-100; null until two CPU samples exist */
  cpuPercent: number | null
  /** Per-logical-CPU 0-100; null entries until two samples */
  cpuCores: Array<number | null>
  memTotalBytes: number
  /** total - available (gauge / overall used) */
  memUsedBytes: number
  memAvailableBytes: number
  memFreeBytes: number
  memBuffersBytes: number
  memCachedBytes: number
  swapTotalBytes: number
  swapUsedBytes: number
  diskTotalBytes: number
  diskUsedBytes: number
  /** Cumulative whole-disk bytes since boot (diskstats) */
  diskReadBytes: number
  diskWriteBytes: number
  /** Bytes/sec; null until two samples */
  diskReadRate: number | null
  diskWriteRate: number | null
  temperatures: ServerMonitorTemp[]
  processes: ServerMonitorProcess[]
  network: ServerMonitorNetIface[]
  error?: string
}

/** `plugin:message` push from the server-monitor main module to its view */
export interface ServerMonitorStatsEvent {
  type: 'stats'
  snapshot: ServerMonitorSnapshot
}

export function isServerMonitorStatsEvent(value: unknown): value is ServerMonitorStatsEvent {
  if (!value || typeof value !== 'object') {
    return false
  }
  const { type, snapshot } = value as Partial<ServerMonitorStatsEvent>
  return type === 'stats' && typeof snapshot === 'object' && snapshot !== null
}

/** Discriminant of snapshot/process requests the view sends to its main module */
export type ServerMonitorRendererMessageType =
  | 'refresh'
  | 'setProcessSort'
  | 'signalProcess'
  | 'processDetails'

/**
 * Snapshot/process requests the server-monitor view sends via
 * `sendPluginMessage` (service and history messages: see `serviceProtocol.ts`).
 * Field types reflect renderer-side intent only - the main module still
 * validates every field at runtime since the payload crosses the IPC boundary
 * as `unknown`.
 */
export type ServerMonitorRendererMessage =
  | { type: 'refresh' }
  | { type: 'setProcessSort'; sort: ServerMonitorProcessSort; descending?: boolean }
  | { type: 'signalProcess'; pid: number; signal: ServerMonitorProcessSignal }
  | { type: 'signalProcess'; pid: number; signalName: string }
  | {
      type: 'processDetails'
      pid: number
      /** Window to return CPU history for, in seconds; matches the panel range */
      windowSeconds?: number
      /** Bucket width to downsample the history to, in seconds */
      bucketSeconds?: number
    }

/** Narrows to a message with a recognized `type`; per-field values are still `unknown`. */
export function isServerMonitorRendererMessageEnvelope(
  value: unknown
): value is { type: ServerMonitorRendererMessageType } & Record<string, unknown> {
  if (!value || typeof value !== 'object') {
    return false
  }
  const type = (value as Record<string, unknown>).type
  return (
    type === 'refresh' ||
    type === 'setProcessSort' ||
    type === 'signalProcess' ||
    type === 'processDetails' ||
    type === 'requestHistory'
  )
}

/** Result of a `setProcessSort` / `signalProcess` / `refresh` request */
export interface ServerMonitorActionResult {
  ok: boolean
  error?: string
}

export function isServerMonitorActionResult(value: unknown): value is ServerMonitorActionResult {
  if (!value || typeof value !== 'object') {
    return false
  }
  return typeof (value as Partial<ServerMonitorActionResult>).ok === 'boolean'
}

/** Result of a `processDetails` request */
export interface ServerMonitorProcessDetailsResult {
  ok: boolean
  error?: string
  details?: ServerMonitorProcessDetails
}

export function isServerMonitorProcessDetailsResult(
  value: unknown
): value is ServerMonitorProcessDetailsResult {
  if (!value || typeof value !== 'object') {
    return false
  }
  return typeof (value as Partial<ServerMonitorProcessDetailsResult>).ok === 'boolean'
}
