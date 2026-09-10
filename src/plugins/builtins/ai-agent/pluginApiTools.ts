import type {
  PluginApiListing,
  PluginApiMethod,
  PluginPermissionDecision,
  PluginSettingsField
} from '@plugin-api/shared'
import { PLUGIN_ID_AI_AGENT } from './id'

/** Wire tool name prefix/separator used to route calls for *other* plugins' API methods. */
const TOOL_NAME_PREFIX = 'plugin_api'
const TOOL_NAME_SEPARATOR = '__'

export interface PluginApiToolTarget {
  pluginId: string
  method: string
}

/** Wire tool name for an *other* plugin's declared method (routed via `ctx.callPluginApi`). */
export function pluginApiToolName(pluginId: string, method: string): string {
  return [TOOL_NAME_PREFIX, pluginId, method].join(TOOL_NAME_SEPARATOR)
}

function parsePluginApiToolName(name: string): PluginApiToolTarget | null {
  const parts = name.split(TOOL_NAME_SEPARATOR)
  if (parts.length !== 3 || parts[0] !== TOOL_NAME_PREFIX) {
    return null
  }
  return { pluginId: parts[1], method: parts[2] }
}

/**
 * Resolve which (pluginId, method) a wire tool name maps to. AI Agent's own
 * built-in tools keep their original unprefixed names (`run_command`, ...);
 * other plugins' methods are prefixed by `pluginApiToolName`.
 */
export function resolveToolTarget(toolName: string): PluginApiToolTarget {
  return parsePluginApiToolName(toolName) ?? { pluginId: PLUGIN_ID_AI_AGENT, method: toolName }
}

function permissionKey(pluginId: string, method: string): string {
  return `apiMethod:${pluginId}:${method}`
}

/** Public settings-key builder, e.g. for `ctx.setSettingValue` on "Approve always". */
export const permissionSettingKey = permissionKey

/** Settings key for one plugin's named `group` field (any nesting level). */
export function groupKey(pluginId: string, groupId: string): string {
  return `apiGroup:${pluginId}:${groupId}`
}

/**
 * Root group id every AI Agent tool permission (its own categories, and every
 * other plugin's declared API) is nested under, so one master toggle gates
 * everything beneath it regardless of nesting depth.
 */
export const AI_AGENT_GROUP_PERMISSIONS_ROOT = 'permissions'
export const AI_AGENT_PERMISSIONS_ROOT_KEY = groupKey(PLUGIN_ID_AI_AGENT, AI_AGENT_GROUP_PERMISSIONS_ROOT)

/** Resolved trinary permission for one (pluginId, method), defaulting to 'allow'. */
export function resolvePermission(
  settings: Record<string, unknown>,
  pluginId: string,
  method: string
): PluginPermissionDecision {
  const value = settings[permissionKey(pluginId, method)]
  return value === 'allow' || value === 'deny' || value === 'ask' ? value : 'allow'
}

function isGroupEnabled(settings: Record<string, unknown>, pluginId: string, groupId: string): boolean {
  return Boolean(settings[groupKey(pluginId, groupId)])
}

/** Whether the top-level "Permissions" master toggle is on (gates every nested group/method). */
function isPermissionsRootEnabled(settings: Record<string, unknown>): boolean {
  return Boolean(settings[AI_AGENT_PERMISSIONS_ROOT_KEY])
}

/** One settings 'group' field: a master toggle plus one 'permission' child per method. */
export function buildApiPermissionGroup(
  pluginId: string,
  groupId: string,
  groupLabel: string,
  methods: PluginApiMethod[],
  opts: { groupDefault?: boolean; description?: string; extraChildren?: PluginSettingsField[] } = {}
): PluginSettingsField {
  return {
    key: groupKey(pluginId, groupId),
    label: groupLabel,
    type: 'group',
    default: opts.groupDefault ?? false,
    description: opts.description,
    children: [
      ...methods.map((m) => buildPermissionField(pluginId, m)),
      ...(opts.extraChildren ?? [])
    ]
  }
}

/**
 * A group field whose children are arbitrary already-built fields (including
 * other `group` fields), so a settings tree can nest as many levels as
 * needed - e.g. one root "Permissions" group containing several category
 * subgroups, each containing their own `permission` leaves.
 */
export function buildContainerGroup(
  pluginId: string,
  groupId: string,
  groupLabel: string,
  children: PluginSettingsField[],
  opts: { groupDefault?: boolean; description?: string } = {}
): PluginSettingsField {
  return {
    key: groupKey(pluginId, groupId),
    label: groupLabel,
    type: 'group',
    default: opts.groupDefault ?? true,
    description: opts.description,
    children
  }
}

/** A single ungrouped 'permission' settings field for one method. */
export function buildPermissionField(pluginId: string, method: PluginApiMethod): PluginSettingsField {
  return {
    key: permissionKey(pluginId, method.name),
    label: method.label ?? method.name,
    type: 'permission',
    default: method.defaultPermission ?? 'allow',
    description: method.description
  }
}

/** Methods within an enabled group whose resolved permission is not 'deny'. */
export function allowedGroupMethods(
  pluginId: string,
  groupId: string,
  methods: PluginApiMethod[],
  settings: Record<string, unknown>
): PluginApiMethod[] {
  if (!isPermissionsRootEnabled(settings) || !isGroupEnabled(settings, pluginId, groupId)) {
    return []
  }
  return methods.filter((m) => resolvePermission(settings, pluginId, m.name) !== 'deny')
}

/** True unless the root "Permissions" toggle is off, or the method's own permission is 'deny'. */
export function isMethodAllowed(
  settings: Record<string, unknown>,
  pluginId: string,
  method: PluginApiMethod
): boolean {
  return isPermissionsRootEnabled(settings) && resolvePermission(settings, pluginId, method.name) !== 'deny'
}

/** Fixed group id: other plugins contribute exactly one API group each. */
const EXTERNAL_API_GROUP_ID = 'api'

/** Settings field for one *other* plugin's whole declared API (one group per plugin). */
export function buildExternalApiPermissionField(
  pluginId: string,
  pluginName: string,
  methods: PluginApiMethod[]
): PluginSettingsField {
  return buildApiPermissionGroup(pluginId, EXTERNAL_API_GROUP_ID, `${pluginName} API`, methods, {
    description: `Let the AI agent call "${pluginName}"'s API when its panel is open.`
  })
}

/** Wire-ready tools for one other plugin's currently-allowed methods. */
export function externalApiTools(
  listing: PluginApiListing,
  settings: Record<string, unknown>
): PluginApiMethod[] {
  const allowed = allowedGroupMethods(listing.pluginId, EXTERNAL_API_GROUP_ID, listing.methods, settings)
  return allowed.map((m) => ({
    ...m,
    name: pluginApiToolName(listing.pluginId, m.name),
    description: `${m.description} (Requires the "${listing.pluginName}" plugin panel to be open in this tab.)`
  }))
}

/** All other-plugin API tools currently offered, across every active plugin on this tab. */
export function allExternalApiTools(
  ctx: { listPluginApis: () => PluginApiListing[] },
  settings: Record<string, unknown>
): PluginApiMethod[] {
  return ctx.listPluginApis().flatMap((listing) => externalApiTools(listing, settings))
}
