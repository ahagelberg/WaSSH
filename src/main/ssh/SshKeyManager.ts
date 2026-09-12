import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'fs'
import { homedir } from 'os'
import { join } from 'path'
import { Readable } from 'stream'
import { Client, ConnectConfig } from 'ssh2'
import * as ssh2 from 'ssh2'
import { hostToConnection, resolveProxyChain } from '../../shared/connection'
import {
  DEFAULT_SERIAL_BAUD_RATE,
  DEFAULT_SERIAL_DATA_BITS,
  DEFAULT_SERIAL_FLOW_CONTROL,
  DEFAULT_SERIAL_PARITY,
  DEFAULT_SERIAL_STOP_BITS,
  type ConnectionParams,
  type HostProfile,
  type SshKeyDeployOptions,
  type SshKeyDeployResult,
  type SshKeyGenerateOptions,
  type SshKeyGenerateResult,
  type SshKeyInfo,
  type SshKeyLocalItem,
  type SshKeyRetrieveOptions,
  type SshKeyRetrieveResult
} from '../../shared/types'
import type { CredentialVault } from '../store/credentialVault'
import type { SessionStore } from '../store/sessionStore'

/** Directory permission mode: 0700 (rwx------) */
const SSH_DIR_FILE_MODE = 0o700
/** Private key permission mode: 0600 (rw-------) */
const SSH_PRIVATE_KEY_FILE_MODE = 0o600
/** Public key permission mode: 0644 (rw-r--r--) */
const SSH_PUBLIC_KEY_FILE_MODE = 0o644

/** RSA key size in bits */
const RSA_MODULUS_BITS = 4096
/** Encryption cipher for passphrase-protected keys */
const KEY_ENCRYPTION_CIPHER = 'aes256-ctr'
/** Default comment appended to generated public keys */
const DEFAULT_KEY_COMMENT = 'wassh'

/** SSH connection timeout in ms */
const SSH_CONNECT_TIMEOUT_MS = 15000
/** Target loopback address for tunnel forwarding */
const FORWARD_BIND_HOST = '127.0.0.1'
/** Ephemeral port for tunnel forwarding */
const FORWARD_BIND_PORT = 0

/** Suffix for public key files */
const PUB_EXT = '.pub'
/** Non-key files in ~/.ssh to ignore when listing */
const IGNORED_SSH_FILES = new Set([
  'authorized_keys',
  'authorized_keys2',
  'config',
  'environment',
  'known_hosts',
  'known_hosts.old'
])

/** Candidate private key filenames to look for on remote hosts */
const REMOTE_KEY_CANDIDATES = ['id_ed25519', 'id_rsa', 'id_ecdsa']

/** Marker delimiter used in remote exec output parsing */
const REMOTE_DELIM_NAME = '===KEY_NAME==='
const REMOTE_DELIM_PRIV = '===KEY_PRIV==='
const REMOTE_DELIM_PUB = '===KEY_PUB==='
const REMOTE_DELIM_END = '===KEY_END==='

export class SshKeyManager {
  constructor(
    private vault: CredentialVault,
    private sessionStore: SessionStore
  ) {}

  getDefaultSshDir(): string {
    return join(homedir(), '.ssh')
  }

  ensureSshDir(): string {
    const dir = this.getDefaultSshDir()
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true, mode: SSH_DIR_FILE_MODE })
    }
    return dir
  }

  getSshKeyInfo(privateKeyPath: string, passphrase?: string): SshKeyInfo {
    if (!privateKeyPath || !existsSync(privateKeyPath)) {
      return { exists: false }
    }

    const pubPath = `${privateKeyPath}${PUB_EXT}`
    let pubContent: string | undefined
    if (existsSync(pubPath)) {
      try {
        pubContent = readFileSync(pubPath, 'utf8').trim()
      } catch {
        // Fall through to parse private key
      }
    }

    let privBuffer: Buffer
    try {
      privBuffer = readFileSync(privateKeyPath)
    } catch {
      return { exists: true, publicKey: pubContent }
    }

    const parsed = ssh2.utils.parseKey(privBuffer, passphrase)
    if (parsed instanceof Error) {
      const isEncrypted = parsed.message.toLowerCase().includes('passphrase') ||
        parsed.message.toLowerCase().includes('encrypted')
      return {
        exists: true,
        hasPassphrase: isEncrypted,
        publicKey: pubContent
      }
    }

    let publicKey = pubContent
    if (!publicKey && parsed.type && typeof parsed.getPublicSSH === 'function') {
      const pubBuf = parsed.getPublicSSH()
      const commentPart = parsed.comment ? ` ${parsed.comment}` : ''
      publicKey = `${parsed.type} ${pubBuf.toString('base64')}${commentPart}`
    }

    return {
      exists: true,
      hasPassphrase: false,
      keyType: parsed.type,
      comment: parsed.comment,
      publicKey
    }
  }

  listLocalSshKeys(): SshKeyLocalItem[] {
    const dir = this.getDefaultSshDir()
    if (!existsSync(dir)) {
      return []
    }

    let entries: string[]
    try {
      entries = readdirSync(dir)
    } catch {
      return []
    }

    const result: SshKeyLocalItem[] = []
    for (const name of entries) {
      if (name.endsWith(PUB_EXT) || IGNORED_SSH_FILES.has(name)) {
        continue
      }
      const fullPath = join(dir, name)
      const info = this.getSshKeyInfo(fullPath)
      if (info.exists) {
        result.push({
          name,
          path: fullPath,
          type: info.keyType || (name.includes('ed25519') ? 'ssh-ed25519' : 'ssh-rsa'),
          publicKey: info.publicKey
        })
      }
    }
    return result
  }

  generateSshKey(options: SshKeyGenerateOptions): SshKeyGenerateResult {
    const dir = this.ensureSshDir()
    const baseName = options.filename?.trim() || (options.type === 'rsa' ? 'id_rsa' : 'id_ed25519')
    const privPath = join(dir, baseName)
    const pubPath = `${privPath}${PUB_EXT}`

    const genOpts: Record<string, unknown> = {}
    if (options.type === 'rsa') {
      genOpts.bits = RSA_MODULUS_BITS
    }
    if (options.passphrase) {
      genOpts.cipher = KEY_ENCRYPTION_CIPHER
      genOpts.passphrase = options.passphrase
    }

    const keyPair = ssh2.utils.generateKeyPairSync(options.type, genOpts)
    let publicContent = keyPair.public.trim()
    const comment = options.comment?.trim() || DEFAULT_KEY_COMMENT
    if (!publicContent.endsWith(` ${comment}`)) {
      publicContent = `${publicContent} ${comment}`
    }

    writeFileSync(privPath, keyPair.private, { mode: SSH_PRIVATE_KEY_FILE_MODE })
    writeFileSync(pubPath, `${publicContent}\n`, { mode: SSH_PUBLIC_KEY_FILE_MODE })

    return {
      privateKeyPath: privPath,
      publicKeyPath: pubPath,
      publicKey: publicContent
    }
  }

  async deploySshKey(options: SshKeyDeployOptions): Promise<SshKeyDeployResult> {
    const cleanKey = options.publicKey.trim()
    if (!cleanKey) {
      return { success: false, message: 'No public key provided to deploy.' }
    }

    let client: Client | null = null
    let proxies: Client[] = []
    try {
      const resolved = await this.connectToTarget({
        host: options.host,
        port: options.port,
        username: options.username,
        password: options.password,
        passwordVaultId: options.passwordVaultId,
        proxyHostId: options.proxyHostId
      })
      client = resolved.client
      proxies = resolved.proxies

      await this.runRemoteDeploy(client, cleanKey)
      return {
        success: true,
        message: 'Public key successfully added to remote ~/.ssh/authorized_keys.'
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      return { success: false, message: `Deployment failed: ${msg}` }
    } finally {
      if (client) {
        client.end()
      }
      for (const p of proxies) {
        p.end()
      }
    }
  }

  async retrieveSshKey(options: SshKeyRetrieveOptions): Promise<SshKeyRetrieveResult> {
    let client: Client | null = null
    let proxies: Client[] = []
    try {
      const resolved = await this.connectToTarget({
        host: options.host,
        port: options.port,
        username: options.username,
        password: options.password,
        passwordVaultId: options.passwordVaultId,
        proxyHostId: options.proxyHostId
      })
      client = resolved.client
      proxies = resolved.proxies

      const remoteData = await this.runRemoteRetrieve(client)
      const dir = this.ensureSshDir()
      const sanitizedHost = options.host.replace(/[^a-zA-Z0-9._-]/g, '_')
      const localBase = `${remoteData.name}_${sanitizedHost}`
      const localPrivPath = join(dir, localBase)
      const localPubPath = `${localPrivPath}${PUB_EXT}`

      writeFileSync(localPrivPath, `${remoteData.privateKey}\n`, { mode: SSH_PRIVATE_KEY_FILE_MODE })
      if (remoteData.publicKey) {
        writeFileSync(localPubPath, `${remoteData.publicKey}\n`, { mode: SSH_PUBLIC_KEY_FILE_MODE })
      }

      return {
        success: true,
        message: `Successfully downloaded remote key "${remoteData.name}" as "${localBase}".`,
        privateKeyPath: localPrivPath,
        publicKey: remoteData.publicKey,
        keyType: remoteData.name.includes('ed25519') ? 'ssh-ed25519' : 'ssh-rsa'
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      return { success: false, message: `Retrieval failed: ${msg}` }
    } finally {
      if (client) {
        client.end()
      }
      for (const p of proxies) {
        p.end()
      }
    }
  }

  private async connectToTarget(options: {
    host: string
    port: number
    username: string
    password?: string
    passwordVaultId?: string
    proxyHostId?: string
  }): Promise<{ client: Client; proxies: Client[] }> {
    const hosts = this.sessionStore.listHosts()
    const targetProfile: HostProfile = {
      id: 'ssh-key-mgr-target',
      name: options.host,
      host: options.host,
      port: options.port,
      username: options.username,
      passwordVaultId: options.passwordVaultId || '',
      privateKeyPath: '',
      passphraseVaultId: '',
      authMethod: 'password',
      connectionType: 'ssh',
      proxyHostId: options.proxyHostId || '',
      tabColor: '',
      termBackground: '',
      termForeground: '',
      fontSizePx: null,
      fontFamily: '',
      bellMode: null,
      cursorStyle: null,
      cursorBlink: null,
      scrollbackLines: null,
      tunnels: [],
      x11Forwarding: false,
      serialBaudRate: DEFAULT_SERIAL_BAUD_RATE,
      serialDataBits: DEFAULT_SERIAL_DATA_BITS,
      serialStopBits: DEFAULT_SERIAL_STOP_BITS,
      serialParity: DEFAULT_SERIAL_PARITY,
      serialFlowControl: DEFAULT_SERIAL_FLOW_CONTROL,
      pluginSettings: {},
      reconnectMode: 'none',
      openInScreen: false,
      remoteSessionKind: 'screen',
      screenSessionName: '',
      screenBusyHandling: 'share',
      tags: []
    }
    const targetParams: ConnectionParams = {
      ...hostToConnection(targetProfile),
      ephemeralPassword: options.password || ''
    }

    const chain = resolveProxyChain(targetParams, hosts)
    const proxies: Client[] = []
    let sock: Readable | undefined

    for (let i = 0; i < chain.length - 1; i++) {
      const hop = chain[i]
      const next = chain[i + 1]
      const hopConfig = this.buildConnectConfig(hop)
      const hopClient = await this.connectClient(hopConfig, sock)
      proxies.push(hopClient)
      sock = await this.forwardThrough(hopClient, next.host, next.port)
    }

    const targetConfig = this.buildConnectConfig(chain[chain.length - 1])
    if (options.password) {
      targetConfig.password = options.password
    }
    const client = await this.connectClient(targetConfig, sock)
    return { client, proxies }
  }

  private buildConnectConfig(params: ConnectionParams): ConnectConfig {
    const config: ConnectConfig = {
      host: params.host,
      port: params.port,
      username: params.username || 'root',
      readyTimeout: SSH_CONNECT_TIMEOUT_MS
    }

    let password = params.ephemeralPassword
    if (!password && params.passwordVaultId) {
      password = this.vault.get(params.passwordVaultId) || ''
    }
    if (password) {
      config.password = password
    }

    if (params.privateKeyPath && existsSync(params.privateKeyPath)) {
      try {
        config.privateKey = readFileSync(params.privateKeyPath)
        let passphrase = params.ephemeralPassphrase
        if (!passphrase && params.passphraseVaultId) {
          passphrase = this.vault.get(params.passphraseVaultId) || ''
        }
        if (passphrase) {
          config.passphrase = passphrase
        }
      } catch {
        // Fall back to password
      }
    }

    return config
  }

  private connectClient(config: ConnectConfig, sock?: Readable): Promise<Client> {
    return new Promise((resolve, reject) => {
      const client = new Client()
      let settled = false

      const timer = setTimeout(() => {
        cleanup()
        client.end()
        reject(new Error(`Connection to ${config.host}:${config.port} timed out`))
      }, SSH_CONNECT_TIMEOUT_MS)

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
        reject(new Error(`Connection closed (${config.host})`))
      }
      const cleanup = (): void => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        client.removeListener('ready', onReady)
        client.removeListener('error', onError)
        client.removeListener('close', onClose)
      }

      client.once('ready', onReady)
      client.once('error', onError)
      client.once('close', onClose)

      client.connect(sock ? { ...config, sock } : config)
    })
  }

  private forwardThrough(client: Client, host: string, port: number): Promise<Readable> {
    return new Promise((resolve, reject) => {
      client.forwardOut(FORWARD_BIND_HOST, FORWARD_BIND_PORT, host, port, (err, stream) => {
        if (err) {
          reject(err)
          return
        }
        resolve(stream)
      })
    })
  }

  private runRemoteDeploy(client: Client, publicKey: string): Promise<void> {
    return new Promise((resolve, reject) => {
      const cmd =
        "sh -c 'mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys; KEY=$(cat); if ! grep -qF \"$KEY\" ~/.ssh/authorized_keys 2>/dev/null; then printf \"%s\\n\" \"$KEY\" >> ~/.ssh/authorized_keys; fi'"
      client.exec(cmd, (err, stream) => {
        if (err) {
          reject(err)
          return
        }
        let errBuf = ''
        stream.stderr.on('data', (d: Buffer) => {
          errBuf += d.toString('utf8')
        })
        stream.on('close', (code: number) => {
          if (code === 0) {
            resolve()
          } else {
            reject(new Error(errBuf.trim() || `Remote deploy failed with exit code ${code}`))
          }
        })
        stream.write(publicKey)
        stream.end()
      })
    })
  }

  private runRemoteRetrieve(
    client: Client
  ): Promise<{ name: string; privateKey: string; publicKey?: string }> {
    return new Promise((resolve, reject) => {
      const candidates = REMOTE_KEY_CANDIDATES.join(' ')
      const cmd = `sh -c 'for k in ${candidates}; do if [ -f "$HOME/.ssh/$k" ]; then echo "${REMOTE_DELIM_NAME}$k"; echo "${REMOTE_DELIM_PRIV}"; cat "$HOME/.ssh/$k"; echo "${REMOTE_DELIM_PUB}"; if [ -f "$HOME/.ssh/$k.pub" ]; then cat "$HOME/.ssh/$k.pub"; fi; echo "${REMOTE_DELIM_END}"; exit 0; fi; done; exit 1'`

      client.exec(cmd, (err, stream) => {
        if (err) {
          reject(err)
          return
        }
        let outBuf = ''
        let errBuf = ''
        stream.on('data', (d: Buffer) => {
          outBuf += d.toString('utf8')
        })
        stream.stderr.on('data', (d: Buffer) => {
          errBuf += d.toString('utf8')
        })
        stream.on('close', (code: number) => {
          if (code !== 0 || !outBuf.includes(REMOTE_DELIM_NAME)) {
            reject(new Error(errBuf.trim() || 'No standard SSH keys found in remote ~/.ssh/'))
            return
          }
          const nameIndex = outBuf.indexOf(REMOTE_DELIM_NAME) + REMOTE_DELIM_NAME.length
          const privIndex = outBuf.indexOf(REMOTE_DELIM_PRIV)
          const pubIndex = outBuf.indexOf(REMOTE_DELIM_PUB)
          const endIndex = outBuf.indexOf(REMOTE_DELIM_END)

          if (privIndex === -1 || pubIndex === -1 || endIndex === -1) {
            reject(new Error('Failed to parse remote key output'))
            return
          }

          const name = outBuf.substring(nameIndex, privIndex).trim()
          const priv = outBuf
            .substring(privIndex + REMOTE_DELIM_PRIV.length, pubIndex)
            .trim()
          const pub = outBuf.substring(pubIndex + REMOTE_DELIM_PUB.length, endIndex).trim()

          resolve({
            name,
            privateKey: priv,
            publicKey: pub || undefined
          })
        })
      })
    })
  }
}
