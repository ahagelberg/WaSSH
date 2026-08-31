import type { Client, ClientChannel, SFTPWrapper } from 'ssh2'
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
  /** Run a command and return its trimmed stdout (e.g. `pwd`). */
  execCapture: (command: string) => Promise<string>
  /** Open an SFTP channel on the live SSH connection. */
  openSftp: () => Promise<SFTPWrapper>
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
