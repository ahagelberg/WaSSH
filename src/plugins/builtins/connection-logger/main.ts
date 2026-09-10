import { randomUUID } from 'crypto'
import type {
  PluginMainContext,
  PluginMainModule,
  PluginSessionStatusEvent
} from '@plugin-api/main'
import { CONNECTION_LOGGER_DATA_VERSION, CONNECTION_LOGGER_DEFAULT_MAX_ENTRIES } from './defaults'
import { PLUGIN_ID_CONNECTION_LOGGER } from './id'
import {
  connectionLoggerScopeId,
  createConnectionLoggerData,
  isConnectionLoggerData,
  type ConnectionLogEntry,
  type ConnectionLogEventKind,
  type ConnectionLoggerMainMessage,
  type ConnectionLoggerRendererMessage
} from './protocol'

interface PluginDataStore {
  get: (pluginId: string, scopeId?: string) => unknown
  set: (pluginId: string, value: unknown, scopeId?: string) => void
}

function readConnectionLogs(raw: unknown): ConnectionLogEntry[] {
  return isConnectionLoggerData(raw) ? raw.events : []
}

function writeConnectionLogs(
  setData: (data: unknown, scopeId?: string) => void,
  scopeId: string,
  events: ConnectionLogEntry[]
): void {
  setData(createConnectionLoggerData(scopeId, events), scopeId)
}

function isDuplicateEntry(
  current: ConnectionLogEntry[],
  entry: Omit<ConnectionLogEntry, 'id'>
): ConnectionLogEntry | null {
  const last = current[current.length - 1]
  if (
    last &&
    last.timestamp === entry.timestamp &&
    last.kind === entry.kind &&
    last.message === entry.message &&
    last.durationMs === entry.durationMs
  ) {
    return last
  }
  return null
}

function appendEntry(
  current: ConnectionLogEntry[],
  entry: Omit<ConnectionLogEntry, 'id'>,
  maxEntries: number
): { entry: ConnectionLogEntry; events: ConnectionLogEntry[]; changed: boolean } {
  const duplicate = isDuplicateEntry(current, entry)
  if (duplicate) {
    return { entry: duplicate, events: current, changed: false }
  }

  const fullEntry: ConnectionLogEntry = {
    id: randomUUID(),
    ...entry
  }
  return {
    entry: fullEntry,
    events: [...current, fullEntry].slice(-maxEntries),
    changed: true
  }
}

function currentMaxEntries(ctx: PluginMainContext): number {
  const configured = ctx.getSettings().maxEntries
  return typeof configured === 'number' && configured > 0
    ? configured
    : CONNECTION_LOGGER_DEFAULT_MAX_ENTRIES
}

function toRendererState(scopeId: string, events: ConnectionLogEntry[]): ConnectionLoggerMainMessage {
  return {
    type: 'state',
    scopeId,
    events
  }
}

function statusToLogKind(event: PluginSessionStatusEvent): ConnectionLogEventKind | null {
  if (event.status === 'connected') {
    return 'connected'
  }
  if (event.status === 'disconnected') {
    return 'disconnected'
  }
  if (event.status === 'reconnecting') {
    return 'reconnecting'
  }
  if (event.status === 'failed') {
    return 'failed'
  }
  if (event.status === 'closed' && event.previousStatus === 'connected') {
    return 'disconnected'
  }
  return null
}

function syncScopeToRenderer(ctx: PluginMainContext, scopeId: string): void {
  ctx.sendToRenderer(toRendererState(scopeId, readConnectionLogs(ctx.getData(scopeId))))
}

export function loadConnectionLogs(
  pluginData: PluginDataStore,
  scopeId: string
): ConnectionLogEntry[] {
  return readConnectionLogs(pluginData.get(PLUGIN_ID_CONNECTION_LOGGER, scopeId))
}

export function saveConnectionLogs(
  pluginData: PluginDataStore,
  scopeId: string,
  events: ConnectionLogEntry[]
): void {
  pluginData.set(
    PLUGIN_ID_CONNECTION_LOGGER,
    {
      version: CONNECTION_LOGGER_DATA_VERSION,
      hostKey: scopeId,
      events
    },
    scopeId
  )
}

export function appendConnectionLog(
  pluginData: PluginDataStore,
  scopeId: string,
  entry: Omit<ConnectionLogEntry, 'id'>,
  maxEntries = CONNECTION_LOGGER_DEFAULT_MAX_ENTRIES
): ConnectionLogEntry {
  const current = loadConnectionLogs(pluginData, scopeId)
  const next = appendEntry(current, entry, maxEntries)
  if (next.changed) {
    saveConnectionLogs(pluginData, scopeId, next.events)
  }
  return next.entry
}

export function clearConnectionLogs(
  pluginData: PluginDataStore,
  scopeId: string
): void {
  saveConnectionLogs(pluginData, scopeId, [])
}

export const connectionLoggerMain: PluginMainModule = {
  onActivate(ctx) {
    syncScopeToRenderer(ctx, ctx.getSessionScopeId())
  },

  onMessage(ctx, payload) {
    if (!payload || typeof payload !== 'object') {
      return
    }

    const msg = payload as ConnectionLoggerRendererMessage
    const scopeId = ctx.getSessionScopeId()

    if (msg.type === 'sync') {
      syncScopeToRenderer(ctx, scopeId)
      return { success: true }
    }

    if (msg.type === 'clearLogs') {
      writeConnectionLogs(ctx.setData, scopeId, [])
      ctx.sendToRenderer(toRendererState(scopeId, []))
      return { success: true }
    }

    return undefined
  },

  onSessionStatus(ctx, event) {
    if (ctx.getSettings().enabled === false) {
      return
    }

    const kind = statusToLogKind(event)
    if (!kind) {
      return
    }

    const scopeId = ctx.getSessionScopeId()
    const current = readConnectionLogs(ctx.getData(scopeId))
    const next = appendEntry(
      current,
      {
        timestamp: event.timestamp,
        kind,
        message: event.message || undefined,
        durationMs: event.previousDurationMs
      },
      currentMaxEntries(ctx)
    )

    if (next.changed) {
      writeConnectionLogs(ctx.setData, scopeId, next.events)
    }

    const rendererMessage: ConnectionLoggerMainMessage = {
      type: 'event',
      scopeId,
      entry: next.entry,
      currentStatus: event.status
    }
    ctx.sendToRenderer(rendererMessage)
  }
}
