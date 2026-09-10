import { BrowserWindow } from 'electron'
import type {
  PluginActiveStateEvent,
  PluginListItem,
  PluginManifest,
  PluginMessageEvent
} from '../../shared/pluginApi'
import { mergePluginSessionSettings } from '../../shared/pluginApi'
import type { SessionStatus } from '../../shared/types'
import type { SettingsStore, SessionStore } from '../store/sessionStore'
import type { PluginDataStore } from '../store/pluginDataStore'
import type { CredentialVault } from '../store/credentialVault'
import { loadExternalPlugins } from './externalLoader'
import { BUILTIN_MANIFESTS, BUILTIN_PLUGIN_DEFINITIONS } from './builtinRegistry'
import type { SessionDataPipeline } from './SessionDataPipeline'
import type { SideConnectionBroker } from './SideConnectionBroker'
import type {
  PluginMainContext,
  PluginMainModule,
  PluginMainRegistration,
  PluginSessionStatusEvent
} from './api'
export type { PluginMainContext, PluginMainModule } from './api'

interface ActiveInstance {
  pluginId: string
  tabId: string
  cleanups: Array<() => void>
  ctx: PluginMainContext
  module: PluginMainModule
  /** Whether the renderer was told this plugin is active (i.e. a view is docked). */
  announced: boolean
}

export class PluginHost {
  private instances = new Map<string, Map<string, ActiveInstance>>()
  private sideDataListeners = new Map<string, Set<(data: string) => void>>()
  private sideClosedListeners = new Map<string, Set<(error?: string) => void>>()
  private registrations = new Map<string, PluginMainRegistration>()
  private tabStatus = new Map<string, { status: SessionStatus; at: number }>()

  constructor(
    private settingsStore: SettingsStore,
    private sessionStore: SessionStore,
    private pipeline: SessionDataPipeline,
    private broker: SideConnectionBroker,
    private writeSession: (tabId: string, data: string) => void,
    private getWindow: () => BrowserWindow | null,
    private pluginData: PluginDataStore,
    private vault: CredentialVault
  ) {}

  /** Call after construction once builtin modules are registered */
  registerBuiltins(): void {
    this.registrations = new Map(
      BUILTIN_PLUGIN_DEFINITIONS.map(({ registration }) => [registration.id, registration])
    )
  }

  private send(channel: string, payload: unknown): void {
    const win = this.getWindow()
    if (!win || win.isDestroyed()) {
      return
    }
    win.webContents.send(channel, payload)
  }

  listPlugins(): PluginListItem[] {
    const enabled = new Set(this.settingsStore.get().enabledPlugins)
    const external = loadExternalPlugins()
    return [...BUILTIN_MANIFESTS, ...external].map((m) => ({
      ...m,
      enabled: enabled.has(m.id)
    }))
  }

  getManifest(pluginId: string): PluginManifest | undefined {
    return this.listPlugins().find((p) => p.id === pluginId)
  }

  /**
   * Host-scoped settings for a plugin: prefer saved HostProfile (fresh after save),
   * else the live connection snapshot (quick-connect / unsaved).
   */
  private resolveHostPluginSettings(
    tabId: string,
    pluginId: string
  ): Record<string, unknown> | undefined {
    const connection = this.broker.getConnectionParams(tabId)
    if (!connection) {
      return undefined
    }
    if (connection.hostId) {
      const fromHost = this.sessionStore.getHost(connection.hostId)?.pluginSettings?.[pluginId]
      if (fromHost) {
        return fromHost
      }
    }
    return connection.pluginSettings?.[pluginId]
  }

  isEnabled(pluginId: string): boolean {
    return this.settingsStore.get().enabledPlugins.includes(pluginId)
  }

  getActivePlugins(tabId: string): string[] {
    const map = this.instances.get(tabId)
    return map ? Array.from(map.keys()) : []
  }

  async activate(tabId: string, pluginId: string, announce = true): Promise<void> {
    if (!this.isEnabled(pluginId)) {
      throw new Error(`Plugin "${pluginId}" is not enabled`)
    }
    const mod = this.registrations.get(pluginId)?.module
    if (!mod) {
      throw new Error(`Plugin "${pluginId}" has no main module`)
    }
    let tabMap = this.instances.get(tabId)
    if (!tabMap) {
      tabMap = new Map()
      this.instances.set(tabId, tabMap)
    }
    const existing = tabMap.get(pluginId)
    if (existing) {
      // Promote a headless instance to a visible one (dock its view) on demand.
      if (announce && !existing.announced) {
        existing.announced = true
        this.send('plugin:active', {
          tabId,
          pluginId,
          active: true
        } satisfies PluginActiveStateEvent)
      }
      return
    }

    const cleanups: Array<() => void> = []
    const ctx: PluginMainContext = {
      tabId,
      pluginId,
      getSettings: () => {
        const manifest = this.getManifest(pluginId)
        const appStored = this.settingsStore.get().pluginSettings[pluginId]
        const hostStored = this.resolveHostPluginSettings(tabId, pluginId)
        return mergePluginSessionSettings(manifest, appStored, hostStored)
      },
      getData: (scopeId) => {
        return this.pluginData.get(pluginId, scopeId)
      },
      setData: (data: unknown, scopeId) => {
        this.pluginData.set(pluginId, data, scopeId)
      },
      getSessionScopeId: () => this.broker.getConnectionParams(tabId)?.hostId || `tab:${tabId}`,
      getSecret: (vaultId: string) => this.vault.get(vaultId),
      sendToRenderer: (payload: unknown) => {
        this.send('plugin:message', {
          tabId,
          pluginId,
          payload
        } satisfies PluginMessageEvent)
      },
      openSideConnection: async (req) => {
        const id = await this.broker.open(tabId, pluginId, req)
        return id
      },
      closeSideConnection: (connectionId) => {
        this.broker.close(connectionId)
      },
      writeSideConnection: (connectionId, data) => {
        this.broker.write(connectionId, data)
      },
      onSideData: (connectionId, cb) => {
        let set = this.sideDataListeners.get(connectionId)
        if (!set) {
          set = new Set()
          this.sideDataListeners.set(connectionId, set)
        }
        set.add(cb)
        return () => {
          set?.delete(cb)
        }
      },
      onSideClosed: (connectionId, cb) => {
        let set = this.sideClosedListeners.get(connectionId)
        if (!set) {
          set = new Set()
          this.sideClosedListeners.set(connectionId, set)
        }
        set.add(cb)
        return () => {
          set?.delete(cb)
        }
      },
      isSshSession: () => this.broker.isSshSession(tabId),
      openTcpStream: async (host, port) => {
        const { stream } = await this.broker.openTcpStream(tabId, pluginId, host, port)
        return stream
      },
      openSftp: () => this.broker.openSftp(tabId, pluginId),
      execCapture: (command) => this.broker.execCapture(tabId, command),
      registerStreamHandler: (mode, direction, handler) => {
        this.pipeline.register(tabId, { pluginId, mode, direction, handler })
        cleanups.push(() => this.pipeline.unregisterPlugin(tabId, pluginId))
      },
      onDeactivateCleanup: (fn) => {
        cleanups.push(fn)
      },
      writeToSession: (data) => {
        this.writeSession(tabId, data)
      }
    }

    const instance: ActiveInstance = {
      pluginId,
      tabId,
      cleanups,
      ctx,
      module: mod,
      announced: announce
    }
    tabMap.set(pluginId, instance)
    await mod.onActivate(ctx)
    if (announce) {
      this.send('plugin:active', {
        tabId,
        pluginId,
        active: true
      } satisfies PluginActiveStateEvent)
    }
  }

  async deactivate(tabId: string, pluginId: string, force = false): Promise<void> {
    const tabMap = this.instances.get(tabId)
    const instance = tabMap?.get(pluginId)
    if (!instance) {
      return
    }
    // Closing the SFTP Files browser must not disable terminal drop-upload:
    // keep the module running headless while the session is still SSH. The
    // renderer already removed the plugin from its view state locally.
    const registration = this.registrations.get(pluginId)
    const retainBackground =
      registration?.retainBackgroundOnViewClose &&
      (registration.backgroundActivation === 'session' ||
        (registration.backgroundActivation === 'ssh' && this.broker.isSshSession(tabId)))
    if (!force && retainBackground) {
      instance.announced = false
      return
    }
    tabMap?.delete(pluginId)
    if (tabMap && tabMap.size === 0) {
      this.instances.delete(tabId)
    }
    try {
      await instance.module.onDeactivate?.(instance.ctx)
    } catch {
      /* ignore */
    }
    for (const fn of instance.cleanups) {
      try {
        fn()
      } catch {
        /* ignore */
      }
    }
    this.pipeline.unregisterPlugin(tabId, pluginId)
    this.broker.closeForPlugin(tabId, pluginId)
    this.send('plugin:active', {
      tabId,
      pluginId,
      active: false
    } satisfies PluginActiveStateEvent)
  }

  async deactivateAll(tabId: string): Promise<void> {
    const ids = this.getActivePlugins(tabId)
    for (const id of ids) {
      await this.deactivate(tabId, id, true)
    }
    this.pipeline.clearTab(tabId)
    this.broker.closeForTab(tabId)
    this.tabStatus.delete(tabId)
  }

  onSessionStatus(tabId: string, status: SessionStatus, message?: string): void {
    const prev = this.tabStatus.get(tabId)
    const now = Date.now()
    const durationMs = prev ? Math.max(0, now - prev.at) : undefined
    this.tabStatus.set(tabId, { status, at: now })

    const statusEvent: PluginSessionStatusEvent = {
      status,
      previousStatus: prev?.status,
      message,
      timestamp: now,
      previousDurationMs: durationMs
    }
    for (const instance of this.instances.get(tabId)?.values() ?? []) {
      const result = instance.module.onSessionStatus?.(instance.ctx, statusEvent)
      if (result instanceof Promise) {
        void result.catch((error) => {
          console.error(`Plugin ${instance.pluginId} session-status hook failed:`, error)
        })
      }
    }

  }

  async onSessionConnected(tabId: string, restoreIds?: string[]): Promise<void> {
    const enabled = this.listPlugins().filter((p) => p.enabled)
    const toActivate = new Set<string>()
    for (const p of enabled) {
      if (p.activation === 'auto') {
        toActivate.add(p.id)
      }
    }
    if (restoreIds) {
      for (const id of restoreIds) {
        if (enabled.some((p) => p.id === id)) {
          toActivate.add(id)
        }
      }
    }
    for (const id of toActivate) {
      try {
        await this.activate(tabId, id)
      } catch (err) {
        console.error(`Failed to activate plugin ${id}:`, err)
      }
    }

    for (const plugin of enabled) {
      const registration = this.registrations.get(plugin.id)
      const shouldRunInBackground =
        registration?.backgroundActivation === 'session' ||
        (registration?.backgroundActivation === 'ssh' && this.broker.isSshSession(tabId))
      if (!shouldRunInBackground || this.instances.get(tabId)?.has(plugin.id)) {
        continue
      }
      try {
        await this.activate(tabId, plugin.id, false)
      } catch (err) {
        console.error(`Failed to activate background plugin ${plugin.id}:`, err)
      }
    }
  }

  async handleRendererMessage(tabId: string, pluginId: string, payload: unknown): Promise<unknown> {
    const instance = this.instances.get(tabId)?.get(pluginId)
    if (!instance) {
      return undefined
    }
    return instance.module.onMessage?.(instance.ctx, payload)
  }

  /** Forward broker side-data into plugin listeners (and renderer already gets IPC). */
  notifySideData(connectionId: string, data: string): void {
    const set = this.sideDataListeners.get(connectionId)
    if (!set) {
      return
    }
    for (const cb of set) {
      try {
        cb(data)
      } catch {
        /* ignore */
      }
    }
  }

  notifySideClosed(connectionId: string, error?: string): void {
    const set = this.sideClosedListeners.get(connectionId)
    this.sideDataListeners.delete(connectionId)
    this.sideClosedListeners.delete(connectionId)
    if (!set) {
      return
    }
    for (const cb of set) {
      try {
        cb(error)
      } catch {
        /* ignore */
      }
    }
  }

  async onEnabledPluginsChanged(previous: string[], next: string[]): Promise<void> {
    const removed = previous.filter((id) => !next.includes(id))
    for (const pluginId of removed) {
      for (const tabId of Array.from(this.instances.keys())) {
        await this.deactivate(tabId, pluginId, true)
      }
    }
  }

  dispose(): void {
    for (const tabId of Array.from(this.instances.keys())) {
      void this.deactivateAll(tabId)
    }
  }
}
