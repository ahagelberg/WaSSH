import { BrowserWindow } from 'electron'
import {
  CONNECTION_TYPE_SERIAL,
  CONNECTION_TYPE_TELNET,
  ConnectRequest,
  HostKeyDecision,
  SavePasswordDecision,
  SessionStatus
} from '../../shared/types'
import { connectionTypeOf } from '../../shared/connection'
import { CredentialVault } from '../store/credentialVault'
import { KnownHostsStore, SessionStore, SettingsStore } from '../store/sessionStore'
import { SerialConnection } from '../serial/SerialConnection'
import { TelnetConnection } from '../telnet/TelnetConnection'
import { SshConnection } from './SshConnection'

type LiveSession = SshConnection | TelnetConnection | SerialConnection

export class SessionManager {
  private sessions = new Map<string, LiveSession>()

  constructor(
    private vault: CredentialVault,
    private knownHosts: KnownHostsStore,
    private sessionStore: SessionStore,
    private settingsStore: SettingsStore,
    private getWindow: () => BrowserWindow | null
  ) {}

  private send(channel: string, ...args: unknown[]): void {
    const win = this.getWindow()
    if (!win || win.isDestroyed()) {
      return
    }
    win.webContents.send(channel, ...args)
  }

  private wire(conn: LiveSession): void {
    const settings = this.settingsStore.get()
    conn.setReconnectPolicy(settings.autoReconnectOnDrop, settings.reconnectMaxAttempts)

    conn.on('data', (data: string) => {
      this.send('session:data', conn.tabId, data)
    })
    conn.on('status', (status: SessionStatus, message?: string) => {
      this.send('session:status', { tabId: conn.tabId, status, message })
    })
    conn.on('hostKeyPrompt', (prompt) => {
      this.send('session:hostKeyPrompt', prompt)
    })
    conn.on('savePasswordPrompt', (prompt) => {
      this.send('session:savePasswordPrompt', prompt)
    })
  }

  async connect(req: ConnectRequest): Promise<void> {
    this.disconnect(req.tabId)
    const type = connectionTypeOf(req.connection)
    let conn: LiveSession
    if (type === CONNECTION_TYPE_TELNET) {
      conn = new TelnetConnection(req.tabId, req.connection)
    } else if (type === CONNECTION_TYPE_SERIAL) {
      conn = new SerialConnection(req.tabId, req.connection)
    } else {
      conn = new SshConnection(
        req.tabId,
        req.connection,
        this.vault,
        this.knownHosts,
        this.sessionStore
      )
    }
    this.sessions.set(req.tabId, conn)
    this.wire(conn)
    await conn.connect(req.cols, req.rows, req.termType)
  }

  disconnect(tabId: string): void {
    const conn = this.sessions.get(tabId)
    if (!conn) {
      return
    }
    conn.dispose()
    this.sessions.delete(tabId)
  }

  write(tabId: string, data: string): void {
    this.sessions.get(tabId)?.write(data)
  }

  resize(tabId: string, cols: number, rows: number): void {
    this.sessions.get(tabId)?.resize(cols, rows)
  }

  respondHostKey(tabId: string, decision: HostKeyDecision): void {
    this.sessions.get(tabId)?.respondHostKey(decision)
  }

  respondSavePassword(
    tabId: string,
    decision: SavePasswordDecision,
    hostName?: string
  ): void {
    this.sessions.get(tabId)?.respondSavePassword(decision, hostName)
  }

  getConnection(tabId: string) {
    return this.sessions.get(tabId)?.getConnection()
  }

  updateReconnectPolicies(): void {
    const settings = this.settingsStore.get()
    for (const conn of Array.from(this.sessions.values())) {
      conn.setReconnectPolicy(settings.autoReconnectOnDrop, settings.reconnectMaxAttempts)
    }
  }

  disposeAll(): void {
    for (const id of Array.from(this.sessions.keys())) {
      this.disconnect(id)
    }
  }
}
