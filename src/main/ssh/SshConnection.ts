import { createHash, randomUUID } from 'crypto'
import { EventEmitter } from 'events'
import { readFileSync } from 'fs'
import { Client, ConnectConfig, PseudoTtyOptions } from 'ssh2'
import { Readable } from 'stream'
import { proxyLabel, resolveProxyChain, tunnelConfigFrom, hostProfileFromConnection } from '../../shared/connection'
import {
  ConnectionParams,
  DEFAULT_TERM_COLS,
  DEFAULT_TERM_ROWS,
  DEFAULT_TERM_TYPE,
  HostKeyDecision,
  HostKeyPrompt,
  RECONNECT_INITIAL_BACKOFF_MS,
  RECONNECT_MAX_BACKOFF_MS,
  SavePasswordDecision,
  SavePasswordPrompt,
  SessionStatus
} from '../../shared/types'
import { CredentialVault } from '../store/credentialVault'
import { KnownHostsStore, SessionStore } from '../store/sessionStore'
import { TunnelManager } from './TunnelManager'

/** SSH connect ready timeout ms */
const CONNECT_READY_TIMEOUT_MS = 20000
/** Status when the remote shell ends (logout / exit), not a network drop */
const SESSION_CLOSED_MESSAGE = 'Session closed'

export class SshConnection extends EventEmitter {
  private client: Client | null = null
  private proxyClients: Client[] = []
  private stream: import('ssh2').ClientChannel | null = null
  private disposed = false
  private hostKeyWait: { resolve: (d: HostKeyDecision) => void } | null = null
  private interactivePassword: string | null = null
  private interactiveUsername: string | null = null
  private usedInteractivePassword = false
  private pendingSavePassword: string | null = null
  private cols = DEFAULT_TERM_COLS
  private rows = DEFAULT_TERM_ROWS
  private termType = DEFAULT_TERM_TYPE
  private reconnectAttempt = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private autoReconnect = false
  private maxReconnectAttempts = 0
  private intentionalDisconnect = false
  private authInputActive = false
  private everConnected = false
  /** True after the remote shell sent an exit-status / exit-signal (logout, not a drop) */
  private remoteShellExited = false
  private tunnels = new TunnelManager((message) => {
    this.emit('data', `\r\n[WaSSH] ${message}\r\n`)
  })

  constructor(
    readonly tabId: string,
    private connection: ConnectionParams,
    private vault: CredentialVault,
    private knownHosts: KnownHostsStore,
    private sessionStore: SessionStore
  ) {
    super()
  }

  getConnection(): ConnectionParams {
    return this.connection
  }

  updateConnection(partial: Partial<ConnectionParams>): void {
    this.connection = { ...this.connection, ...partial }
  }

  setReconnectPolicy(enabled: boolean, maxAttempts: number): void {
    this.autoReconnect = enabled
    this.maxReconnectAttempts = maxAttempts
  }

  async connect(cols: number, rows: number, termType: string): Promise<void> {
    this.cols = cols
    this.rows = rows
    this.termType = termType
    this.intentionalDisconnect = false
    this.reconnectAttempt = 0
    this.everConnected = false
    this.remoteShellExited = false
    await this.open()
  }

  respondHostKey(decision: HostKeyDecision): void {
    if (!this.hostKeyWait) {
      return
    }
    const wait = this.hostKeyWait
    this.hostKeyWait = null
    wait.resolve(decision)
  }

  respondSavePassword(decision: SavePasswordDecision, hostName?: string): void {
    const password = this.pendingSavePassword
    this.pendingSavePassword = null
    if (!password || decision === 'skip') {
      return
    }
    if (decision === 'save' && this.connection.hostId) {
      const vaultId = this.connection.passwordVaultId || `pwd-${this.connection.hostId}`
      this.vault.set(vaultId, password)
      const host = this.sessionStore.getHost(this.connection.hostId)
      if (host) {
        host.passwordVaultId = vaultId
        host.authMethod = host.authMethod === 'none' ? 'password' : host.authMethod
        this.sessionStore.saveHost(host)
      }
      this.connection.passwordVaultId = vaultId
      this.connection.ephemeralPassword = ''
      return
    }
    const id = randomUUID()
    const vaultId = `pwd-${id}`
    this.vault.set(vaultId, password)
    const name = hostName || `${this.connection.username}@${this.connection.host}`
    const profile = hostProfileFromConnection(this.connection, id)
    profile.name = name
    profile.passwordVaultId = vaultId
    profile.authMethod = 'password'
    this.sessionStore.saveHost(profile)
    this.connection.hostId = id
    this.connection.name = name
    this.connection.passwordVaultId = vaultId
    this.connection.ephemeralPassword = ''
  }

  write(data: string): void {
    if (this.authInputActive) {
      this.emit('terminalInput', data)
      return
    }
    this.stream?.write(data)
  }

  resize(cols: number, rows: number): void {
    this.cols = cols
    this.rows = rows
    this.stream?.setWindow(rows, cols, 0, 0)
  }

  disconnect(): void {
    this.intentionalDisconnect = true
    this.clearReconnectTimer()
    this.closeClientOnly()
    this.emitStatus('disconnected')
  }

  dispose(): void {
    this.disposed = true
    this.disconnect()
    this.removeAllListeners()
  }

  private emitStatus(status: SessionStatus, message?: string): void {
    this.emit('status', status, message)
  }

  private fingerprintOf(key: Buffer): string {
    const hash = createHash('sha256').update(key).digest('base64')
    return `SHA256:${hash.replace(/=+$/, '')}`
  }

  private waitHostKeyDecision(): Promise<HostKeyDecision> {
    return new Promise((resolve) => {
      this.hostKeyWait = { resolve }
    })
  }

  private passwordFor(params: ConnectionParams): string {
    if (params.hostId === this.connection.hostId && this.interactivePassword !== null) {
      return this.interactivePassword
    }
    if (params.ephemeralPassword) {
      return params.ephemeralPassword
    }
    if (params.passwordVaultId) {
      return this.vault.get(params.passwordVaultId) || ''
    }
    return ''
  }

  private passphraseFor(params: ConnectionParams): string {
    if (params.ephemeralPassphrase) {
      return params.ephemeralPassphrase
    }
    if (params.passphraseVaultId) {
      return this.vault.get(params.passphraseVaultId) || ''
    }
    return ''
  }

  private usernameFor(params: ConnectionParams): string {
    if (params.hostId === this.connection.hostId && this.interactiveUsername !== null) {
      return this.interactiveUsername
    }
    return params.username
  }

  private promptTerminal(label: string, hideInput: boolean): Promise<string> {
    return new Promise((resolve) => {
      this.authInputActive = true
      this.emit('data', label)
      let buf = ''
      const onData = (chunk: string): void => {
        for (const ch of chunk) {
          if (ch === '\r' || ch === '\n') {
            this.off('terminalInput', onData)
            this.authInputActive = false
            this.emit('data', '\r\n')
            resolve(buf)
            return
          }
          if (ch === '\u007f' || ch === '\b') {
            if (buf.length > 0) {
              buf = buf.slice(0, -1)
              if (!hideInput) {
                this.emit('data', '\b \b')
              }
            }
            continue
          }
          if (ch < ' ' && ch !== '\t') {
            continue
          }
          buf += ch
          if (!hideInput) {
            this.emit('data', ch)
          }
        }
      }
      this.on('terminalInput', onData)
    })
  }

  private async ensureTargetCredentials(): Promise<void> {
    let username = this.usernameFor(this.connection)
    if (!username) {
      this.emitStatus('authenticating', 'Username required')
      username = (await this.promptTerminal('login as: ', false)).trim()
      this.interactiveUsername = username
      this.connection.username = username
    }

    const hasKey = Boolean(this.connection.privateKeyPath)
    const hasPassword =
      Boolean(this.connection.ephemeralPassword) ||
      Boolean(
        this.connection.passwordVaultId &&
          this.vault.get(this.connection.passwordVaultId)
      )

    if (!hasKey && !hasPassword) {
      this.emitStatus('authenticating', 'Password required')
      const pwd = await this.promptTerminal(
        `${username}@${this.connection.host}'s password: `,
        true
      )
      this.interactivePassword = pwd
      this.usedInteractivePassword = true
    }
  }

  private proxyCredentialsReady(params: ConnectionParams): boolean {
    if (!params.username) {
      return false
    }
    if (params.privateKeyPath) {
      return true
    }
    if (params.ephemeralPassword) {
      return true
    }
    if (params.passwordVaultId && this.vault.get(params.passwordVaultId)) {
      return true
    }
    return false
  }

  private async verifyHostKey(params: ConnectionParams, key: Buffer): Promise<boolean> {
    const fingerprint = this.fingerprintOf(key)
    const existing = this.knownHosts.find(params.host, params.port)
    if (existing && existing.fingerprint === fingerprint) {
      return true
    }
    const reason = existing ? 'mismatch' : 'unknown'
    this.emitStatus(
      'awaiting_host_key',
      reason === 'mismatch' ? `Host key mismatch (${params.host})` : undefined
    )
    const prompt: HostKeyPrompt = {
      tabId: this.tabId,
      host: params.host,
      port: params.port,
      keyType: 'ssh-hostkey',
      fingerprint,
      reason
    }
    this.emit('hostKeyPrompt', prompt)
    const decision = await this.waitHostKeyDecision()
    if (decision !== 'accept') {
      return false
    }
    this.knownHosts.upsert({
      host: params.host,
      port: params.port,
      keyType: 'ssh-hostkey',
      fingerprint
    })
    return true
  }

  private buildConfig(params: ConnectionParams, sock?: Readable): ConnectConfig {
    const username = this.usernameFor(params)
    const config: ConnectConfig = {
      host: params.host,
      port: params.port,
      username,
      readyTimeout: CONNECT_READY_TIMEOUT_MS,
      tryKeyboard: true,
      hostVerifier: (key: Buffer, verify: (ok: boolean) => void) => {
        void this.verifyHostKey(params, key).then(verify)
      }
    }
    if (sock) {
      config.sock = sock
    }

    const keyPath = params.privateKeyPath
    if (keyPath) {
      try {
        config.privateKey = readFileSync(keyPath)
        const passphrase = this.passphraseFor(params)
        if (passphrase) {
          config.passphrase = passphrase
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        throw new Error(`Key read failed (${params.name || params.host}): ${msg}`)
      }
    }

    const password = this.passwordFor(params)
    if (password) {
      config.password = password
    }

    return config
  }

  private attachKeyboardInteractive(client: Client, params: ConnectionParams): void {
    client.on('keyboard-interactive', (_name, _instructions, _lang, prompts, finish) => {
      void (async () => {
        const answers: string[] = []
        for (const p of prompts) {
          const hide = !p.echo
          const answer = await this.promptTerminal(
            `[${params.name || params.host}] ${p.prompt}`,
            hide
          )
          answers.push(answer)
          if (hide && answer && params.hostId === this.connection.hostId) {
            this.interactivePassword = answer
            this.usedInteractivePassword = true
          }
        }
        finish(answers)
      })()
    })
  }

  private connectClient(params: ConnectionParams, sock?: Readable): Promise<Client> {
    return new Promise((resolve, reject) => {
      const client = new Client()
      this.attachKeyboardInteractive(client, params)

      const onReady = (): void => {
        cleanup()
        resolve(client)
      }
      const onError = (err: Error): void => {
        cleanup()
        reject(err)
      }
      const onClose = (): void => {
        cleanup()
        reject(new Error(`Connection closed (${params.name || params.host})`))
      }
      const cleanup = (): void => {
        client.removeListener('ready', onReady)
        client.removeListener('error', onError)
        client.removeListener('close', onClose)
      }

      client.once('ready', onReady)
      client.once('error', onError)
      client.once('close', onClose)

      try {
        client.connect(this.buildConfig(params, sock))
      } catch (err) {
        cleanup()
        reject(err instanceof Error ? err : new Error(String(err)))
      }
    })
  }

  private forwardThrough(
    client: Client,
    destHost: string,
    destPort: number
  ): Promise<Readable> {
    return new Promise((resolve, reject) => {
      client.forwardOut('127.0.0.1', 0, destHost, destPort, (err, stream) => {
        if (err) {
          reject(err)
          return
        }
        resolve(stream)
      })
    })
  }

  private async open(): Promise<void> {
    if (this.disposed) {
      return
    }
    this.clearReconnectTimer()
    this.closeClientOnly()
    this.remoteShellExited = false
    this.emitStatus('connecting')

    let chain: ConnectionParams[]
    try {
      chain = resolveProxyChain(this.connection, this.sessionStore.listHosts())
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      this.emitStatus('failed', msg)
      return
    }

    const via = proxyLabel(chain)
    if (via) {
      this.emitStatus('connecting', `Via ${via}`)
    }

    for (const hop of chain.slice(0, -1)) {
      if (!this.proxyCredentialsReady(hop)) {
        this.emitStatus(
          'failed',
          `Proxy ${hop.name || hop.host} needs username and credentials in host settings`
        )
        return
      }
    }

    await this.ensureTargetCredentials()
    if (this.disposed || this.intentionalDisconnect) {
      return
    }

    try {
      let sock: Readable | undefined
      this.proxyClients = []

      for (let i = 0; i < chain.length - 1; i++) {
        const hop = chain[i]
        const next = chain[i + 1]
        const hopClient = await this.connectClient(hop)
        this.proxyClients.push(hopClient)
        sock = await this.forwardThrough(hopClient, next.host, next.port)
      }

      const target = chain[chain.length - 1]
      this.client = await this.connectClient(target, sock)
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      this.emitStatus('failed', msg)
      this.closeClientOnly()
      this.scheduleReconnect()
      return
    }

    if (this.disposed || this.intentionalDisconnect) {
      this.closeClientOnly()
      return
    }

    this.emitStatus('authenticating')
    await this.startShell(this.client)
  }

  private startShell(client: Client): Promise<void> {
    return new Promise((resolve) => {
      const pty: PseudoTtyOptions = {
        term: this.termType,
        cols: this.cols,
        rows: this.rows
      }
      const tunnelOpts = tunnelConfigFrom(this.connection)
      const shellOptions = tunnelOpts.x11Forwarding ? { x11: true } : {}

      void this.tunnels.start(client, tunnelOpts.tunnels, tunnelOpts.x11Forwarding).then(() => {
        client.shell(pty, shellOptions, (err, stream) => {
          if (err) {
            this.tunnels.stop()
            this.emitStatus('failed', err.message)
            this.scheduleReconnect()
            resolve()
            return
          }
          this.stream = stream
          this.reconnectAttempt = 0
          this.everConnected = true
          this.emitStatus('connected')

          if (this.usedInteractivePassword && this.interactivePassword) {
            this.pendingSavePassword = this.interactivePassword
            this.usedInteractivePassword = false
            this.emit('savePasswordPrompt', {
              tabId: this.tabId,
              hasHostProfile: Boolean(this.connection.hostId)
            } satisfies SavePasswordPrompt)
          }

          stream.on('data', (data: Buffer) => {
            this.emit('data', data.toString('utf8'))
          })
          stream.stderr.on('data', (data: Buffer) => {
            this.emit('data', data.toString('utf8'))
          })
          stream.on('exit', () => {
            this.remoteShellExited = true
          })
          stream.on('close', () => {
            this.stream = null
            this.tunnels.stop()
            if (this.intentionalDisconnect || this.disposed) {
              this.emitStatus('disconnected')
              resolve()
              return
            }
            if (this.remoteShellExited) {
              this.endAfterRemoteLogout()
              resolve()
              return
            }
            this.emitStatus('disconnected', SESSION_CLOSED_MESSAGE)
            this.scheduleReconnect()
            resolve()
          })
        })
      })

      client.on('close', () => {
        if (this.intentionalDisconnect || this.disposed) {
          return
        }
        if (!this.stream) {
          this.scheduleReconnect()
        }
      })
    })
  }

  private endAfterRemoteLogout(): void {
    this.intentionalDisconnect = true
    this.clearReconnectTimer()
    this.closeClientOnly()
    this.emitStatus('closed', SESSION_CLOSED_MESSAGE)
  }

  private scheduleReconnect(): void {
    if (!this.autoReconnect || this.intentionalDisconnect || this.disposed || !this.everConnected) {
      return
    }
    if (this.reconnectTimer) {
      return
    }
    if (this.reconnectAttempt >= this.maxReconnectAttempts) {
      this.emitStatus('failed', 'Reconnect attempts exhausted')
      return
    }
    const attempt = this.reconnectAttempt
    this.reconnectAttempt += 1
    const delay = Math.min(
      RECONNECT_INITIAL_BACKOFF_MS * 2 ** attempt,
      RECONNECT_MAX_BACKOFF_MS
    )
    this.emitStatus(
      'reconnecting',
      `Attempt ${this.reconnectAttempt}/${this.maxReconnectAttempts}`
    )
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      void this.open()
    }, delay)
  }

  private clearReconnectTimer(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
  }

  private closeClientOnly(): void {
    this.tunnels.stop()
    try {
      this.stream?.close()
    } catch {
      /* ignore */
    }
    this.stream = null
    try {
      this.client?.end()
    } catch {
      /* ignore */
    }
    this.client = null
    for (const proxy of this.proxyClients) {
      try {
        proxy.end()
      } catch {
        /* ignore */
      }
    }
    this.proxyClients = []
  }
}
