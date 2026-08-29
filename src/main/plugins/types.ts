import type { Client, ClientChannel } from 'ssh2'
import { Socket } from 'net'
import type { ConnectionParams } from '../../shared/types'

/**
 * Capability surface SessionManager exposes to the plugin side-connection broker.
 * Serial side connections are reserved for a future API.
 */
export interface PluginSessionHandle {
  tabId: string
  connection: ConnectionParams
  isSsh: boolean
  getSshClient: () => Client | null
  exec: (command: string) => Promise<ClientChannel>
  openExtraShell: () => Promise<ClientChannel>
  forwardOut: (host: string, port: number) => Promise<ClientChannel>
  /** Build credentials for an isolated duplicate SSH client (no secrets leave main). */
  openDuplicateClient: () => Promise<{ client: Client; dispose: () => void }>
  openDirectTcp: (host: string, port: number) => Promise<Socket>
}

export type StreamTransform = (data: string) => string | null

export interface StreamHandlerRegistration {
  pluginId: string
  mode: 'observe' | 'intercept'
  direction: 'inbound' | 'outbound'
  handler: StreamTransform
}
