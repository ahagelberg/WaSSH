import { BrowserWindow } from 'electron'
import type { Duplex } from 'stream'
import type {
  PluginActiveStateEvent,
  PluginListItem,
  PluginManifest,
  PluginMessageEvent,
  SideConnectionOpenRequest,
  StreamDirection,
  StreamMode
} from '../../shared/plugins'
import { mergePluginSessionSettings, PLUGIN_ID_MACRO_PAD, PLUGIN_ID_MQTT_EXPLORER, PLUGIN_ID_SCRATCHPAD, PLUGIN_ID_SERVER_MONITOR } from '../../shared/plugins'
import type { SettingsStore, SessionStore } from '../store/sessionStore'
import type { PluginDataStore } from '../store/pluginDataStore'
import { BUILTIN_MANIFESTS } from './builtins/manifests'
import { macroPadMain } from './builtins/macroPad'
import { mqttExplorerMain } from './builtins/mqttExplorer'
import { scratchpadMain } from './builtins/scratchpad'
import { serverMonitorMain } from './builtins/serverMonitor'
import { loadExternalPlugins } from './externalLoader'
import type { SessionDataPipeline } from './SessionDataPipeline'
import type { SideConnectionBroker } from './SideConnectionBroker'
import type { StreamTransform } from './types'

export interface PluginMainContext {
  tabId: string
  pluginId: string
  getSettings: () => Record<string, unknown>
  /** Read this plugin's JSON file from userData */
  getData: () => unknown
  /** Write this plugin's JSON file under userData */
  setData: (data: unknown) => void
  sendToRenderer: (payload: unknown) => void
  openSideConnection: (req: SideConnectionOpenRequest) => Promise<string>
  closeSideConnection: (connectionId: string) => void
  writeSideConnection: (connectionId: string, data: string) => void
  onSideData: (connectionId: string, cb: (data: string) => void) => () => void
  onSideClosed: (connectionId: string, cb: (error?: string) => void) => () => void
  /** Whether the tab's live session is SSH (required for remote MQTT tunnel). */
  isSshSession: () => boolean
  /**
   * Binary TCP duplex via SSH forwardOut (or direct TCP).
   * Not UTF-8 side-data — for protocols like MQTT.
   */
  openTcpStream: (host: string, port: number) => Promise<Duplex>
  registerStreamHandler: (
    mode: StreamMode,
    direction: StreamDirection,
    handler: StreamTransform
  ) => void
  /** Register cleanup run on deactivate (in addition to onDeactivate) */
  onDeactivateCleanup: (fn: () => void) => void
  writeToSession: (data: string) => void
}

export interface PluginMainModule {
  onActivate: (ctx: PluginMainContext) => void | Promise<void>
  onDeactivate?: (ctx: PluginMainContext) => void | Promise<void>
  onMessage?: (ctx: PluginMainContext, payload: unknown) => void
}

interface ActiveInstance {
  pluginId: string
  tabId: string
  cleanups: Array<() => void>
  ctx: PluginMainContext
  module: PluginMainModule
}

export class PluginHost {
  private instances = new Map<string, Map<string, ActiveInstance>>()
  private sideDataListeners = new Map<string, Set<(data: string) => void>>()
  private sideClosedListeners = new Map<string, Set<(error?: string) => void>>()
  private modules = new Map<string, PluginMainModule>()

  constructor(
    private settingsStore: SettingsStore,
    private sessionStore: SessionStore,
    private pipeline: SessionDataPipeline,
    private broker: SideConnectionBroker,
    private writeSession: (tabId: string, data: string) => void,
    private getWindow: () => BrowserWindow | null,
    private pluginData: PluginDataStore
  ) {}

  /** Call after construction once builtin modules are registered */
  registerBuiltins(): void {
    this.modules.set(PLUGIN_ID_SERVER_MONITOR, serverMonitorMain)
    this.modules.set(PLUGIN_ID_SCRATCHPAD, scratchpadMain)
    this.modules.set(PLUGIN_ID_MACRO_PAD, macroPadMain)
    this.modules.set(PLUGIN_ID_MQTT_EXPLORER, mqttExplorerMain)
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

  async activate(tabId: string, pluginId: string): Promise<void> {
    if (!this.isEnabled(pluginId)) {
      throw new Error(`Plugin "${pluginId}" is not enabled`)
    }
    const mod = this.modules.get(pluginId)
    if (!mod) {
      throw new Error(`Plugin "${pluginId}" has no main module`)
    }
    let tabMap = this.instances.get(tabId)
    if (!tabMap) {
      tabMap = new Map()
      this.instances.set(tabId, tabMap)
    }
    if (tabMap.has(pluginId)) {
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
      getData: () => this.pluginData.get(pluginId),
      setData: (data: unknown) => {
        this.pluginData.set(pluginId, data)
      },
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

    const instance: ActiveInstance = { pluginId, tabId, cleanups, ctx, module: mod }
    tabMap.set(pluginId, instance)
    await mod.onActivate(ctx)
    this.send('plugin:active', {
      tabId,
      pluginId,
      active: true
    } satisfies PluginActiveStateEvent)
  }

  async deactivate(tabId: string, pluginId: string): Promise<void> {
    const tabMap = this.instances.get(tabId)
    const instance = tabMap?.get(pluginId)
    if (!instance) {
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
      await this.deactivate(tabId, id)
    }
    this.pipeline.clearTab(tabId)
    this.broker.closeForTab(tabId)
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
  }

  async handleRendererMessage(tabId: string, pluginId: string, payload: unknown): Promise<void> {
    const instance = this.instances.get(tabId)?.get(pluginId)
    if (!instance) {
      return
    }
    instance.module.onMessage?.(instance.ctx, payload)
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
        await this.deactivate(tabId, pluginId)
      }
    }
  }

  dispose(): void {
    for (const tabId of Array.from(this.instances.keys())) {
      void this.deactivateAll(tabId)
    }
  }
}
