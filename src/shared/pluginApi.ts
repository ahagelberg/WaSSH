export type { SessionStatus } from './types'

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

/** Host-managed side connection kinds (serial reserved for future). */
export type SideConnectionKind = 'ssh-exec' | 'ssh-shell' | 'tcp' | 'serial'

export type PluginSettingsFieldType =
  | 'boolean'
  | 'number'
  | 'string'
  | 'select'
  | 'stringList'
  | 'macroList'

export interface PluginSettingsSelectOption {
  value: string
  label: string
}

export interface PluginMacroButton {
  id: string
  label: string
  text: string
  /** e.g. "Ctrl+Shift+1" - empty means no hotkey. */
  hotkey: string
  /** Macro pad group id; empty or absent means ungrouped. */
  groupId?: string
}

export interface PluginSettingsField {
  key: string
  label: string
  type: PluginSettingsFieldType
  default: unknown
  description?: string
  /** When true, string fields use a password input in settings UI. */
  secret?: boolean
  /** When type is select, the available dropdown options. */
  options?: PluginSettingsSelectOption[]
}

export interface PluginToolbarContribution {
  label: string
}

export interface PluginViewContribution {
  id: string
  placement: PluginViewPlacement
  title?: string
}

export interface PluginTerminalFileDropContribution {
  /** Accessible description shown by the host while a file is dragged over the terminal. */
  label?: string
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
    /** Options dialog section heading (app-wide settings). */
    settingsHeading?: string
    /** App-wide settings. */
    settingsSchema?: PluginSettingsField[]
    /** Where app-wide settings are edited. Defaults to the options dialog. */
    settingsPresentation?: 'options' | 'view'
    /** Host dialog section heading for per-host plugin settings. */
    hostSettingsHeading?: string
    /** Per-host settings. */
    hostSettingsSchema?: PluginSettingsField[]
    views?: PluginViewContribution[]
    terminalFileDrop?: PluginTerminalFileDropContribution
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
  /** ssh-exec command. */
  command?: string
  /** ssh-shell: open an isolated duplicate SSH client. */
  duplicate?: boolean
  /** TCP destination. */
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
  if (!schema || schema.length === 0) {
    return defaults
  }
  if (!stored || typeof stored !== 'object' || Array.isArray(stored)) {
    return defaults
  }
  const src = stored as Record<string, unknown>
  const out = { ...defaults }
  for (const field of schema) {
    if (Object.prototype.hasOwnProperty.call(src, field.key)) {
      out[field.key] = src[field.key]
    }
  }
  return out
}

export function mergePluginSessionSettings(
  manifest: Pick<PluginManifest, 'contributes'> | undefined,
  appStored: unknown,
  hostStored: unknown
): Record<string, unknown> {
  const app = mergePluginSettings(manifest?.contributes.settingsSchema, appStored)
  const hostDefaults = defaultPluginSettingsFromSchema(manifest?.contributes.hostSettingsSchema)
  const hostSrc =
    hostStored && typeof hostStored === 'object' && !Array.isArray(hostStored)
      ? (hostStored as Record<string, unknown>)
      : {}

  const result: Record<string, unknown> = { ...app }
  for (const [key, value] of Object.entries(hostDefaults)) {
    if (!Object.prototype.hasOwnProperty.call(result, key)) {
      result[key] = value
    }
  }
  for (const field of manifest?.contributes.hostSettingsSchema ?? []) {
    if (Object.prototype.hasOwnProperty.call(hostSrc, field.key)) {
      result[field.key] = hostSrc[field.key]
    }
  }
  return result
}

/** Normalize HostProfile and ConnectionParams plugin settings maps. */
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
