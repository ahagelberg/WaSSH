/** Sentinel id for the ungrouped section in drag/drop UI */
export const UNGROUPED_SECTION_ID = '__ungrouped__'

/** 6-digit hex color (#rrggbb), optional leading # */
const HEX_COLOR_RE = /^#?[0-9a-fA-F]{6}$/

/** Normalize a stored group color to lowercase '#rrggbb', or undefined when invalid/absent. */
function normalizeGroupColor(raw: unknown): string | undefined {
  if (typeof raw !== 'string') {
    return undefined
  }
  const m = HEX_COLOR_RE.exec(raw.trim())
  return m ? `#${m[0].toLowerCase().replace('#', '')}` : undefined
}

export interface HostGroup {
  id: string
  name: string
  /** Accent color (hex) for the group header in the host list; empty = no color */
  color?: string
  collapsed: boolean
  hostIds: string[]
}

export interface HostsOrganization {
  groups: HostGroup[]
  ungroupedHostIds: string[]
  ungroupedCollapsed: boolean
}

export const DEFAULT_HOSTS_ORGANIZATION: HostsOrganization = {
  groups: [],
  ungroupedHostIds: [],
  ungroupedCollapsed: false
}

function dedupeIds(ids: string[]): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const id of ids) {
    if (!id || seen.has(id)) {
      continue
    }
    seen.add(id)
    out.push(id)
  }
  return out
}

function normalizeGroup(raw: Partial<HostGroup> & { id: string }): HostGroup {
  return {
    id: raw.id,
    name: typeof raw.name === 'string' && raw.name.trim() ? raw.name.trim() : 'Group',
    color: normalizeGroupColor(raw.color),
    collapsed: raw.collapsed === true,
    hostIds: dedupeIds(Array.isArray(raw.hostIds) ? raw.hostIds.filter((id) => typeof id === 'string') : [])
  }
}

export function normalizeHostsOrganization(raw: unknown): HostsOrganization {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return { ...DEFAULT_HOSTS_ORGANIZATION }
  }
  const obj = raw as Partial<HostsOrganization>
  return {
    groups: Array.isArray(obj.groups)
      ? obj.groups
          .filter((g): g is HostGroup => Boolean(g && typeof g.id === 'string'))
          .map((g) => normalizeGroup(g))
      : [],
    ungroupedHostIds: dedupeIds(
      Array.isArray(obj.ungroupedHostIds)
        ? obj.ungroupedHostIds.filter((id): id is string => typeof id === 'string')
        : []
    ),
    ungroupedCollapsed: obj.ungroupedCollapsed === true
  }
}

/** Drop unknown ids; append hosts missing from every list to ungrouped */
export function reconcileOrganization(
  org: HostsOrganization,
  hostIds: string[]
): HostsOrganization {
  const valid = new Set(hostIds)
  const placed = new Set<string>()

  const groups = org.groups.map((g) => {
    const hostIdsInGroup = g.hostIds.filter((id) => {
      if (!valid.has(id) || placed.has(id)) {
        return false
      }
      placed.add(id)
      return true
    })
    return { ...g, hostIds: hostIdsInGroup }
  })

  const ungroupedHostIds = org.ungroupedHostIds.filter((id) => {
    if (!valid.has(id) || placed.has(id)) {
      return false
    }
    placed.add(id)
    return true
  })

  for (const id of hostIds) {
    if (!placed.has(id)) {
      ungroupedHostIds.push(id)
      placed.add(id)
    }
  }

  return {
    groups,
    ungroupedHostIds,
    ungroupedCollapsed: org.ungroupedCollapsed
  }
}

export function removeHostFromOrganization(
  org: HostsOrganization,
  hostId: string
): HostsOrganization {
  return {
    ...org,
    groups: org.groups.map((g) => ({
      ...g,
      hostIds: g.hostIds.filter((id) => id !== hostId)
    })),
    ungroupedHostIds: org.ungroupedHostIds.filter((id) => id !== hostId)
  }
}

export function appendHostToUngrouped(
  org: HostsOrganization,
  hostId: string
): HostsOrganization {
  const without = removeHostFromOrganization(org, hostId)
  return {
    ...without,
    ungroupedHostIds: [...without.ungroupedHostIds, hostId]
  }
}

function hostIdsForSection(org: HostsOrganization, sectionId: string): string[] {
  if (sectionId === UNGROUPED_SECTION_ID) {
    return org.ungroupedHostIds
  }
  const group = org.groups.find((g) => g.id === sectionId)
  return group?.hostIds ?? []
}

function setHostIdsForSection(
  org: HostsOrganization,
  sectionId: string,
  hostIds: string[]
): HostsOrganization {
  if (sectionId === UNGROUPED_SECTION_ID) {
    return { ...org, ungroupedHostIds: hostIds }
  }
  return {
    ...org,
    groups: org.groups.map((g) => (g.id === sectionId ? { ...g, hostIds } : g))
  }
}

/** Move a host to a section at insertIndex (after removal from all sections) */
export function moveHostInOrganization(
  org: HostsOrganization,
  hostId: string,
  toSectionId: string,
  insertIndex: number
): HostsOrganization {
  const without = removeHostFromOrganization(org, hostId)
  const list = hostIdsForSection(without, toSectionId).slice()
  const idx = Math.max(0, Math.min(insertIndex, list.length))
  list.splice(idx, 0, hostId)
  return setHostIdsForSection(without, toSectionId, list)
}

/** Reorder named groups; ungrouped is not part of this list */
export function reorderNamedGroups(
  org: HostsOrganization,
  fromIndex: number,
  toIndex: number
): HostsOrganization {
  if (fromIndex === toIndex || fromIndex < 0 || toIndex < 0) {
    return org
  }
  const groups = org.groups.slice()
  if (fromIndex >= groups.length || toIndex >= groups.length) {
    return org
  }
  const [item] = groups.splice(fromIndex, 1)
  groups.splice(toIndex, 0, item)
  return { ...org, groups }
}

/** Delete a named group; its hosts move to the end of ungrouped */
export function deleteNamedGroup(org: HostsOrganization, groupId: string): HostsOrganization {
  const group = org.groups.find((g) => g.id === groupId)
  if (!group) {
    return org
  }
  const ungroupedSet = new Set(org.ungroupedHostIds)
  const ungroupedHostIds = org.ungroupedHostIds.slice()
  for (const id of group.hostIds) {
    if (!ungroupedSet.has(id)) {
      ungroupedHostIds.push(id)
      ungroupedSet.add(id)
    }
  }
  return {
    ...org,
    groups: org.groups.filter((g) => g.id !== groupId),
    ungroupedHostIds
  }
}

export function uniqueGroupName(groups: HostGroup[], base = 'New group'): string {
  const names = new Set(groups.map((g) => g.name.toLowerCase()))
  if (!names.has(base.toLowerCase())) {
    return base
  }
  let n = 2
  while (names.has(`${base} ${n}`.toLowerCase())) {
    n += 1
  }
  return `${base} ${n}`
}
