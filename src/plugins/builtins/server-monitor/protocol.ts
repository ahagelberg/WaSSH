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

/** Discriminant of requests the server-monitor view sends to its main module */
export type ServerMonitorRendererMessageType = 'refresh' | 'setProcessSort' | 'signalProcess'

/**
 * Requests the server-monitor view sends via `sendPluginMessage`. Field types
 * reflect renderer-side intent only - the main module still validates every
 * field at runtime since the payload crosses the IPC boundary as `unknown`.
 */
export type ServerMonitorRendererMessage =
  | { type: 'refresh' }
  | { type: 'setProcessSort'; sort: ServerMonitorProcessSort; descending?: boolean }
  | { type: 'signalProcess'; pid: number; signal: ServerMonitorProcessSignal }

/** Narrows to a message with a recognized `type`; per-field values are still `unknown`. */
export function isServerMonitorRendererMessageEnvelope(
  value: unknown
): value is { type: ServerMonitorRendererMessageType } & Record<string, unknown> {
  if (!value || typeof value !== 'object') {
    return false
  }
  const type = (value as Record<string, unknown>).type
  return type === 'refresh' || type === 'setProcessSort' || type === 'signalProcess'
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
