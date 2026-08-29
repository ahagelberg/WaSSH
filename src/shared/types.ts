/** Default SSH port */
export const DEFAULT_SSH_PORT = 22

/** Default Telnet port */
export const DEFAULT_TELNET_PORT = 23

export const CONNECTION_TYPE_SSH = 'ssh'
export const CONNECTION_TYPE_TELNET = 'telnet'
export const CONNECTION_TYPE_SERIAL = 'serial'

export type ConnectionType =
  | typeof CONNECTION_TYPE_SSH
  | typeof CONNECTION_TYPE_TELNET
  | typeof CONNECTION_TYPE_SERIAL

/** Default protocol for saved hosts and quick connect */
export const DEFAULT_CONNECTION_TYPE: ConnectionType = CONNECTION_TYPE_SSH

/** Common serial baud rates (datalist suggestions; any rate in range is allowed) */
export const SERIAL_BAUD_RATES = [
  300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600
] as const

export const SERIAL_BAUD_MIN = 300
export const SERIAL_BAUD_MAX = 3000000
export const DEFAULT_SERIAL_BAUD_RATE = 115200

export const SERIAL_DATA_BITS_5 = 5
export const SERIAL_DATA_BITS_6 = 6
export const SERIAL_DATA_BITS_7 = 7
export const SERIAL_DATA_BITS_8 = 8
export type SerialDataBits =
  | typeof SERIAL_DATA_BITS_5
  | typeof SERIAL_DATA_BITS_6
  | typeof SERIAL_DATA_BITS_7
  | typeof SERIAL_DATA_BITS_8
export const DEFAULT_SERIAL_DATA_BITS: SerialDataBits = SERIAL_DATA_BITS_8

export const SERIAL_STOP_BITS_1 = 1
export const SERIAL_STOP_BITS_1_5 = 1.5
export const SERIAL_STOP_BITS_2 = 2
export type SerialStopBits =
  | typeof SERIAL_STOP_BITS_1
  | typeof SERIAL_STOP_BITS_1_5
  | typeof SERIAL_STOP_BITS_2
export const DEFAULT_SERIAL_STOP_BITS: SerialStopBits = SERIAL_STOP_BITS_1

export const SERIAL_PARITY_NONE = 'none'
export const SERIAL_PARITY_EVEN = 'even'
export const SERIAL_PARITY_ODD = 'odd'
export const SERIAL_PARITY_MARK = 'mark'
export const SERIAL_PARITY_SPACE = 'space'
export type SerialParity =
  | typeof SERIAL_PARITY_NONE
  | typeof SERIAL_PARITY_EVEN
  | typeof SERIAL_PARITY_ODD
  | typeof SERIAL_PARITY_MARK
  | typeof SERIAL_PARITY_SPACE
export const DEFAULT_SERIAL_PARITY: SerialParity = SERIAL_PARITY_NONE

export const SERIAL_FLOW_NONE = 'none'
export const SERIAL_FLOW_RTSCTS = 'rtscts'
export const SERIAL_FLOW_XONXOFF = 'xonxoff'
export type SerialFlowControl =
  | typeof SERIAL_FLOW_NONE
  | typeof SERIAL_FLOW_RTSCTS
  | typeof SERIAL_FLOW_XONXOFF
export const DEFAULT_SERIAL_FLOW_CONTROL: SerialFlowControl = SERIAL_FLOW_NONE

export interface SerialPortInfo {
  path: string
  /** Manufacturer / friendly name when the OS provides one */
  detail: string
}

export interface SerialConfig {
  serialBaudRate: number
  serialDataBits: SerialDataBits
  serialStopBits: SerialStopBits
  serialParity: SerialParity
  serialFlowControl: SerialFlowControl
}

/** Default local bind address for tunnels */
export const DEFAULT_TUNNEL_LISTEN_HOST = '127.0.0.1'

/** Tunnel port range */
export const TUNNEL_PORT_MIN = 1
export const TUNNEL_PORT_MAX = 65535

/** Local port forward (listen here → remote dest) */
export const TUNNEL_TYPE_LOCAL = 'local'

/** Remote port forward (listen on server → local dest) */
export const TUNNEL_TYPE_REMOTE = 'remote'

/** Dynamic SOCKS proxy (listen here) */
export const TUNNEL_TYPE_DYNAMIC = 'dynamic'

export type TunnelType =
  | typeof TUNNEL_TYPE_LOCAL
  | typeof TUNNEL_TYPE_REMOTE
  | typeof TUNNEL_TYPE_DYNAMIC

/** Default X11 forwarding off */
export const DEFAULT_X11_FORWARDING = false

/** Local X server host when DISPLAY is unset */
export const DEFAULT_X11_HOST = '127.0.0.1'

/** X11 TCP base port (display N → 6000+N) */
export const X11_TCP_BASE_PORT = 6000

export interface SshTunnel {
  id: string
  enabled: boolean
  type: TunnelType
  /** Bind address for the listening side */
  listenHost: string
  listenPort: number
  /** Destination host (local/remote); unused for dynamic */
  destHost: string
  /** Destination port (local/remote); unused for dynamic */
  destPort: number
}

/** Default terminal columns/rows before first fit */
export const DEFAULT_TERM_COLS = 80
export const DEFAULT_TERM_ROWS = 24

/** Default scrollback lines */
export const DEFAULT_SCROLLBACK_LINES = 10000

/** Minimum scrollback lines */
export const SCROLLBACK_LINES_MIN = 100

/** Maximum scrollback lines */
export const SCROLLBACK_LINES_MAX = 100000

/** Default font size in px */
export const DEFAULT_FONT_SIZE_PX = 14

/** Minimum terminal font size in px */
export const FONT_SIZE_MIN_PX = 8

/** Maximum terminal font size in px */
export const FONT_SIZE_MAX_PX = 48

/** Default TERM for PTY */
export const DEFAULT_TERM_TYPE = 'xterm-256color'

/** Bundled terminal fonts (always available) */
export const BUNDLED_FONT_FAMILIES = ['JetBrains Mono', 'IBM Plex Mono'] as const

/** Default font family for terminal */
export const DEFAULT_FONT_FAMILY: string = BUNDLED_FONT_FAMILIES[0]

/** Empty host/session color: follow the active app theme */
export const THEME_COLOR_UNSET = ''

/** BEL: play the OS default sound */
export const BELL_MODE_SYSTEM = 'system'

/** BEL: invert the whole terminal briefly */
export const BELL_MODE_INVERT_WINDOW = 'invertWindow'

/** BEL: invert the current line briefly */
export const BELL_MODE_INVERT_LINE = 'invertLine'

export type BellMode =
  | typeof BELL_MODE_SYSTEM
  | typeof BELL_MODE_INVERT_WINDOW
  | typeof BELL_MODE_INVERT_LINE

/** Default BEL action */
export const DEFAULT_BELL_MODE: BellMode = BELL_MODE_SYSTEM

/** Cursor: filled cell */
export const CURSOR_STYLE_BLOCK = 'block'

/** Cursor: underline */
export const CURSOR_STYLE_UNDERLINE = 'underline'

/** Cursor: vertical bar */
export const CURSOR_STYLE_BAR = 'bar'

export type CursorStyle =
  | typeof CURSOR_STYLE_BLOCK
  | typeof CURSOR_STYLE_UNDERLINE
  | typeof CURSOR_STYLE_BAR

/** Default cursor shape */
export const DEFAULT_CURSOR_STYLE: CursorStyle = CURSOR_STYLE_BLOCK

/** Default cursor blink */
export const DEFAULT_CURSOR_BLINK = true

export type AppTheme = 'dark' | 'light'

/** Default app theme */
export const DEFAULT_THEME: AppTheme = 'dark'

/**
 * BrowserWindow background per theme.
 * Keep in sync with `--bg-app` in app.css.
 */
export const THEME_WINDOW_BACKGROUND: Record<AppTheme, string> = {
  dark: '#1b1d21',
  light: '#f3f4f6'
}

/** Mid-session reconnect: initial backoff ms */
export const RECONNECT_INITIAL_BACKOFF_MS = 1000

/** Mid-session reconnect: max backoff ms */
export const RECONNECT_MAX_BACKOFF_MS = 30000

/** Mid-session reconnect: default max attempts */
export const DEFAULT_RECONNECT_MAX_ATTEMPTS = 10

/** Wait after OS resume so the network stack is up before reconnecting */
export const WAKE_RECONNECT_DELAY_MS = 2000

/** Tab snapshot write debounce ms */
export const TAB_SNAPSHOT_DEBOUNCE_MS = 500

/** Keyboard key for cycling session tabs */
export const TAB_CYCLE_KEY = 'Tab'

/** Ctrl+Tab moves to the next session tab */
export const TAB_CYCLE_NEXT = 1

/** Ctrl+Shift+Tab moves to the previous session tab */
export const TAB_CYCLE_PREV = -1

/** Ctrl+F4 closes the active session tab */
export const TAB_CLOSE_KEY = 'F4'

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
  | 'closed'
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
  /** ssh | telnet | serial; missing in old files = ssh */
  connectionType: ConnectionType
  /** Saved host id used as SSH jump/proxy; empty = direct */
  proxyHostId: string
  /** Tab accent color (hex); empty = app theme */
  tabColor: string
  /** Terminal background (hex); empty = app theme */
  termBackground: string
  /** Terminal default text color (hex); empty = app theme */
  termForeground: string
  /** Terminal font size in px */
  fontSizePx: number
  /** Terminal font family */
  fontFamily: string
  /** Action when the remote sends BEL */
  bellMode: BellMode
  /** Cursor shape */
  cursorStyle: CursorStyle
  /** Whether the cursor blinks */
  cursorBlink: boolean
  /** Primary-buffer scrollback lines */
  scrollbackLines: number
  /** Port forwards / SOCKS tunnels for this host */
  tunnels: SshTunnel[]
  /** Forward remote X11 clients to the local display */
  x11Forwarding: boolean
  serialBaudRate: number
  serialDataBits: SerialDataBits
  serialStopBits: SerialStopBits
  serialParity: SerialParity
  serialFlowControl: SerialFlowControl
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
  /** ssh | telnet | serial; missing in old files = ssh */
  connectionType: ConnectionType
  /** Saved host id used as SSH jump/proxy; empty = direct */
  proxyHostId: string
  /** Tab accent color (hex); empty = app theme */
  tabColor: string
  /** Terminal background (hex); empty = app theme */
  termBackground: string
  /** Terminal default text color (hex); empty = app theme */
  termForeground: string
  /** Terminal font size in px */
  fontSizePx: number
  /** Terminal font family */
  fontFamily: string
  /** Action when the remote sends BEL */
  bellMode: BellMode
  /** Cursor shape */
  cursorStyle: CursorStyle
  /** Whether the cursor blinks */
  cursorBlink: boolean
  /** Primary-buffer scrollback lines */
  scrollbackLines: number
  /** Port forwards / SOCKS tunnels for this session */
  tunnels: SshTunnel[]
  /** Forward remote X11 clients to the local display */
  x11Forwarding: boolean
  serialBaudRate: number
  serialDataBits: SerialDataBits
  serialStopBits: SerialStopBits
  serialParity: SerialParity
  serialFlowControl: SerialFlowControl
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
  termType: string
  sidebarCollapsed: boolean
  windowBounds: WindowBounds | null
  theme: AppTheme
}

export const DEFAULT_SETTINGS: AppSettings = {
  reconnectOnStartup: true,
  autoReconnectOnDrop: true,
  reconnectMaxAttempts: DEFAULT_RECONNECT_MAX_ATTEMPTS,
  termType: DEFAULT_TERM_TYPE,
  sidebarCollapsed: false,
  windowBounds: null,
  theme: DEFAULT_THEME
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
  listSerialPorts: () => Promise<SerialPortInfo[]>
  beep: () => Promise<void>
  onSessionData: (cb: (tabId: string, data: string) => void) => () => void
  onSessionStatus: (cb: (ev: SessionStatusEvent) => void) => () => void
  onCycleTab: (cb: (delta: number) => void) => () => void
  onCloseActiveTab: (cb: () => void) => () => void
  onOpenPreferences: (cb: () => void) => () => void
  onHostKeyPrompt: (cb: (prompt: HostKeyPrompt) => void) => () => void
  onSavePasswordPrompt: (cb: (prompt: SavePasswordPrompt) => void) => () => void
}

declare global {
  interface Window {
    wassh: WasshApi
  }
}
