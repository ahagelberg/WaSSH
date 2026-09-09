import { randomUUID } from 'crypto'
import type { PluginMainContext, PluginMainModule } from '../PluginHost'
import type {
  ConnectionLogEntry,
  ConnectionLogEventKind,
  ConnectionLoggerData,
  ConnectionLoggerRendererMessage
} from '../../../shared/plugins'
import type { SessionStatus } from '../../../shared/types'
import {
  PLUGIN_ID_CONNECTION_LOGGER,
  CONNECTION_LOGGER_DEFAULT_MAX_ENTRIES
} from '../../../shared/plugins'
import type { PluginDataStore } from '../../store/pluginDataStore'

export function connectionLoggerScopeId(
  hostId: string | null | undefined,
  tabId: string
): string {
  return hostId || `tab:${tabId}`
}

export function loadConnectionLogs(
  pluginData: PluginDataStore,
  scopeId: string
): ConnectionLogEntry[] {
  const raw = pluginData.get(PLUGIN_ID_CONNECTION_LOGGER, scopeId)
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return []
  }
  const data = raw as ConnectionLoggerData
  return Array.isArray(data.events) ? data.events : []
}

export function saveConnectionLogs(
  pluginData: PluginDataStore,
  scopeId: string,
  events: ConnectionLogEntry[]
): void {
  const data: ConnectionLoggerData = {
    version: 1,
    hostKey: scopeId,
    events
  }
  pluginData.set(PLUGIN_ID_CONNECTION_LOGGER, data, scopeId)
}

export function appendConnectionLog(
  pluginData: PluginDataStore,
  scopeId: string,
  entry: Omit<ConnectionLogEntry, 'id'>,
  maxEntries = CONNECTION_LOGGER_DEFAULT_MAX_ENTRIES
): ConnectionLogEntry {
  const current = loadConnectionLogs(pluginData, scopeId)
  const fullEntry: ConnectionLogEntry = {
    id: randomUUID(),
    ...entry
  }
  // Store chronological (newest at the end)
  const updated = [...current, fullEntry].slice(-maxEntries)
  saveConnectionLogs(pluginData, scopeId, updated)
  return fullEntry
}

export function clearConnectionLogs(
  pluginData: PluginDataStore,
  scopeId: string
): void {
  saveConnectionLogs(pluginData, scopeId, [])
}

export const connectionLoggerMain: PluginMainModule = {
  onActivate(ctx: PluginMainContext) {
    const raw = ctx.getData()
    const events =
      raw && typeof raw === 'object' && !Array.isArray(raw) && Array.isArray((raw as ConnectionLoggerData).events)
        ? (raw as ConnectionLoggerData).events
        : []

    ctx.sendToRenderer({
      type: 'state',
      events
    })
  },

  onMessage(ctx: PluginMainContext, payload: unknown) {
    if (!payload || typeof payload !== 'object') {
      return
    }
    const msg = payload as ConnectionLoggerRendererMessage
    if (msg.type === 'sync') {
      const raw = ctx.getData()
      const events =
        raw && typeof raw === 'object' && !Array.isArray(raw) && Array.isArray((raw as ConnectionLoggerData).events)
          ? (raw as ConnectionLoggerData).events
          : []
      ctx.sendToRenderer({
        type: 'state',
        events
      })
      return { success: true }
    }

    if (msg.type === 'clearLogs') {
      ctx.setData({
        version: 1,
        hostKey: msg.scopeId,
        events: []
      })
      ctx.sendToRenderer({
        type: 'state',
        events: []
      })
      return { success: true }
    }

    return undefined
  }
}
