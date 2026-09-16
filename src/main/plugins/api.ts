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
// Re-exported deliberately: plugins get the SFTP session type and the two
// shared error/path helpers through the API entry (`@plugin-api/main`) instead
// of importing `./SftpSession` internals directly.
export { classifySftpError, joinRemotePath } from './SftpSession'
export type { SftpError, SftpSession } from './SftpSession'

export interface PluginSessionStatusEvent {
  status: SessionStatus
  previousStatus?: SessionStatus
  message?: string
  timestamp: number
  previousDurationMs?: number
}

/** File-type filter for native open/save dialogs. */
export interface PluginFileDialogFilter {
  name: string
  extensions: string[]
}

/** Options for `ctx.showOpenDialog` (narrow subset of the Electron dialog options). */
export interface PluginOpenDialogOptions {
  title?: string
  buttonLabel?: string
  defaultPath?: string
  /** `openFile`/`openDirectory` select one; add `multiSelections` for many. */
  properties?: Array<'openFile' | 'openDirectory' | 'multiSelections'>
  filters?: PluginFileDialogFilter[]
}

/** Result of `ctx.showOpenDialog`; `filePaths` is empty when cancelled. */
export interface PluginOpenDialogResult {
  canceled: boolean
  filePaths: string[]
}

/** Options for `ctx.showSaveDialog` (narrow subset of the Electron dialog options). */
export interface PluginSaveDialogOptions {
  title?: string
  buttonLabel?: string
  defaultPath?: string
  filters?: PluginFileDialogFilter[]
}

/** Result of `ctx.showSaveDialog`; `filePath` is undefined when cancelled. */
export interface PluginSaveDialogResult {
  canceled: boolean
  filePath?: string
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
  /** Native open-file/directory dialog; returns the picked paths (empty when cancelled). */
  showOpenDialog: (options: PluginOpenDialogOptions) => Promise<PluginOpenDialogResult>
  /** Native save-file dialog; returns the picked path (`undefined` when cancelled). */
  showSaveDialog: (options: PluginSaveDialogOptions) => Promise<PluginSaveDialogResult>
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
