/** Default poll interval for server monitor (ms) */
export const SERVER_MONITOR_DEFAULT_INTERVAL_MS = 1000

/** Minimum poll interval for server monitor (ms) */
export const SERVER_MONITOR_MIN_INTERVAL_MS = 500

/** How many top processes to return per sample */
export const SERVER_MONITOR_TOP_PROCESS_COUNT = 24

/** Bytes in one kibibyte (ps rss /proc/meminfo units) */
export const BYTES_PER_KIB = 1024

/** Bits per byte (link speed -> byte rate) */
export const BITS_PER_BYTE = 8

/** sysfs net speed file unit (decimal megabits) */
export const SERVER_MONITOR_MEGABIT_BITS = 1_000_000

/** Linux /proc/diskstats sector size */
export const SERVER_MONITOR_DISK_SECTOR_BYTES = 512

/** /sys thermal zone temp units -> °C */
export const SERVER_MONITOR_TEMP_MILLI_PER_C = 1000

/** Loopback iface excluded from network table */
export const SERVER_MONITOR_LOOPBACK_IFACE = 'lo'

/** Visibility defaults for monitor panel sections */
export const SERVER_MONITOR_SHOW_GAUGES_DEFAULT = true
export const SERVER_MONITOR_SHOW_SPARKS_DEFAULT = true
export const SERVER_MONITOR_SHOW_STATUS_DEFAULT = true
export const SERVER_MONITOR_SHOW_PROCESSES_DEFAULT = true
export const SERVER_MONITOR_SHOW_NETWORK_DEFAULT = true
