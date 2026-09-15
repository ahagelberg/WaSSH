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

/** One settings 'group' field: a master toggle plus one 'permission' child per method. */
export function buildApiPermissionGroup(
  pluginId: string,
  groupId: string,
  groupLabel: string,
  methods: PluginApiMethod[],
  opts: {
    groupDefault?: boolean
    description?: string
    extraChildren?: PluginSettingsField[]
    /** Method sets collapsed into one permission field each instead of one per method. */
    bundles?: PermissionBundle[]
  } = {}
): PluginSettingsField {
  const bundles = opts.bundles ?? []
  const bundled = new Set(bundles.flatMap((b) => b.methods.map((m) => m.name)))
  const children = methods
    .filter((m) => !bundled.has(m.name))
    .map((m) => buildPermissionField(pluginId, m))
  for (const bundle of bundles) {
    children.push(
      buildBundledPermissionField(
        pluginId,
        bundle.methods,
        bundle.label,
        bundle.description
      )
    )
  }
  return {
    key: groupKey(pluginId, groupId),
    label: groupLabel,
    type: 'group',
    default: opts.groupDefault ?? false,
    description: opts.description,
    children: [...children, ...(opts.extraChildren ?? [])]
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

/** A set of methods sharing one permission field. */
export interface PermissionBundle {
  methods: PluginApiMethod[]
  label: string
  description: string
}

/**
 * One 'permission' field controlling several methods at once. The key is the
 * first method's, so every bundled method resolves through the same setting.
 */
export function buildBundledPermissionField(
  pluginId: string,
  methods: PluginApiMethod[],
  label: string,
  description: string,
  defaultPermission: PluginPermissionDecision = 'ask'
): PluginSettingsField {
  return {
    key: permissionKey(pluginId, methods[0].name),
    label,
    type: 'permission',
    default: defaultPermission,
    description
  }
}

/** Methods within an enabled group whose resolved permission is not 'deny'. */
export function allowedGroupMethods(
  pluginId: string,
  groupId: string,
  methods: PluginApiMethod[],
  settings: Record<string, unknown>
): PluginApiMethod[] {
  if (!isGroupEnabled(settings, pluginId, groupId)) {
    return []
  }
  return methods.filter((m) => resolvePermission(settings, pluginId, m.name) !== 'deny')
}

/** Map each bundled method name to the permission key its bundle resolves through. */
function bundleKeyMap(pluginId: string, bundles: PermissionBundle[]): Map<string, string> {
  const map = new Map<string, string>()
  for (const bundle of bundles) {
    for (const method of bundle.methods) {
      map.set(method.name, permissionKey(pluginId, bundle.methods[0].name))
    }
  }
  return map
}

/**
 * Like `allowedGroupMethods`, but methods in any of `bundles` resolve through
 * their bundle leader's permission setting, so one bundled field gates them
 * all. Methods outside every bundle keep their own permission.
 */
export function allowedGroupMethodsWithBundles(
  pluginId: string,
  groupId: string,
  methods: PluginApiMethod[],
  bundles: PermissionBundle[],
  settings: Record<string, unknown>
): PluginApiMethod[] {
  if (!isGroupEnabled(settings, pluginId, groupId)) {
    return []
  }
  const keys = bundleKeyMap(pluginId, bundles)
  return methods.filter((m) => {
    const key = keys.get(m.name) ?? permissionKey(pluginId, m.name)
    const value = settings[key]
    const decision: PluginPermissionDecision =
      value === 'allow' || value === 'deny' || value === 'ask' ? value : 'allow'
    return decision !== 'deny'
  })
}

/**
 * Settings key a method's permission is actually stored under. Bundled
 * methods share their bundle leader's key, so "Always allow" on any of them
 * persists to the setting the bundle reads.
 */
export function permissionSettingKeyFor(
  pluginId: string,
  method: string,
  bundles: PermissionBundle[]
): string {
  const keys = bundleKeyMap(pluginId, bundles)
  return keys.get(method) ?? permissionKey(pluginId, method)
}

/** True unless the method's own permission is 'deny'. */
export function isMethodAllowed(
  settings: Record<string, unknown>,
  pluginId: string,
  method: PluginApiMethod
): boolean {
  return resolvePermission(settings, pluginId, method.name) !== 'deny'
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
