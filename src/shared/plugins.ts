/** Built-in plugin ids */
export const PLUGIN_ID_SERVER_MONITOR = 'server-monitor'
export const PLUGIN_ID_SCRATCHPAD = 'scratchpad'
export const PLUGIN_ID_MACRO_PAD = 'macro-pad'
export const PLUGIN_ID_MQTT_EXPLORER = 'mqtt-explorer'

export const DEFAULT_ENABLED_PLUGINS: string[] = [
  PLUGIN_ID_SERVER_MONITOR,
  PLUGIN_ID_SCRATCHPAD,
  PLUGIN_ID_MACRO_PAD,
  PLUGIN_ID_MQTT_EXPLORER
]

export type PluginActivation = 'manual' | 'auto'
export type PluginSource = 'builtin' | 'external'

export type PluginViewPlacement =
  | 'overlay'
  | 'split-left'
  | 'split-right'
  | 'split-top'
  | 'split-bottom'

export const PLUGIN_VIEW_PLACEMENTS: PluginViewPlacement[] = [
  'split-left',
  'split-right',
  'split-top',
  'split-bottom',
  'overlay'
]

export const PLUGIN_VIEW_PLACEMENT_LABELS: Record<PluginViewPlacement, string> = {
  'split-left': 'Left',
  'split-right': 'Right',
  'split-top': 'Top',
  'split-bottom': 'Bottom',
  overlay: 'Overlay'
}

export function isPluginViewPlacement(value: unknown): value is PluginViewPlacement {
  return (
    value === 'overlay' ||
    value === 'split-left' ||
    value === 'split-right' ||
    value === 'split-top' ||
    value === 'split-bottom'
  )
}

export type StreamMode = 'observe' | 'intercept'

export type StreamDirection = 'inbound' | 'outbound'

/** Host-managed side connection kinds (serial reserved for future) */
export type SideConnectionKind = 'ssh-exec' | 'ssh-shell' | 'tcp' | 'serial'

export type PluginSettingsFieldType =
  | 'boolean'
  | 'number'
  | 'string'
  | 'stringList'
  | 'macroList'

export interface PluginMacroButton {
  id: string
  label: string
  text: string
  /** e.g. "Ctrl+Shift+1" — empty = no hotkey */
  hotkey: string
}

export interface PluginSettingsField {
  key: string
  label: string
  type: PluginSettingsFieldType
  default: unknown
  description?: string
  /** When true, string fields use a password input in settings UI */
  secret?: boolean
}

export interface PluginToolbarContribution {
  label: string
}

export interface PluginViewContribution {
  id: string
  placement: PluginViewPlacement
  title?: string
}

export interface PluginManifest {
  id: string
  name: string
  version: string
  description: string
  activation: PluginActivation
  source: PluginSource
  contributes: {
    toolbar?: PluginToolbarContribution
    /** Options dialog section heading (app-wide settings) */
    settingsHeading?: string
    /** App-wide settings (Options dialog) */
    settingsSchema?: PluginSettingsField[]
    /** Host dialog section heading for per-host plugin settings */
    hostSettingsHeading?: string
    /** Per-host settings (Host / session settings dialog) */
    hostSettingsSchema?: PluginSettingsField[]
    views?: PluginViewContribution[]
    /** Future: menu contributions */
  }
}

export interface PluginListItem extends PluginManifest {
  enabled: boolean
}

export interface PluginActiveStateEvent {
  tabId: string
  pluginId: string
  active: boolean
}

export interface PluginMessageEvent {
  tabId: string
  pluginId: string
  payload: unknown
}

export interface SideConnectionOpenRequest {
  kind: SideConnectionKind
  /** ssh-exec command */
  command?: string
  /** ssh-shell: open an isolated duplicate SSH client */
  duplicate?: boolean
  /** tcp destination */
  host?: string
  port?: number
}

export interface SideConnectionOpened {
  connectionId: string
}

export interface SideConnectionDataEvent {
  connectionId: string
  data: string
}

export interface SideConnectionClosedEvent {
  connectionId: string
  error?: string
}

/** Default poll interval for server monitor (ms) */
export const SERVER_MONITOR_DEFAULT_INTERVAL_MS = 1000

/** Minimum poll interval for server monitor (ms) */
export const SERVER_MONITOR_MIN_INTERVAL_MS = 500

/** Structured stats pushed from the server-monitor main module to the UI */
export interface ServerMonitorSnapshot {
  updatedAt: number
  hostname: string
  uptimeSec: number
  load1: number
  load5: number
  load15: number
  /** 0–100; null until two CPU samples exist */
  cpuPercent: number | null
  memTotalBytes: number
  memUsedBytes: number
  memAvailableBytes: number
  swapTotalBytes: number
  swapUsedBytes: number
  diskTotalBytes: number
  diskUsedBytes: number
  error?: string
}

/** Default MQTT broker host on the remote machine */
export const MQTT_EXPLORER_DEFAULT_HOST = '127.0.0.1'

/** Default plain MQTT port */
export const MQTT_EXPLORER_DEFAULT_PORT = 1883

/** Max retained history entries per topic in the UI */
export const MQTT_EXPLORER_HISTORY_LIMIT = 100

/** Brief highlight duration when a topic receives a message (ms) */
export const MQTT_EXPLORER_BLINK_MS = 500

export type MqttExplorerStatusState =
  | 'idle'
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'unavailable'
  | 'error'

export type MqttExplorerErrorKind =
  | 'not_ssh'
  | 'no_broker'
  | 'auth_failed'
  | 'unreachable'
  | 'other'

/** Main → renderer status event */
export interface MqttExplorerStatusPayload {
  type: 'status'
  state: MqttExplorerStatusState
  reason?: string
  errorKind?: MqttExplorerErrorKind
}

/** Main → renderer message event */
export interface MqttExplorerMessagePayload {
  type: 'message'
  topic: string
  /** UTF-8 text when binary is false */
  payloadText?: string
  /** Base64 when binary is true */
  payloadBase64?: string
  binary: boolean
  qos: 0 | 1 | 2
  retain: boolean
  timestamp: number
}

/** Renderer → main */
export type MqttExplorerRendererMessage =
  | {
      type: 'publish'
      topic: string
      payloadBase64: string
      qos: 0 | 1 | 2
      retain: boolean
    }
  | { type: 'reconnect' }

export function defaultPluginSettingsFromSchema(
  schema: PluginSettingsField[] | undefined
): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  if (!schema) {
    return out
  }
  for (const field of schema) {
    out[field.key] = field.default
  }
  return out
}

export function mergePluginSettings(
  schema: PluginSettingsField[] | undefined,
  stored: unknown
): Record<string, unknown> {
  const defaults = defaultPluginSettingsFromSchema(schema)
  if (!stored || typeof stored !== 'object' || Array.isArray(stored)) {
    return defaults
  }
  return { ...defaults, ...(stored as Record<string, unknown>) }
}

/**
 * Merge app-wide + per-host plugin settings for a session.
 * Host keys override app keys when both schemas define the same key (they should not).
 */
export function mergePluginSessionSettings(
  manifest: Pick<PluginManifest, 'contributes'> | undefined,
  appStored: unknown,
  hostStored: unknown
): Record<string, unknown> {
  const app = mergePluginSettings(manifest?.contributes.settingsSchema, appStored)
  const host = mergePluginSettings(manifest?.contributes.hostSettingsSchema, hostStored)
  return { ...app, ...host }
}

/** Normalize HostProfile / ConnectionParams pluginSettings maps */
export function normalizeHostPluginSettings(
  raw: unknown
): Record<string, Record<string, unknown>> {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return {}
  }
  const out: Record<string, Record<string, unknown>> = {}
  for (const [pluginId, value] of Object.entries(raw as Record<string, unknown>)) {
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      out[pluginId] = { ...(value as Record<string, unknown>) }
    }
  }
  return out
}
