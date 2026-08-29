import { randomUUID } from 'crypto'
import { BrowserWindow } from 'electron'
import { Socket, connect as netConnect } from 'net'
import type { ClientChannel } from 'ssh2'
import type {
  SideConnectionClosedEvent,
  SideConnectionDataEvent,
  SideConnectionOpenRequest
} from '../../shared/plugins'
import type { PluginSessionHandle } from './types'

interface OpenSideConnection {
  id: string
  tabId: string
  pluginId: string
  dispose: () => void
  write: (data: string) => void
}

export class SideConnectionBroker {
  private connections = new Map<string, OpenSideConnection>()

  constructor(
    private getSession: (tabId: string) => PluginSessionHandle | null,
    private getWindow: () => BrowserWindow | null,
    private onData?: (connectionId: string, data: string) => void,
    private onClosed?: (connectionId: string, error?: string) => void
  ) {}

  private send(channel: string, payload: unknown): void {
    const win = this.getWindow()
    if (!win || win.isDestroyed()) {
      return
    }
    win.webContents.send(channel, payload)
  }

  private emitData(connectionId: string, data: string): void {
    this.send('plugin:sideData', {
      connectionId,
      data
    } satisfies SideConnectionDataEvent)
    this.onData?.(connectionId, data)
  }

  private emitClosed(connectionId: string, error?: string): void {
    this.send('plugin:sideClosed', {
      connectionId,
      error
    } satisfies SideConnectionClosedEvent)
    this.onClosed?.(connectionId, error)
  }

  async open(
    tabId: string,
    pluginId: string,
    req: SideConnectionOpenRequest
  ): Promise<string> {
    const session = this.getSession(tabId)
    if (!session) {
      throw new Error('Session is not connected')
    }

    if (req.kind === 'serial') {
      throw new Error('Serial side connections are not implemented yet')
    }

    if (req.kind === 'ssh-exec') {
      if (!session.isSsh) {
        throw new Error('SSH exec requires an SSH session')
      }
      const command = req.command?.trim()
      if (!command) {
        throw new Error('SSH exec requires a command')
      }
      const channel = await session.exec(command)
      return this.attachChannel(tabId, pluginId, channel)
    }

    if (req.kind === 'ssh-shell') {
      if (!session.isSsh) {
        throw new Error('SSH shell requires an SSH session')
      }
      if (req.duplicate) {
        const dup = await session.openDuplicateClient()
        const channel = await new Promise<ClientChannel>((resolve, reject) => {
          dup.client.shell((err, stream) => {
            if (err) {
              dup.dispose()
              reject(err)
              return
            }
            resolve(stream)
          })
        })
        return this.attachChannel(tabId, pluginId, channel, () => dup.dispose())
      }
      const channel = await session.openExtraShell()
      return this.attachChannel(tabId, pluginId, channel)
    }

    if (req.kind === 'tcp') {
      const host = (req.host || session.connection.host).trim()
      const port = req.port ?? session.connection.port
      if (!host || !port) {
        throw new Error('TCP side connection requires host and port')
      }
      if (session.isSsh && session.getSshClient()) {
        const channel = await session.forwardOut(host, port)
        return this.attachChannel(tabId, pluginId, channel)
      }
      const socket = await session.openDirectTcp(host, port)
      return this.attachSocket(tabId, pluginId, socket)
    }

    throw new Error(`Unknown side connection kind: ${req.kind}`)
  }

  write(connectionId: string, data: string): void {
    this.connections.get(connectionId)?.write(data)
  }

  close(connectionId: string): void {
    const conn = this.connections.get(connectionId)
    if (!conn) {
      return
    }
    this.connections.delete(connectionId)
    conn.dispose()
    this.emitClosed(connectionId)
  }

  closeForPlugin(tabId: string, pluginId: string): void {
    for (const [id, conn] of Array.from(this.connections.entries())) {
      if (conn.tabId === tabId && conn.pluginId === pluginId) {
        this.close(id)
      }
    }
  }

  closeForTab(tabId: string): void {
    for (const [id, conn] of Array.from(this.connections.entries())) {
      if (conn.tabId === tabId) {
        this.close(id)
      }
    }
  }

  disposeAll(): void {
    for (const id of Array.from(this.connections.keys())) {
      this.close(id)
    }
  }

  private attachChannel(
    tabId: string,
    pluginId: string,
    channel: ClientChannel,
    extraDispose?: () => void
  ): string {
    const id = randomUUID()
    const dispose = (): void => {
      try {
        channel.close()
      } catch {
        /* ignore */
      }
      extraDispose?.()
    }
    channel.on('data', (buf: Buffer) => {
      this.emitData(id, buf.toString('utf8'))
    })
    channel.stderr?.on('data', (buf: Buffer) => {
      this.emitData(id, buf.toString('utf8'))
    })
    channel.on('close', () => {
      if (!this.connections.has(id)) {
        return
      }
      this.connections.delete(id)
      extraDispose?.()
      this.emitClosed(id)
    })
    channel.on('error', (err: Error) => {
      if (!this.connections.has(id)) {
        return
      }
      this.connections.delete(id)
      extraDispose?.()
      this.emitClosed(id, err.message)
    })
    this.connections.set(id, {
      id,
      tabId,
      pluginId,
      dispose,
      write: (data: string) => {
        try {
          channel.write(data)
        } catch {
          /* ignore */
        }
      }
    })
    return id
  }

  private attachSocket(tabId: string, pluginId: string, socket: Socket): string {
    const id = randomUUID()
    const dispose = (): void => {
      try {
        socket.destroy()
      } catch {
        /* ignore */
      }
    }
    socket.on('data', (buf: Buffer) => {
      this.emitData(id, buf.toString('utf8'))
    })
    socket.on('close', () => {
      if (!this.connections.has(id)) {
        return
      }
      this.connections.delete(id)
      this.emitClosed(id)
    })
    socket.on('error', (err: Error) => {
      if (!this.connections.has(id)) {
        return
      }
      this.connections.delete(id)
      this.emitClosed(id, err.message)
    })
    this.connections.set(id, {
      id,
      tabId,
      pluginId,
      dispose,
      write: (data: string) => {
        try {
          socket.write(data)
        } catch {
          /* ignore */
        }
      }
    })
    return id
  }
}

/** Direct TCP helper used when the session is not SSH (or for localhost). */
export function openDirectTcpSocket(host: string, port: number): Promise<Socket> {
  return new Promise((resolve, reject) => {
    const socket = netConnect({ host, port }, () => resolve(socket))
    socket.once('error', reject)
  })
}
