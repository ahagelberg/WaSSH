import type { MonitorSeries } from '@shared/monitorSeries'

/** Lifecycle of the remote daemon ("WaSSH Service") on the host. */
export type MonitorServiceState =
  | 'probing'
  | 'missing'
  | 'outdated'
  | 'ready'
  | 'installing'
  | 'uninstalling'
  | 'error'

export interface MonitorServiceStatus {
  state: MonitorServiceState
  version?: string
  message?: string
  streamConnected: boolean
}

export const MONITOR_SERVICE_INITIAL_STATUS: MonitorServiceStatus = {
  state: 'probing',
  streamConnected: false
}

/** Daemon stream opened and described its series set. */
export interface MonitorHistoryReadyEvent {
  type: 'historyReady'
  series: MonitorSeries[]
  temperatureZones: string[]
  interfaces: string[]
}

/** One aggregated bucket pushed to the view. */
export interface MonitorHistoryBucketEvent {
  type: 'history'
  /** Request that produced this bucket; the view drops superseded responses */
  requestId: number
  bucketSeconds: number
  seriesId: string
  timestamp: number
  min: number
  max: number
  avg: number
}

export type MonitorMainMessage =
  | { type: 'service'; status: MonitorServiceStatus }
  | MonitorHistoryReadyEvent
  | MonitorHistoryBucketEvent

export type MonitorRendererMessage =
  | { type: 'probeService' }
  | { type: 'installService'; password: string }
  | { type: 'uninstallService'; password: string }
  | { type: 'requestHistory'; requestId: number; bucketSeconds: number; fromMs: number; toMs: number }

export interface MonitorActionResult {
  ok: boolean
  error?: string
}

export function isMonitorRendererMessage(value: unknown): value is MonitorRendererMessage {
  if (!value || typeof value !== 'object') {
    return false
  }
  const message = value as Record<string, unknown>
  if (message.type === 'probeService') {
    return true
  }
  if (message.type === 'installService' || message.type === 'uninstallService') {
    return typeof message.password === 'string'
  }
  return (
    message.type === 'requestHistory' &&
    typeof message.requestId === 'number' &&
    typeof message.bucketSeconds === 'number' &&
    typeof message.fromMs === 'number' &&
    typeof message.toMs === 'number'
  )
}

export function isMonitorActionResult(value: unknown): value is MonitorActionResult {
  return (
    !!value && typeof value === 'object' && typeof (value as MonitorActionResult).ok === 'boolean'
  )
}

export function isMonitorMainMessage(value: unknown): value is MonitorMainMessage {
  if (!value || typeof value !== 'object') {
    return false
  }
  const type = (value as { type?: unknown }).type
  return type === 'service' || type === 'historyReady' || type === 'history'
}
