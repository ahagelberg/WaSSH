import type { Duplex } from 'stream'
import type {
  SideConnectionOpenRequest,
  StreamDirection,
  StreamMode
} from '../../shared/pluginApi'
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
}

export interface PluginMainModule {
  onActivate: (ctx: PluginMainContext) => void | Promise<void>
  onDeactivate?: (ctx: PluginMainContext) => void | Promise<void>
  onMessage?: (ctx: PluginMainContext, payload: unknown) => unknown | Promise<unknown>
  onSessionStatus?: (
    ctx: PluginMainContext,
    event: PluginSessionStatusEvent
  ) => void | Promise<void>
}

export interface PluginMainRegistration {
  id: string
  module: PluginMainModule
  /** Start without opening a view whenever a matching session connects. */
  backgroundActivation?: 'session' | 'ssh'
  /** Keep a background instance alive when its final view is closed. */
  retainBackgroundOnViewClose?: boolean
}
