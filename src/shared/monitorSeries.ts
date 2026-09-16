/**
 * Series identity for the monitor history ladder. Ids are deterministic so the
 * renderer can switch between the daemon-provided ladder and the app-side
 * ladder without a visual jump.
 */

/** How a series should be scaled and drawn. */
export type MonitorSeriesKind = 'gauge' | 'rate' | 'state'

export interface MonitorSeries {
  id: string
  label: string
  /** Display unit; empty for percentages and states. */
  unit: MonitorSeriesUnit
  kind: MonitorSeriesKind
  /**
   * Fixed 0..max scale for `gauge`/`state` series. `null` means "derive the
   * scale from the data" (rates, load).
   */
  max: number | null
}

export type MonitorSeriesUnit = '' | '%' | 'B' | 'B/s' | '°C' | 'count'

export const SERIES_CPU = 'cpu'
export const SERIES_MEM_USED = 'mem:used'
export const SERIES_MEM_TOTAL = 'mem:total'
export const SERIES_DISK_USED = 'disk:used'
export const SERIES_DISK_TOTAL = 'disk:total'
export const SERIES_DISK_READ = 'diskio:read'
export const SERIES_DISK_WRITE = 'diskio:write'

/** Percent scale shared by the CPU / memory / disk gauges. */
export const SERIES_PERCENT_MAX = 100

export function tempSeriesId(zone: string): string {
  return `temp:${zone}`
}

export function netSeriesId(iface: string, direction: 'rx' | 'tx'): string {
  return `net:${iface}:${direction}`
}

export function ifaceStateSeriesId(iface: string): string {
  return `iface:${iface}:state`
}

export function isTempSeriesId(id: string): boolean {
  return id.startsWith('temp:')
}

export function isNetSeriesId(id: string): boolean {
  return id.startsWith('net:')
}

export function isIfaceStateSeriesId(id: string): boolean {
  return id.startsWith('iface:') && id.endsWith(':state')
}

/** Interface name encoded in a `net:<iface>:<direction>` id, or null. */
export function netSeriesIface(id: string): string | null {
  if (!isNetSeriesId(id)) {
    return null
  }
  const parts = id.split(':')
  return parts.length === 3 ? parts[1] : null
}

/** Interface name encoded in an `iface:<name>:state` id, or null. */
export function ifaceStateSeriesIface(id: string): string | null {
  if (!isIfaceStateSeriesId(id)) {
    return null
  }
  const parts = id.split(':')
  return parts.length === 3 ? parts[1] : null
}

/** Thermal zone name encoded in a `temp:<zone>` id, or null. */
export function tempSeriesZone(id: string): string | null {
  return isTempSeriesId(id) ? id.slice('temp:'.length) : null
}

export function gaugeSeries(id: string, label: string, max: number | null): MonitorSeries {
  return { id, label, unit: max === null ? '' : '%', kind: 'gauge', max }
}

export function rateSeries(id: string, label: string): MonitorSeries {
  return { id, label, unit: 'B/s', kind: 'rate', max: null }
}

export function stateSeries(id: string, label: string): MonitorSeries {
  return { id, label, unit: '', kind: 'state', max: 1 }
}

/** Bytes in one kibibyte (`/proc/meminfo` and `df -Pk` units). */
export const BYTES_PER_KIB = 1024

/** Centi-degrees Celsius per degree (wire format for temperatures). */
export const CENTI_PER_C = 100
