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
  | 'textArea'
  | 'directory'
  | 'select'
  | 'stringList'
  | 'commandList'
  /** Boolean master toggle with nested `children` fields. */
  | 'group'
  /** Trinary allow/deny/ask toggle; see `PluginPermissionDecision`. */
  | 'permission'

export interface PluginSettingsSelectOption {
  value: string
  label: string
}

/** Trinary decision for a single permission-type settings field. */
export type PluginPermissionDecision = 'allow' | 'deny' | 'ask'

/** JSON-schema-ish parameter shape, matching the AI Agent's tool-call format. */
export interface PluginApiParameterSchema {
  type: 'object'
  properties: Record<string, unknown>
  required?: string[]
}

/** One callable method a plugin exposes to other plugins via `ctx.callPluginApi`. */
export interface PluginApiMethod {
  name: string
  label?: string
  description: string
  parameters?: PluginApiParameterSchema
  /** Default permission once this method's group is enabled. Defaults to 'allow'. */
  defaultPermission?: PluginPermissionDecision
}

export interface PluginApiContribution {
  methods: PluginApiMethod[]
}

/** Declared API methods of one other plugin, as seen by `ctx.listPluginApis()`. */
export interface PluginApiListing {
  pluginId: string
  pluginName: string
  methods: PluginApiMethod[]
}

export interface PluginCommand {
  id: string
  label: string
  text: string
  /** e.g. "Ctrl+Shift+1" - empty means no hotkey. */
  hotkey: string
  /** Macro pad group id; empty or absent means ungrouped. */
  groupId?: string
}

/** Neutral remote filesystem entry exposed to plugins by host file APIs. */
export type RemoteFileEntryType = 'file' | 'directory' | 'symlink' | 'other'

export interface RemoteFileEntry {
  name: string
  path: string
  type: RemoteFileEntryType
  size: number
  mode: number
  modeSymbolic: string
  mtime: number
  uid?: number
  gid?: number
}

export type RemoteFileErrorKind =
  | 'not_ssh'
  | 'not_found'
  | 'permission'
  | 'not_dir'
  | 'exists'
  | 'name_in_use'
  | 'io'
  | 'connection'
  | 'cancelled'
  | 'other'

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
  /** When type is 'group', the nested fields shown under the master toggle. */
  children?: PluginSettingsField[]
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
    /** Methods this plugin exposes to other plugins via `ctx.callPluginApi`. */
    api?: PluginApiContribution
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

/** Flatten a schema's fields and any nested `group` children into one flat list.
 * Recurses to arbitrary depth - a `group`'s `children` may themselves contain
 * `group` fields, nested as many levels deep as needed. */
export function flattenFields(schema: PluginSettingsField[] | undefined): PluginSettingsField[] {
  if (!schema) {
    return []
  }
  const out: PluginSettingsField[] = []
  for (const field of schema) {
    out.push(field)
    if (field.type === 'group' && field.children) {
      out.push(...flattenFields(field.children))
    }
  }
  return out
}

/**
 * Return a new schema with `extraChildren` appended to the `children` of the
 * `group` field whose `key` matches `groupKey`, searched at any depth. Lets a
 * composition root inject fields into a specific nesting level (e.g. one
 * shared "Permissions" group) without knowing or rebuilding the rest of the
 * tree. Returns `schema` unchanged (new array, same fields) if no matching
 * group is found.
 */
export function appendGroupChildren(
  schema: PluginSettingsField[],
  groupKey: string,
  extraChildren: PluginSettingsField[]
): PluginSettingsField[] {
  return schema.map((field) => {
    if (field.type !== 'group') {
      return field
    }
    if (field.key === groupKey) {
      return { ...field, children: [...(field.children ?? []), ...extraChildren] }
    }
    if (field.children) {
      return { ...field, children: appendGroupChildren(field.children, groupKey, extraChildren) }
    }
    return field
  })
}

export function defaultPluginSettingsFromSchema(
  schema: PluginSettingsField[] | undefined
): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const field of flattenFields(schema)) {
    out[field.key] = field.default
  }
  return out
}

export function mergePluginSettings(
  schema: PluginSettingsField[] | undefined,
  stored: unknown
): Record<string, unknown> {
  const defaults = defaultPluginSettingsFromSchema(schema)
  const fields = flattenFields(schema)
  if (fields.length === 0) {
    return defaults
  }
  if (!stored || typeof stored !== 'object' || Array.isArray(stored)) {
    return defaults
  }
  const src = stored as Record<string, unknown>
  const out = { ...defaults }
  for (const field of fields) {
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
  const hostFields = flattenFields(manifest?.contributes.hostSettingsSchema)
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
  for (const field of hostFields) {
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
