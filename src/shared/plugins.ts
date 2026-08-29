/** Built-in plugin ids */
export const PLUGIN_ID_SERVER_MONITOR = 'server-monitor'
export const PLUGIN_ID_SCRATCHPAD = 'scratchpad'
export const PLUGIN_ID_MACRO_PAD = 'macro-pad'

export const DEFAULT_ENABLED_PLUGINS: string[] = [
  PLUGIN_ID_SERVER_MONITOR,
  PLUGIN_ID_SCRATCHPAD,
  PLUGIN_ID_MACRO_PAD
]

export type PluginActivation = 'manual' | 'auto'
export type PluginSource = 'builtin' | 'external'

export type PluginViewPlacement =
  | 'overlay'
  | 'split-left'
  | 'split-right'
  | 'split-top'
  | 'split-bottom'

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
    /** Options dialog section heading */
    settingsHeading?: string
    settingsSchema?: PluginSettingsField[]
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
export const SERVER_MONITOR_DEFAULT_INTERVAL_MS = 5000

/** Minimum poll interval for server monitor (ms) */
export const SERVER_MONITOR_MIN_INTERVAL_MS = 2000

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
