export const DAEMON_MONITOR_SAMPLE_INTERVAL_SEC = 10
export const DAEMON_MONITOR_RETENTION_DAYS = 7
export const DAEMON_MONITOR_MAX_HISTORY_MIB = 10
export const DAEMON_MONITOR_DEFAULT_RANGE = '1h' as const
export const DAEMON_MONITOR_MAX_PING_TARGETS = 16

export const DAEMON_MONITOR_RANGES = {
  '15m': 15 * 60,
  '1h': 60 * 60,
  '6h': 6 * 60 * 60,
  '24h': 24 * 60 * 60,
  '7d': DAEMON_MONITOR_RETENTION_DAYS * 24 * 60 * 60
} as const

export type DaemonMonitorRange = keyof typeof DAEMON_MONITOR_RANGES

export function isDaemonMonitorRange(value: unknown): value is DaemonMonitorRange {
  return typeof value === 'string' && value in DAEMON_MONITOR_RANGES
}
