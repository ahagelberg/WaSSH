import { BrowserWindow } from 'electron'
import type { SettingsStore, SessionStore } from '../store/sessionStore'
import type { PluginDataStore } from '../store/pluginDataStore'
import type { CredentialVault } from '../store/credentialVault'
import type { SessionManager } from '../ssh/SessionManager'
import { PluginHost } from './PluginHost'
import { SessionDataPipeline } from './SessionDataPipeline'
import { SideConnectionBroker } from './SideConnectionBroker'

export interface PluginSystem {
  host: PluginHost
  pipeline: SessionDataPipeline
  broker: SideConnectionBroker
  dispose: () => void
}

/** Pending restore ids keyed by tab — applied on next connected status */
const pendingRestore = new Map<string, string[]>()

export function createPluginSystem(
  settingsStore: SettingsStore,
  sessionStore: SessionStore,
  sessions: SessionManager,
  getWindow: () => BrowserWindow | null,
  pluginData: PluginDataStore,
  vault: CredentialVault
): PluginSystem {
  const pipeline = new SessionDataPipeline()
  sessions.setPipeline(pipeline)

  let hostRef: PluginHost | null = null

  const broker = new SideConnectionBroker(
    (tabId) => sessions.getPluginSessionHandle(tabId),
    getWindow,
    (connectionId, data) => hostRef?.notifySideData(connectionId, data),
    (connectionId, error) => hostRef?.notifySideClosed(connectionId, error)
  )

  const host = new PluginHost(
    settingsStore,
    sessionStore,
    pipeline,
    broker,
    (tabId, data) => sessions.writeRaw(tabId, data),
    getWindow,
    pluginData,
    vault
  )
  host.registerBuiltins()
  hostRef = host

  sessions.setPluginHooks({
    onStatusConnected: (tabId) => {
      const restore = pendingRestore.get(tabId)
      pendingRestore.delete(tabId)
      void host.onSessionConnected(tabId, restore)
    },
    onSessionRemoved: (tabId) => {
      void host.deactivateAll(tabId)
    }
  })

  return {
    host,
    pipeline,
    broker,
    dispose: () => {
      host.dispose()
      broker.disposeAll()
    }
  }
}

export function queuePluginRestore(tabId: string, activePluginIds: string[]): void {
  pendingRestore.set(tabId, activePluginIds)
}
