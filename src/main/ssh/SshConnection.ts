import { createHash, randomUUID } from 'crypto'
import { EventEmitter } from 'events'
import { readFileSync } from 'fs'
import { Client, ClientChannel, ConnectConfig, PseudoTtyOptions } from 'ssh2'
import type { SFTPWrapper } from 'ssh2'
import { Readable } from 'stream'
import { proxyLabel, resolveProxyChain, tunnelConfigFrom, hostProfileFromConnection, reconnectModeFrom, reconnectModeSchedulesBackoff, reconnectModeWantsFocus, screenConfigFrom, parseScreenListForName, parseTmuxListForName, remoteSessionBusyFallbackMessage, remoteSessionUnavailableMessage, type ScreenSessionConfig, type ScreenSessionPresence } from '../../shared/connection'
import {
  ConnectionParams,
  DEFAULT_RECONNECT_MODE,
  DEFAULT_TERM_COLS,
  DEFAULT_TERM_ROWS,
  DEFAULT_TERM_TYPE,
  HostKeyDecision,
  HostKeyPrompt,
  RECONNECT_INITIAL_BACKOFF_MS,
  RECONNECT_MAX_BACKOFF_MS,
  REMOTE_SESSION_KIND_TMUX,
  SCREEN_BUSY_DO_NOT_ATTACH,
  SCREEN_BUSY_FORCE_DETACH,
  SCREEN_BUSY_SHARE,
  SavePasswordDecision,
  SavePasswordPrompt,
  SessionStatus,
  type ReconnectMode,
  type RemoteSessionKind
} from '../../shared/types'
import { CredentialVault } from '../store/credentialVault'
import { KnownHostsStore, SessionStore } from '../store/sessionStore'
import { TunnelManager } from './TunnelManager'

/** SSH connect ready timeout ms */
const CONNECT_READY_TIMEOUT_MS = 20000
/** Status when the remote shell ends (logout / exit), not a network drop */
const SESSION_CLOSED_MESSAGE = 'Session closed'
/** ms → whole seconds for reconnect status text */
const MS_PER_SECOND = 1000
/** Exit status when the remote executable is missing */
const EXEC_NOT_FOUND_STATUS = 127

function shellSingleQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

function remoteSessionAttachCommand(
  kind: RemoteSessionKind,
  sessionName: string,
  busyHandling: ScreenSessionConfig['screenBusyHandling']
): string {
  const quoted = shellSingleQuote(sessionName)
  if (kind === REMOTE_SESSION_KIND_TMUX) {
    if (busyHandling === SCREEN_BUSY_SHARE) {
      return `tmux new-session -A -s ${quoted}`
    }
    if (busyHandling === SCREEN_BUSY_FORCE_DETACH) {
      return `tmux new-session -A -D -s ${quoted}`
    }
    return `tmux new-session -s ${quoted}`
  }
  if (busyHandling === SCREEN_BUSY_SHARE) {
    return `screen -S ${quoted} -xR`
  }
  if (busyHandling === SCREEN_BUSY_FORCE_DETACH) {
    return `screen -S ${quoted} -d -RR`
  }
  return `screen -S ${quoted}`
}

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
  private reconnectMode: ReconnectMode = DEFAULT_RECONNECT_MODE
  private intentionalDisconnect = false
  private authInputActive = false
  private everConnected = false
  /** True while open() is connecting, before the shell stream is ready */
  private opening = false
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
    this.reconnectMode = reconnectModeFrom(connection)
  }

  getConnection(): ConnectionParams {
    return this.connection
  }

  updateConnection(partial: Partial<ConnectionParams>): void {
    this.connection = { ...this.connection, ...partial }
    if (partial.reconnectMode !== undefined) {
      this.setReconnectPolicy(reconnectModeFrom(this.connection))
    }
  }

  setReconnectPolicy(mode: ReconnectMode): void {
    this.reconnectMode = mode
  }

  wantsFocusReconnect(): boolean {
    return reconnectModeWantsFocus(this.reconnectMode)
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
    try {
      this.stream?.write(data)
    } catch {
      /* ignore */
    }
  }

  resize(cols: number, rows: number): void {
    this.cols = cols
    this.rows = rows
    try {
      this.stream?.setWindow(rows, cols, 0, 0)
    } catch {
      /* ignore */
    }
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

  prepareForSleep(): void {
    if (this.disposed || this.intentionalDisconnect) {
      return
    }
    this.clearReconnectTimer()
    if (!this.stream && !this.client && !this.everConnected) {
      return
    }
    this.closeClientOnly()
    this.emitStatus('disconnected', 'Computer sleep')
  }

  reconnectNow(): void {
    if (this.disposed || this.intentionalDisconnect) {
      return
    }
    if (!this.everConnected) {
      return
    }
    if (this.stream || this.opening) {
      return
    }
    this.reconnectAttempt = 0
    this.clearReconnectTimer()
    void this.open()
  }

  getSshClient(): Client | null {
    return this.client
  }

  execCommand(command: string): Promise<ClientChannel> {
    const client = this.client
    if (!client) {
      return Promise.reject(new Error('SSH session is not connected'))
    }
    return new Promise((resolve, reject) => {
      client.exec(command, (err, channel) => {
        if (err) {
          reject(err)
          return
        }
        resolve(channel)
      })
    })
  }

  /**
   * Open a new SFTP channel on the live connection.
   * A separate channel from the interactive shell, so it coexists safely.
   */
  openSftp(): Promise<SFTPWrapper> {
    const client = this.client
    if (!client) {
      return Promise.reject(new Error('SSH session is not connected'))
    }
    return new Promise((resolve, reject) => {
      client.sftp((err, sftp) => {
        if (err) {
          reject(err)
          return
        }
        resolve(sftp)
      })
    })
  }

  /**
   * Run a command and capture its trimmed stdout.
   * Used for quick probes (e.g. `pwd`). Rejects on non-zero exit or errors.
   */
  execCapture(command: string): Promise<string> {
    return this.execCommand(command).then((channel) => {
      return new Promise<string>((resolve, reject) => {
        let out = ''
        let errOut = ''
        let exitCode: number | null = null
        let settled = false
        const finish = (err?: Error): void => {
          if (settled) {
            return
          }
          settled = true
          try {
            channel.close()
          } catch {
            /* ignore */
          }
          if (err) {
            reject(err)
            return
          }
          if (exitCode !== null && exitCode !== 0) {
            reject(new Error(errOut.trim() || `Command exited with code ${exitCode}`))
            return
          }
          resolve(out.trim())
        }
        channel.on('data', (buf: Buffer) => {
          out += buf.toString('utf8')
        })
        channel.stderr?.on('data', (buf: Buffer) => {
          errOut += buf.toString('utf8')
        })
        channel.on('exit', (code: number | null) => {
          exitCode = code
        })
        channel.on('close', () => finish())
        channel.on('error', (err: Error) => finish(err))
      })
    })
  }

  openExtraShell(): Promise<ClientChannel> {
    const client = this.client
    if (!client) {
      return Promise.reject(new Error('SSH session is not connected'))
    }
    const pty: PseudoTtyOptions = {
      term: this.termType,
      cols: this.cols,
      rows: this.rows
    }
    return new Promise((resolve, reject) => {
      client.shell(pty, (err, channel) => {
        if (err) {
          reject(err)
          return
        }
        resolve(channel)
      })
    })
  }

  forwardOut(destHost: string, destPort: number): Promise<ClientChannel> {
    const client = this.client
    if (!client) {
      return Promise.reject(new Error('SSH session is not connected'))
    }
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

  /**
   * Opens an isolated SSH client using the same profile/proxy chain.
   * Does not disturb the interactive shell.
   */
  async openDuplicateClient(): Promise<{ client: Client; dispose: () => void }> {
    let chain: ConnectionParams[]
    try {
      chain = resolveProxyChain(this.connection, this.sessionStore.listHosts())
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      throw new Error(msg)
    }

    const proxies: Client[] = []
    let sock: Readable | undefined
    try {
      for (let i = 0; i < chain.length - 1; i++) {
        const hop = chain[i]
        const next = chain[i + 1]
        const hopClient = await this.connectPluginClient(hop)
        proxies.push(hopClient)
        sock = await this.forwardThroughQuiet(hopClient, next.host, next.port)
      }
      const target = chain[chain.length - 1]
      const client = await this.connectPluginClient(target, sock)
      return {
        client,
        dispose: () => {
          this.quietEnd(client)
          for (const proxy of proxies) {
            this.quietEnd(proxy)
          }
        }
      }
    } catch (err) {
      for (const proxy of proxies) {
        this.quietEnd(proxy)
      }
      throw err
    }
  }

  private connectPluginClient(params: ConnectionParams, sock?: Readable): Promise<Client> {
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
      client.on('error', () => {
        /* absorb after ready */
      })

      try {
        client.connect(this.buildConfig(params, sock))
      } catch (err) {
        cleanup()
        reject(err instanceof Error ? err : new Error(String(err)))
      }
    })
  }

  private forwardThroughQuiet(
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
        stream.on('error', () => {
          /* absorb — plugin duplicate path */
        })
        resolve(stream)
      })
    })
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

  private guardClient(client: Client): void {
    client.on('error', (err: Error) => {
      if (this.client !== client && !this.proxyClients.includes(client)) {
        return
      }
      this.onTransportError(err)
    })
  }

  private onTransportError(err: Error): void {
    if (this.intentionalDisconnect || this.disposed || this.opening) {
      return
    }
    this.emitStatus('disconnected', err.message)
    this.closeClientOnly()
    this.scheduleReconnect()
  }

  private quietEnd(client: Client | null): void {
    if (!client) {
      return
    }
    client.removeAllListeners()
    client.on('error', () => {
      /* absorb errors from ending a dead socket */
    })
    try {
      client.end()
    } catch {
      /* ignore */
    }
  }

  private connectClient(params: ConnectionParams, sock?: Readable): Promise<Client> {
    return new Promise((resolve, reject) => {
      const client = new Client()
      this.attachKeyboardInteractive(client, params)
      this.guardClient(client)

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
        stream.on('error', (streamErr: Error) => {
          this.onTransportError(streamErr)
        })
        resolve(stream)
      })
    })
  }

  private async open(): Promise<void> {
    if (this.disposed) {
      return
    }
    this.opening = true
    this.clearReconnectTimer()
    this.closeClientOnly()
    this.remoteShellExited = false
    this.emitStatus('connecting')

    let chain: ConnectionParams[]
    try {
      chain = resolveProxyChain(this.connection, this.sessionStore.listHosts())
    } catch (err) {
      this.opening = false
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
        this.opening = false
        this.emitStatus(
          'failed',
          `Proxy ${hop.name || hop.host} needs username and credentials in host settings`
        )
        return
      }
    }

    await this.ensureTargetCredentials()
    if (this.disposed || this.intentionalDisconnect) {
      this.opening = false
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
      this.opening = false
      const msg = err instanceof Error ? err.message : String(err)
      this.emitStatus('failed', msg)
      this.closeClientOnly()
      this.scheduleReconnect()
      return
    }

    if (this.disposed || this.intentionalDisconnect) {
      this.opening = false
      this.closeClientOnly()
      return
    }

    this.emitStatus('authenticating')
    const client = this.client
    if (!client) {
      this.opening = false
      this.emitStatus('failed', 'SSH client missing after connect')
      return
    }
    await this.startShell(client)
  }

  private startShell(client: Client): Promise<void> {
    return new Promise((resolve) => {
      const pty: PseudoTtyOptions = {
        term: this.termType,
        cols: this.cols,
        rows: this.rows
      }
      const tunnelOpts = tunnelConfigFrom(this.connection)
      const channelOptions = tunnelOpts.x11Forwarding ? { x11: true as const } : {}

      void this.tunnels
        .start(client, tunnelOpts.tunnels, tunnelOpts.x11Forwarding)
        .then(async () => {
          const screenPlan = await this.resolveRemoteSessionChannel(client)
          this.openInteractiveChannel(
            client,
            pty,
            channelOptions,
            screenPlan,
            resolve
          )
        })
        .catch((err: unknown) => {
          this.opening = false
          const msg = err instanceof Error ? err.message : String(err)
          this.emitStatus('failed', msg)
          this.scheduleReconnect()
          resolve()
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

  private openInteractiveChannel(
    client: Client,
    pty: PseudoTtyOptions,
    channelOptions: { x11?: true },
    screenPlan: { command: string | null; statusMessage?: string },
    resolve: () => void
  ): void {
    const wireStream = (
      stream: ClientChannel,
      statusMessage?: string,
      multiplexer = false
    ): void => {
      this.stream = stream
      this.opening = false
      this.reconnectAttempt = 0
      this.everConnected = true
      this.emitStatus('connected', statusMessage)

      if (multiplexer) {
        // A newly created screen/tmux window only paints its first frame after a
        // window-change / SIGWINCH. Re-apply the current size (the exec pty may
        // have been created at the default size because resizes that arrived
        // before the channel were dropped) and signal WINCH so the prompt shows.
        try {
          stream.setWindow(this.rows, this.cols, 0, 0)
        } catch {
          /* ignore */
        }
        try {
          stream.signal('WINCH')
        } catch {
          /* ignore */
        }
      }

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
      stream.stderr?.on('data', (data: Buffer) => {
        this.emit('data', data.toString('utf8'))
      })
      stream.on('error', (streamErr: Error) => {
        this.onTransportError(streamErr)
      })
      stream.stderr?.on('error', () => {
        /* absorb stderr socket resets */
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
        if (this.remoteShellExited && multiplexer) {
          // The screen/tmux client exited. That may be a real logout or a
          // transient exit (e.g. detaching a nested screen took this client
          // down) while the first screen session is still alive — so decide by
          // probing the remote session instead of ending unconditionally.
          void this.recoverRemoteSessionIfAlive(client).finally(resolve)
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
    }

    const openLoginShell = (statusMessage?: string): void => {
      client.shell(pty, channelOptions, (err, stream) => {
        if (err) {
          this.opening = false
          this.tunnels.stop()
          this.emitStatus('failed', err.message)
          this.scheduleReconnect()
          resolve()
          return
        }
        wireStream(stream, statusMessage)
      })
    }

    if (!screenPlan.command) {
      openLoginShell(screenPlan.statusMessage)
      return
    }

    client.exec(screenPlan.command, { pty, ...channelOptions }, (err, stream) => {
      if (err) {
        openLoginShell()
        return
      }
      wireStream(stream, undefined, true)
    })
  }

  private async resolveRemoteSessionChannel(
    client: Client
  ): Promise<{ command: string | null; statusMessage?: string }> {
    const screen = screenConfigFrom(this.connection)
    if (!screen.openInScreen) {
      return { command: null }
    }
    const kind = screen.remoteSessionKind

    // Probe for the tool first: if tmux/screen is missing, fall back to a normal
    // shell with a warning regardless of the busy-handling mode.
    const presence = await this.probeRemoteSession(client, kind, screen.screenSessionName)
    if (presence === 'unavailable') {
      return {
        command: null,
        statusMessage: remoteSessionUnavailableMessage(kind)
      }
    }

    if (
      screen.screenBusyHandling === SCREEN_BUSY_SHARE ||
      screen.screenBusyHandling === SCREEN_BUSY_FORCE_DETACH
    ) {
      return {
        command: remoteSessionAttachCommand(kind, screen.screenSessionName, screen.screenBusyHandling)
      }
    }

    if (presence === 'attached') {
      return {
        command: null,
        statusMessage: remoteSessionBusyFallbackMessage(kind, screen.screenSessionName)
      }
    }
    if (presence === 'detached') {
      if (kind === REMOTE_SESSION_KIND_TMUX) {
        return {
          command: `tmux attach-session -t ${shellSingleQuote(screen.screenSessionName)}`
        }
      }
      return {
        command: `screen -S ${shellSingleQuote(screen.screenSessionName)} -r`
      }
    }
    return {
      command: remoteSessionAttachCommand(kind, screen.screenSessionName, SCREEN_BUSY_DO_NOT_ATTACH)
    }
  }

  private probeRemoteSession(
    client: Client,
    kind: RemoteSessionKind,
    sessionName: string
  ): Promise<'none' | 'detached' | 'attached' | 'unknown' | 'unavailable'> {
    const listCommand =
      kind === REMOTE_SESSION_KIND_TMUX ? 'tmux list-sessions' : 'screen -ls'
    return new Promise((resolve) => {
      client.exec(listCommand, (err, channel) => {
        if (err) {
          resolve('unavailable')
          return
        }
        let output = ''
        let exitStatus: number | null = null
        channel.on('data', (data: Buffer) => {
          output += data.toString('utf8')
        })
        channel.stderr?.on('data', (data: Buffer) => {
          output += data.toString('utf8')
        })
        channel.on('exit', (code) => {
          exitStatus = typeof code === 'number' ? code : null
        })
        channel.on('close', () => {
          if (exitStatus === EXEC_NOT_FOUND_STATUS) {
            resolve('unavailable')
            return
          }
          const lower = output.toLowerCase()
          if (
            lower.includes('command not found') ||
            lower.includes('no such file or directory')
          ) {
            resolve('unavailable')
            return
          }
          if (kind === REMOTE_SESSION_KIND_TMUX) {
            resolve(parseTmuxListForName(output, sessionName))
            return
          }
          resolve(parseScreenListForName(output, sessionName))
        })
      })
    })
  }

  private endAfterRemoteLogout(): void {
    this.intentionalDisconnect = true
    this.clearReconnectTimer()
    this.closeClientOnly()
    this.emitStatus('closed', SESSION_CLOSED_MESSAGE)
  }

  /**
   * The screen/tmux client of a remote session exited. The session itself may
   * still exist (e.g. a nested screen was attached from within it and its
   * detach took this client down), so only treat it as logout when the remote
   * session is really gone.
   */
  private async recoverRemoteSessionIfAlive(client: Client): Promise<void> {
    if (this.intentionalDisconnect || this.disposed || this.opening) {
      return
    }
    const screen = screenConfigFrom(this.connection)
    let presence: ScreenSessionPresence | 'unavailable'
    try {
      presence = await this.probeRemoteSession(
        client,
        screen.remoteSessionKind,
        screen.screenSessionName
      )
    } catch {
      presence = 'unavailable'
    }
    if (presence === 'detached' || presence === 'attached' || presence === 'unknown') {
      // The first screen session is still alive — re-attach to it.
      this.emitStatus('reconnecting', 'Remote session still active; re-attaching…')
      void this.open()
      return
    }
    if (presence === 'unavailable') {
      // Could not determine state (e.g. the transport dropped); reconnect.
      this.emitStatus('disconnected', SESSION_CLOSED_MESSAGE)
      this.scheduleReconnect()
      return
    }
    // No such session — it was closed; this is a real logout.
    this.endAfterRemoteLogout()
  }

  private scheduleReconnect(): void {
    if (
      !reconnectModeSchedulesBackoff(this.reconnectMode) ||
      this.intentionalDisconnect ||
      this.disposed ||
      !this.everConnected
    ) {
      return
    }
    if (this.reconnectTimer) {
      return
    }
    const attempt = this.reconnectAttempt
    this.reconnectAttempt += 1
    const delay = Math.min(
      RECONNECT_INITIAL_BACKOFF_MS * 2 ** attempt,
      RECONNECT_MAX_BACKOFF_MS
    )
    const delaySec = Math.max(1, Math.round(delay / MS_PER_SECOND))
    this.emitStatus('reconnecting', `Attempt ${this.reconnectAttempt} (in ${delaySec}s)`)
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
    const stream = this.stream
    this.stream = null
    if (stream) {
      stream.removeAllListeners()
      stream.on('error', () => {
        /* absorb */
      })
      try {
        stream.close()
      } catch {
        /* ignore */
      }
    }
    const client = this.client
    this.client = null
    this.quietEnd(client)
    const proxies = this.proxyClients
    this.proxyClients = []
    for (const proxy of proxies) {
      this.quietEnd(proxy)
    }
  }
}
