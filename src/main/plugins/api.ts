import type { Duplex } from 'stream'
import type {
  PluginApiListing,
  SideConnectionOpenRequest,
  StreamDirection,
  StreamMode
} from '../../shared/pluginApi'
export type { PluginApiListing } from '../../shared/pluginApi'
import type { SessionStatus } from '../../shared/types'
export type { SessionStatus } from '../../shared/types'
import type { SftpSession } from './SftpSession'
import type { StreamTransform } from './types'
export { classifySftpError, joinRemotePath } from './SftpSession'
export type { SftpError, SftpSession } from './SftpSession'

export interface PluginSessionStatusEvent {
  status: SessionStatus
  previousStatus?: SessionStatus
  message?: string
  timestamp: number
  previousDurationMs?: number
}

export interface PluginMainContext {
  readonly tabId: string
  readonly pluginId: string
  getSettings: () => Record<string, unknown>
  /** Read this plugin's JSON data, optionally scoped to a host/session key. */
  getData: (scopeId?: string) => unknown
  /** Write this plugin's JSON data, optionally scoped to a host/session key. */
  setData: (data: unknown, scopeId?: string) => void
  /** Saved host id, or a stable tab-local scope for unsaved sessions. */
  getSessionScopeId: () => string
  /** Read a vault secret (DPAPI/safeStorage encrypted); null when absent. */
  getSecret: (vaultId: string) => string | null
  sendToRenderer: (payload: unknown) => void
  openSideConnection: (req: SideConnectionOpenRequest) => Promise<string>
  closeSideConnection: (connectionId: string) => void
  writeSideConnection: (connectionId: string, data: string) => void
  onSideData: (connectionId: string, cb: (data: string) => void) => () => void
  onSideClosed: (connectionId: string, cb: (error?: string) => void) => () => void
  isSshSession: () => boolean
  openTcpStream: (host: string, port: number) => Promise<Duplex>
  openSftp: () => Promise<SftpSession>
  execCapture: (command: string) => Promise<string>
  registerStreamHandler: (
    mode: StreamMode,
    direction: StreamDirection,
    handler: StreamTransform
  ) => void
  onDeactivateCleanup: (fn: () => void) => void
  writeToSession: (data: string) => void
  /** Declared API methods of every other plugin currently active on this tab. */
  listPluginApis: () => PluginApiListing[]
  /**
   * Call another plugin's declared API method on this same tab. Throws if the
   * target plugin has no active instance on this tab, or doesn't declare
   * `method`. Callers are responsible for their own permission checks -
   * this performs no gating and does not auto-activate the target plugin.
   */
  callPluginApi: (pluginId: string, method: string, params: unknown) => Promise<unknown>
  /** Persist one key of this plugin's own app-wide stored settings. */
  setSettingValue: (key: string, value: unknown) => void
}

export interface PluginMainModule {
  onActivate: (ctx: PluginMainContext) => void | Promise<void>
  onDeactivate?: (ctx: PluginMainContext) => void | Promise<void>
  onMessage?: (ctx: PluginMainContext, payload: unknown) => unknown | Promise<unknown>
  onSessionStatus?: (
    ctx: PluginMainContext,
    event: PluginSessionStatusEvent
  ) => void | Promise<void>
  /** Handle a `ctx.callPluginApi` invocation from another plugin (or self). */
  onApiCall?: (ctx: PluginMainContext, method: string, params: unknown) => unknown | Promise<unknown>
}

export interface PluginMainRegistration {
  id: string
  module: PluginMainModule
  /** Start without opening a view whenever a matching session connects. */
  backgroundActivation?: 'session' | 'ssh'
  /** Keep a background instance alive when its final view is closed. */
  retainBackgroundOnViewClose?: boolean
}
