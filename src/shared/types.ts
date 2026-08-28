/** Default SSH port */
export const DEFAULT_SSH_PORT = 22

/** Default terminal columns/rows before first fit */
export const DEFAULT_TERM_COLS = 80
export const DEFAULT_TERM_ROWS = 24

/** Default scrollback lines (PuTTY-like) */
export const DEFAULT_SCROLLBACK_LINES = 10000

/** Default font size in px */
export const DEFAULT_FONT_SIZE_PX = 14

/** Default TERM for PTY */
export const DEFAULT_TERM_TYPE = 'xterm-256color'

/** Default font family for terminal */
export const DEFAULT_FONT_FAMILY = 'Cascadia Mono, Consolas, monospace'

/** Mid-session reconnect: initial backoff ms */
export const RECONNECT_INITIAL_BACKOFF_MS = 1000

/** Mid-session reconnect: max backoff ms */
export const RECONNECT_MAX_BACKOFF_MS = 30000

/** Mid-session reconnect: default max attempts */
export const DEFAULT_RECONNECT_MAX_ATTEMPTS = 10

/** Tab snapshot write debounce ms */
export const TAB_SNAPSHOT_DEBOUNCE_MS = 500

/** Default main window width px */
export const DEFAULT_WINDOW_WIDTH = 1280

/** Default main window height px */
export const DEFAULT_WINDOW_HEIGHT = 800

/** Minimum main window width px */
export const DEFAULT_WINDOW_MIN_WIDTH = 800

/** Minimum main window height px */
export const DEFAULT_WINDOW_MIN_HEIGHT = 500

/** Debounce for persisting window bounds ms */
export const WINDOW_BOUNDS_DEBOUNCE_MS = 500

/** Min visible px on screen when restoring window position */
export const WINDOW_VISIBLE_MARGIN_PX = 50

export interface WindowBounds {
  x: number
  y: number
  width: number
  height: number
  maximized: boolean
}

export type SessionStatus =
  | 'idle'
  | 'connecting'
  | 'awaiting_host_key'
  | 'authenticating'
  | 'connected'
  | 'reconnecting'
  | 'disconnected'
  | 'failed'

export type AuthMethod = 'password' | 'privateKey' | 'none'

export interface HostProfile {
  id: string
  name: string
  host: string
  port: number
  username: string
  /** Vault id for password; empty if unset */
  passwordVaultId: string
  privateKeyPath: string
  /** Vault id for key passphrase; empty if unset */
  passphraseVaultId: string
  authMethod: AuthMethod
  /** Saved host id used as SSH jump/proxy; empty = direct */
  proxyHostId: string
}

/** Connection params for a tab (saved host and/or quick-connect / overrides) */
export interface ConnectionParams {
  hostId: string | null
  name: string
  host: string
  port: number
  username: string
  passwordVaultId: string
  privateKeyPath: string
  passphraseVaultId: string
  authMethod: AuthMethod
  /** Saved host id used as SSH jump/proxy; empty = direct */
  proxyHostId: string
  /** Session-local password not yet vaulted (ephemeral) */
  ephemeralPassword: string
  ephemeralPassphrase: string
}

export interface TabSnapshot {
  id: string
  connection: ConnectionParams
  active: boolean
}

export interface AppSettings {
  reconnectOnStartup: boolean
  autoReconnectOnDrop: boolean
  reconnectMaxAttempts: number
  scrollbackLines: number
  fontSizePx: number
  fontFamily: string
  termType: string
  sidebarCollapsed: boolean
  windowBounds: WindowBounds | null
}

export const DEFAULT_SETTINGS: AppSettings = {
  reconnectOnStartup: true,
  autoReconnectOnDrop: true,
  reconnectMaxAttempts: DEFAULT_RECONNECT_MAX_ATTEMPTS,
  scrollbackLines: DEFAULT_SCROLLBACK_LINES,
  fontSizePx: DEFAULT_FONT_SIZE_PX,
  fontFamily: DEFAULT_FONT_FAMILY,
  termType: DEFAULT_TERM_TYPE,
  sidebarCollapsed: false,
  windowBounds: null
}

export interface KnownHostEntry {
  host: string
  port: number
  /** algorithm e.g. ssh-ed25519 */
  keyType: string
  /** base64 public key / fingerprint material */
  fingerprint: string
}

export interface HostKeyPrompt {
  tabId: string
  host: string
  port: number
  keyType: string
  fingerprint: string
  reason: 'unknown' | 'mismatch'
}

export interface SavePasswordPrompt {
  tabId: string
  hasHostProfile: boolean
}

export interface SessionStatusEvent {
  tabId: string
  status: SessionStatus
  message?: string
}

export interface ConnectRequest {
  tabId: string
  connection: ConnectionParams
  cols: number
  rows: number
  termType: string
}

export type HostKeyDecision = 'accept' | 'reject'
export type SavePasswordDecision = 'save' | 'skip' | 'save_as_host'

export interface WasshApi {
  getSettings: () => Promise<AppSettings>
  setSettings: (partial: Partial<AppSettings>) => Promise<AppSettings>
  listHosts: () => Promise<HostProfile[]>
  saveHost: (host: HostProfile) => Promise<HostProfile>
  deleteHost: (id: string) => Promise<void>
  getTabSnapshot: () => Promise<TabSnapshot[]>
  saveTabSnapshot: (tabs: TabSnapshot[]) => Promise<void>
  setSecret: (vaultId: string, value: string) => Promise<void>
  getSecret: (vaultId: string) => Promise<string | null>
  deleteSecret: (vaultId: string) => Promise<void>
  connect: (req: ConnectRequest) => Promise<void>
  disconnect: (tabId: string) => Promise<void>
  write: (tabId: string, data: string) => Promise<void>
  resize: (tabId: string, cols: number, rows: number) => Promise<void>
  respondHostKey: (tabId: string, decision: HostKeyDecision) => Promise<void>
  respondSavePassword: (
    tabId: string,
    decision: SavePasswordDecision,
    hostName?: string
  ) => Promise<void>
  pickPrivateKeyFile: () => Promise<string | null>
  onSessionData: (cb: (tabId: string, data: string) => void) => () => void
  onSessionStatus: (cb: (ev: SessionStatusEvent) => void) => () => void
  onHostKeyPrompt: (cb: (prompt: HostKeyPrompt) => void) => () => void
  onSavePasswordPrompt: (cb: (prompt: SavePasswordPrompt) => void) => () => void
}

declare global {
  interface Window {
    wassh: WasshApi
  }
}
