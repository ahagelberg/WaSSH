import type { SessionStatus } from '@plugin-api/shared'
import { CONNECTION_LOGGER_DATA_VERSION } from './defaults'

export type ConnectionLogEventKind = 'connected' | 'disconnected' | 'reconnecting' | 'failed'

export interface ConnectionLogEntry {
  id: string
  timestamp: number
  kind: ConnectionLogEventKind
  message?: string
  durationMs?: number
}

export interface ConnectionLoggerData {
  version: number
  hostKey: string
  events: ConnectionLogEntry[]
}

export type ConnectionLoggerGraphMode = 'timeline' | 'step' | 'heatmap'

export type ConnectionLoggerTimeWindow = '1h' | '6h' | '24h' | '7d' | 'all'

export type ConnectionLoggerRendererMessage =
  | { type: 'sync' }
  | { type: 'clearLogs' }

export type ConnectionLoggerMainMessage =
  | { type: 'state'; scopeId: string; events: ConnectionLogEntry[] }
  | { type: 'event'; scopeId: string; entry: ConnectionLogEntry; currentStatus: SessionStatus }

export function connectionLoggerScopeId(
  hostId: string | null | undefined,
  tabId: string
): string {
  return hostId || `tab:${tabId}`
}

export function isConnectionLoggerData(value: unknown): value is ConnectionLoggerData {
  return (
    !!value &&
    typeof value === 'object' &&
    !Array.isArray(value) &&
    Array.isArray((value as { events?: unknown }).events)
  )
}

export function createConnectionLoggerData(
  hostKey: string,
  events: ConnectionLogEntry[]
): ConnectionLoggerData {
  return {
    version: CONNECTION_LOGGER_DATA_VERSION,
    hostKey,
    events
  }
}
