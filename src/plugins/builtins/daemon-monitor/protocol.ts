import type { DaemonMonitorRange } from './defaults'

export type DaemonMonitorServiceState =
  | 'probing'
  | 'missing'
  | 'outdated'
  | 'ready'
  | 'installing'
  | 'uninstalling'
  | 'error'

export interface DaemonMonitorServiceStatus {
  state: DaemonMonitorServiceState
  version?: string
  message?: string
  streamConnected: boolean
}

export interface DaemonMonitorSample {
  timestamp: number
  cpuPercent: number
  memoryUsedBytes: number
  memoryTotalBytes: number
  diskUsedBytes: number
  diskTotalBytes: number
}

export type DaemonMonitorTargetKind = 'interface' | 'ping'
export type DaemonMonitorTargetState = 'up' | 'down'

export interface DaemonMonitorStateEvent {
  timestamp: number
  kind: DaemonMonitorTargetKind
  name: string
  state: DaemonMonitorTargetState
  transition: boolean
}

export type DaemonMonitorMainMessage =
  | { type: 'status'; status: DaemonMonitorServiceStatus }
  | {
      type: 'records'
      reset: boolean
      samples: DaemonMonitorSample[]
      events: DaemonMonitorStateEvent[]
    }

export type DaemonMonitorRendererMessage =
  | { type: 'probe' }
  | { type: 'install'; password: string }
  | { type: 'uninstall'; password: string }
  | { type: 'setRange'; range: DaemonMonitorRange }

export interface DaemonMonitorActionResult {
  ok: boolean
  error?: string
}

export function isDaemonMonitorRendererMessage(
  value: unknown
): value is DaemonMonitorRendererMessage {
  if (!value || typeof value !== 'object') {
    return false
  }
  const message = value as Record<string, unknown>
  if (message.type === 'probe') {
    return true
  }
  if (message.type === 'install' || message.type === 'uninstall') {
    return typeof message.password === 'string'
  }
  return message.type === 'setRange' && typeof message.range === 'string'
}

export function isDaemonMonitorMainMessage(value: unknown): value is DaemonMonitorMainMessage {
  if (!value || typeof value !== 'object') {
    return false
  }
  const message = value as Partial<DaemonMonitorMainMessage>
  return message.type === 'status' || message.type === 'records'
}

export function isDaemonMonitorActionResult(value: unknown): value is DaemonMonitorActionResult {
  return (
    !!value &&
    typeof value === 'object' &&
    typeof (value as Partial<DaemonMonitorActionResult>).ok === 'boolean'
  )
}
