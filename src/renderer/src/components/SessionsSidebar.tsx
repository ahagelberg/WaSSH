import { useEffect, useRef, useState, type ReactElement } from 'react'
import type { HostProfile, HostsOrganization } from '@shared/types'
import { hostDisplayName, hostSubtitle, resolveSessionStyle, type SessionStyle } from '@shared/connection'
import { sessionAccentStyle } from '../sessionStyleCss'
import ColorHexInput from './ColorHexInput'
import {
  UNGROUPED_SECTION_ID,
  deleteNamedGroup,
  moveHostInOrganization,
  reorderNamedGroups,
  uniqueGroupName,
  type HostGroup
} from '@shared/hostOrganization'

interface Props {
  hosts: HostProfile[]
  organization: HostsOrganization
  styleDefaults: SessionStyle
  collapsed: boolean
  onToggleCollapse: () => void
  onConnect: (host: HostProfile) => void
  onEdit: (host: HostProfile) => void
  onDuplicate: (host: HostProfile) => void
  onDelete: (host: HostProfile) => void
  onNewHost: () => void
  onAddHostToGroup: (groupId: string) => void
  onSaveOrganization: (org: HostsOrganization) => void
}

/** Gap between anchor and menu */
const HOST_MENU_GAP_PX = 4

/** Approximate menu height (4 items) for flip-above calculation */
const HOST_MENU_ESTIMATED_HEIGHT_PX = 160

/** Minimum menu width; keep in sync with --host-menu-min-width */
const HOST_MENU_MIN_WIDTH_PX = 120

/** Pointer movement before drag starts; keep in sync with --host-drag-threshold */
const HOST_DRAG_THRESHOLD_PX = 4

/** Drag ghost offset from pointer (px) */
const HOST_DRAG_GHOST_OFFSET_X_PX = 12
const HOST_DRAG_GHOST_OFFSET_Y_PX = 10

/** Drop before host when pointer is above this fraction of row height */
const HOST_DROP_BEFORE_RATIO = 0.5

/** Preset accent colors offered in the group color picker */
const GROUP_COLOR_PRESETS = [
  '#e35d6a',
  '#f0b429',
  '#3dd68c',
  '#14b8a6',
  '#3d8bfd',
  '#5a9dff',
  '#8b5cf6',
  '#d946ef',
  '#ec4899',
  '#f97316',
  '#84cc16',
  '#94a3b8'
]

/** Shown in the native swatch while a group has no color yet */
const GROUP_COLOR_FALLBACK = '#3d8bfd'

/** Group color popover sizing (keep in sync with .host-group-color-pop CSS) */
const GROUP_COLOR_POP_WIDTH_PX = 224
const GROUP_COLOR_POP_ESTIMATED_HEIGHT_PX = 176

interface HostMenuState {
  hostId: string
  left: number
  top: number
}

interface GroupMenuState {
  groupId: string
  left: number
  top: number
}

interface HostDragGhost {
  hostId: string
  x: number
  y: number
}

interface HostDragState {
  hostId: string
  startX: number
  startY: number
  lastY: number
  active: boolean
}

interface GroupDragState {
  groupId: string
  startX: number
  startY: number
  active: boolean
  dropGap: number | null
}

type HostDropHint =
  | { kind: 'before'; sectionId: string; hostId: string }
  | { kind: 'after'; sectionId: string; hostId: string }
  | { kind: 'append'; sectionId: string }

function menuPositionAtPoint(x: number, y: number): { left: number; top: number } {
  let left = x
  if (left + HOST_MENU_MIN_WIDTH_PX > window.innerWidth) {
    left = Math.max(0, window.innerWidth - HOST_MENU_MIN_WIDTH_PX)
  }

  const openBelow = y + HOST_MENU_GAP_PX
  const openAbove = y - HOST_MENU_GAP_PX - HOST_MENU_ESTIMATED_HEIGHT_PX
  const fitsBelow = openBelow + HOST_MENU_ESTIMATED_HEIGHT_PX <= window.innerHeight
  const top = fitsBelow ? openBelow : Math.max(0, openAbove)

  return { left, top }
}

function hostIdsForSection(org: HostsOrganization, sectionId: string): string[] {
  if (sectionId === UNGROUPED_SECTION_ID) {
    return org.ungroupedHostIds
  }
  return org.groups.find((g) => g.id === sectionId)?.hostIds ?? []
}

function targetInsertIndex(
  list: string[],
  draggedId: string,
  hint: HostDropHint
): number {
  const without = list.filter((id) => id !== draggedId)
  if (hint.kind === 'append') {
    return without.length
  }
  const refIdx = without.indexOf(hint.hostId)
  if (refIdx < 0) {
    return without.length
  }
  return hint.kind === 'before' ? refIdx : refIdx + 1
}

function findHostDropHint(listEl: HTMLElement, clientY: number): HostDropHint | null {
  const items = listEl.querySelectorAll<HTMLElement>('.host-item[data-host-id]')
  for (const item of items) {
    const rect = item.getBoundingClientRect()
    const sectionEl = item.closest<HTMLElement>('[data-section-id]')
    const sectionId = sectionEl?.dataset.sectionId
    const hostId = item.dataset.hostId
    if (!sectionId || !hostId) {
      continue
    }
    if (clientY >= rect.top && clientY <= rect.bottom) {
      const mid = rect.top + rect.height * HOST_DROP_BEFORE_RATIO
      return clientY < mid
        ? { kind: 'before', sectionId, hostId }
        : { kind: 'after', sectionId, hostId }
    }
  }

  const groups = listEl.querySelectorAll<HTMLElement>('.host-group[data-section-id]')
  for (const group of groups) {
    const sectionId = group.dataset.sectionId
    if (!sectionId) {
      continue
    }
    const rect = group.getBoundingClientRect()
    if (clientY >= rect.top && clientY <= rect.bottom) {
      return { kind: 'append', sectionId }
    }
  }

  return null
}

function gapIndexAtY(listEl: HTMLElement, clientY: number, groupCount: number): number {
  const headers = listEl.querySelectorAll<HTMLElement>('.host-group-header[data-group-id]')
  for (let i = 0; i < headers.length; i++) {
    const section = headers[i].closest<HTMLElement>('.host-group')
    const rect = (section ?? headers[i]).getBoundingClientRect()
    const mid = rect.top + rect.height * HOST_DROP_BEFORE_RATIO
    if (clientY < mid) {
      return i
    }
  }
  return groupCount
}

function groupInsertIndexFromGap(from: number, gap: number): number | null {
  if (gap === from || gap === from + 1) {
    return null
  }
  return gap > from ? gap - 1 : gap
}

function hostDropClass(hint: HostDropHint | null, sectionId: string, hostId: string): string {
  if (!hint) {
    return ''
  }
  if (hint.kind === 'before' && hint.sectionId === sectionId && hint.hostId === hostId) {
    return ' drop-before'
  }
  if (hint.kind === 'after' && hint.sectionId === sectionId && hint.hostId === hostId) {
    return ' drop-after'
  }
  return ''
}

export default function SessionsSidebar({
  hosts,
  organization,
  styleDefaults,
  collapsed,
  onToggleCollapse,
  onConnect,
  onEdit,
  onDuplicate,
  onDelete,
  onNewHost,
  onAddHostToGroup,
  onSaveOrganization
}: Props): ReactElement {
  const [hostMenu, setHostMenu] = useState<HostMenuState | null>(null)
  const [groupMenu, setGroupMenu] = useState<GroupMenuState | null>(null)
  const [renamingGroupId, setRenamingGroupId] = useState<string | null>(null)
  const [renameDraft, setRenameDraft] = useState('')
  const [draggingHostId, setDraggingHostId] = useState<string | null>(null)
  const [hostDragGhost, setHostDragGhost] = useState<HostDragGhost | null>(null)
  const [draggingGroupId, setDraggingGroupId] = useState<string | null>(null)
  const [hostDropHint, setHostDropHint] = useState<HostDropHint | null>(null)
  const [groupDropGap, setGroupDropGap] = useState<number | null>(null)
  const [appendTargetSectionId, setAppendTargetSectionId] = useState<string | null>(null)
  const [groupColorMenu, setGroupColorMenu] = useState<GroupMenuState | null>(null)
  const [groupDragGhost, setGroupDragGhost] = useState<{
    groupId: string
    x: number
    y: number
  } | null>(null)

  const listRef = useRef<HTMLDivElement>(null)
  const hostDragRef = useRef<HostDragState | null>(null)
  const groupDragRef = useRef<GroupDragState | null>(null)
  const suppressHostClickRef = useRef(false)
  const suppressGroupClickRef = useRef(false)
  const hostDropHintRef = useRef<HostDropHint | null>(null)
  const orgRef = useRef(organization)
  const onSaveOrgRef = useRef(onSaveOrganization)

  orgRef.current = organization
  onSaveOrgRef.current = onSaveOrganization
  hostDropHintRef.current = hostDropHint

  const hostById = new Map(hosts.map((h) => [h.id, h]))

  useEffect(() => {
    if (!hostMenu && !groupMenu && !groupColorMenu) {
      return
    }
    const close = (ev: MouseEvent): void => {
      const target = ev.target
      if (!(target instanceof Element)) {
        return
      }
      if (
        target.closest('.host-context-menu') ||
        target.closest('.host-group-context-menu') ||
        target.closest('.host-group-color-pop')
      ) {
        return
      }
      setHostMenu(null)
      setGroupMenu(null)
      setGroupColorMenu(null)
    }
    const closeOnScroll = (): void => {
      setHostMenu(null)
      setGroupMenu(null)
      setGroupColorMenu(null)
    }
    const closeOnEscape = (ev: KeyboardEvent): void => {
      if (ev.key === 'Escape') {
        setGroupColorMenu(null)
      }
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('scroll', closeOnScroll, true)
    document.addEventListener('keydown', closeOnEscape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('scroll', closeOnScroll, true)
      document.removeEventListener('keydown', closeOnEscape)
    }
  }, [hostMenu, groupMenu, groupColorMenu])

  useEffect(() => {
    const onMove = (e: PointerEvent): void => {
      const hostDrag = hostDragRef.current
      if (hostDrag) {
        if (!hostDrag.active) {
          const dx = e.clientX - hostDrag.startX
          const dy = e.clientY - hostDrag.startY
          if (dx * dx + dy * dy < HOST_DRAG_THRESHOLD_PX * HOST_DRAG_THRESHOLD_PX) {
            return
          }
          hostDrag.active = true
          setHostMenu(null)
          setGroupMenu(null)
          setGroupColorMenu(null)
          setDraggingHostId(hostDrag.hostId)
          setHostDragGhost({ hostId: hostDrag.hostId, x: e.clientX, y: e.clientY })
        }
        hostDrag.lastY = e.clientY
        if (hostDrag.active) {
          setHostDragGhost({ hostId: hostDrag.hostId, x: e.clientX, y: e.clientY })
        }
        const list = listRef.current
        if (!list) {
          return
        }
        const hint = findHostDropHint(list, e.clientY)
        hostDropHintRef.current = hint
        setHostDropHint(hint)
        setAppendTargetSectionId(hint?.kind === 'append' ? hint.sectionId : null)
        return
      }

      const groupDrag = groupDragRef.current
      if (!groupDrag) {
        return
      }
      if (!groupDrag.active) {
        const dx = e.clientX - groupDrag.startX
        const dy = e.clientY - groupDrag.startY
        if (dx * dx + dy * dy < HOST_DRAG_THRESHOLD_PX * HOST_DRAG_THRESHOLD_PX) {
          return
        }
        groupDrag.active = true
        setHostMenu(null)
        setGroupMenu(null)
        setGroupColorMenu(null)
        setDraggingGroupId(groupDrag.groupId)
      }
      setGroupDragGhost({ groupId: groupDrag.groupId, x: e.clientX, y: e.clientY })
      const list = listRef.current
      if (!list) {
        return
      }
      const gap = gapIndexAtY(list, e.clientY, orgRef.current.groups.length)
      groupDrag.dropGap = gap
      setGroupDropGap(gap)
    }

    const onUp = (): void => {
      const hostDrag = hostDragRef.current
      if (hostDrag) {
        if (hostDrag.active) {
          suppressHostClickRef.current = true
          const list = listRef.current
          const activeHint =
            hostDropHintRef.current ??
            (list ? findHostDropHint(list, hostDrag.lastY) : null)
          if (activeHint) {
            const org = orgRef.current
            const sectionList = hostIdsForSection(org, activeHint.sectionId)
            const insertIndex = targetInsertIndex(sectionList, hostDrag.hostId, activeHint)
            const next = moveHostInOrganization(
              org,
              hostDrag.hostId,
              activeHint.sectionId,
              insertIndex
            )
            onSaveOrgRef.current(next)
          }
        }
        hostDragRef.current = null
        setDraggingHostId(null)
        setHostDragGhost(null)
        setHostDropHint(null)
        setAppendTargetSectionId(null)
        window.setTimeout(() => {
          suppressHostClickRef.current = false
        }, 0)
      }

      const groupDrag = groupDragRef.current
      if (groupDrag) {
        if (groupDrag.active) {
          suppressGroupClickRef.current = true
          window.setTimeout(() => {
            suppressGroupClickRef.current = false
          }, 0)
          if (groupDrag.dropGap !== null) {
            const from = orgRef.current.groups.findIndex((g) => g.id === groupDrag.groupId)
            const insert = groupInsertIndexFromGap(from, groupDrag.dropGap)
            if (from >= 0 && insert !== null) {
              onSaveOrgRef.current(reorderNamedGroups(orgRef.current, from, insert))
            }
          }
        }
        groupDragRef.current = null
        setDraggingGroupId(null)
        setGroupDropGap(null)
        setGroupDragGhost(null)
      }
    }

    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', onUp)
    return () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      window.removeEventListener('pointercancel', onUp)
    }
  }, [])

  const closeHostMenu = (): void => setHostMenu(null)
  const closeGroupMenu = (): void => setGroupMenu(null)

  const openHostMenuAt = (hostId: string, left: number, top: number): void => {
    setGroupMenu(null)
    setHostMenu({ hostId, ...menuPositionAtPoint(left, top) })
  }

  const toggleGroupCollapse = (sectionId: string): void => {
    if (sectionId === UNGROUPED_SECTION_ID) {
      onSaveOrganization({
        ...organization,
        ungroupedCollapsed: !organization.ungroupedCollapsed
      })
      return
    }
    onSaveOrganization({
      ...organization,
      groups: organization.groups.map((g) =>
        g.id === sectionId ? { ...g, collapsed: !g.collapsed } : g
      )
    })
  }

  const startRenameGroup = (group: HostGroup): void => {
    setRenamingGroupId(group.id)
    setRenameDraft(group.name)
    closeGroupMenu()
  }

  const commitRenameGroup = (): void => {
    if (!renamingGroupId) {
      return
    }
    const name = renameDraft.trim()
    if (name) {
      onSaveOrganization({
        ...organization,
        groups: organization.groups.map((g) =>
          g.id === renamingGroupId ? { ...g, name } : g
        )
      })
    }
    setRenamingGroupId(null)
    setRenameDraft('')
  }

  const cancelRenameGroup = (): void => {
    setRenamingGroupId(null)
    setRenameDraft('')
  }

  const handleNewGroup = (): void => {
    const id = crypto.randomUUID()
    const name = uniqueGroupName(organization.groups)
    onSaveOrganization({
      ...organization,
      groups: [...organization.groups, { id, name, collapsed: false, hostIds: [] }]
    })
    setRenamingGroupId(id)
    setRenameDraft(name)
  }

  const handleDeleteGroup = (groupId: string): void => {
    closeGroupMenu()
    onSaveOrganization(deleteNamedGroup(organization, groupId))
  }

  const setGroupColor = (groupId: string, color: string | undefined): void => {
    onSaveOrganization({
      ...organization,
      groups: organization.groups.map((g) =>
        g.id === groupId ? { ...g, color: color ?? undefined } : g
      )
    })
  }

  const menuHost = hostMenu ? hostById.get(hostMenu.hostId) : undefined
  const menuGroup = groupMenu
    ? organization.groups.find((g) => g.id === groupMenu.groupId)
    : undefined
  const menuColorGroup = groupColorMenu
    ? organization.groups.find((g) => g.id === groupColorMenu.groupId)
    : undefined
  const ghostHost = hostDragGhost ? hostById.get(hostDragGhost.hostId) : undefined
  const ghostGroup = groupDragGhost
    ? organization.groups.find((g) => g.id === groupDragGhost.groupId)
    : undefined

  const renderHostRow = (host: HostProfile, sectionId: string): ReactElement => {
    const proxy = host.proxyHostId ? hostById.get(host.proxyHostId) : null
    const dropClass = hostDropClass(hostDropHint, sectionId, host.id)
    const tabColor = resolveSessionStyle(host, styleDefaults).tabColor
    return (
      <div
        key={host.id}
        className={`host-item${draggingHostId === host.id ? ' dragging' : ''}${dropClass}`}
        data-host-id={host.id}
        data-tab-color={tabColor || undefined}
        style={sessionAccentStyle(tabColor)}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
          openHostMenuAt(host.id, e.clientX, e.clientY)
        }}
      >
        <div
          className="host-item-main"
          onDoubleClick={() => {
            if (suppressHostClickRef.current) {
              return
            }
            onConnect(host)
          }}
          onPointerDown={(e) => {
            if (e.button !== 0) {
              return
            }
            hostDragRef.current = {
              hostId: host.id,
              startX: e.clientX,
              startY: e.clientY,
              lastY: e.clientY,
              active: false
            }
            e.currentTarget.setPointerCapture(e.pointerId)
          }}
          onPointerUp={(e) => {
            if (e.currentTarget.hasPointerCapture(e.pointerId)) {
              e.currentTarget.releasePointerCapture(e.pointerId)
            }
          }}
          onPointerCancel={(e) => {
            if (e.currentTarget.hasPointerCapture(e.pointerId)) {
              e.currentTarget.releasePointerCapture(e.pointerId)
            }
          }}
          title="Double-click to connect"
        >
          <div className="host-item-name">{hostDisplayName(host)}</div>
          <div className="host-item-meta">
            {hostSubtitle(host)}
            {proxy ? ` · via ${hostDisplayName(proxy)}` : ''}
          </div>
          {host.tags && host.tags.length > 0 ? (
            <div className="host-item-tags">
              {host.tags.map((tag) => (
                <span key={tag} className="host-tag">
                  {tag}
                </span>
              ))}
            </div>
          ) : null}
        </div>
      </div>
    )
  }

  const renderSection = (
    sectionId: string,
    title: string,
    hostIds: string[],
    sectionCollapsed: boolean,
    options: { named: boolean; group?: HostGroup }
  ): ReactElement => {
    const isNamed = options.named
    const group = options.group
    const isDraggingGroup = isNamed && group && draggingGroupId === group.id
    const isAppendTarget = appendTargetSectionId === sectionId
    const groupColor = isNamed && group?.color ? group.color : undefined
    const groupIndex = isNamed && group ? organization.groups.indexOf(group) : -1
    // Horizontal insertion line above this card; hidden for gaps next to the
    // dragged group itself (those are no-ops). Ungrouped anchors the end gap.
    const showDropLineAbove =
      groupDropGap !== null &&
      groupDragFromIndex >= 0 &&
      groupDropGap !== groupDragFromIndex &&
      groupDropGap !== groupDragFromIndex + 1 &&
      (isNamed ? groupDropGap === groupIndex : groupDropGap === organization.groups.length)

    return (
      <section
        key={sectionId}
        className={`host-group${sectionCollapsed ? ' collapsed' : ''}${isAppendTarget ? ' is-drop-target' : ''}${showDropLineAbove ? ' group-drop-before' : ''}`}
        data-section-id={sectionId}
        data-tab-color={groupColor}
        style={sessionAccentStyle(groupColor)}
      >
        <div
          className={`host-group-header${isDraggingGroup ? ' is-dragging' : ''}`}
          data-group-id={isNamed ? sectionId : undefined}
          onClick={(e) => {
            if (suppressGroupClickRef.current) {
              return
            }
            if (e.target instanceof Element) {
              if (
                e.target.closest('.host-group-title-input') ||
                e.target.closest('.host-group-collapse') ||
                e.target.closest('.host-group-color-dot')
              ) {
                return
              }
            }
            if (isNamed && renamingGroupId === sectionId) {
              return
            }
            toggleGroupCollapse(sectionId)
          }}
          onDoubleClick={(e) => {
            if (!isNamed || !group) {
              return
            }
            e.stopPropagation()
            startRenameGroup(group)
          }}
          onContextMenu={(e) => {
            if (!isNamed || !group) {
              return
            }
            e.preventDefault()
            e.stopPropagation()
            setHostMenu(null)
            setGroupColorMenu(null)
            setGroupMenu({ groupId: group.id, ...menuPositionAtPoint(e.clientX, e.clientY) })
          }}
          onPointerDown={(e) => {
            if (!isNamed || !group) {
              return
            }
            if (e.button !== 0) {
              return
            }
            if (renamingGroupId === sectionId) {
              return
            }
            if (e.target instanceof Element && e.target.closest('.host-group-collapse')) {
              return
            }
            groupDragRef.current = {
              groupId: group.id,
              startX: e.clientX,
              startY: e.clientY,
              active: false,
              dropGap: null
            }
            e.currentTarget.setPointerCapture(e.pointerId)
          }}
          onPointerUp={(e) => {
            if (e.currentTarget.hasPointerCapture(e.pointerId)) {
              e.currentTarget.releasePointerCapture(e.pointerId)
            }
          }}
          onPointerCancel={(e) => {
            if (e.currentTarget.hasPointerCapture(e.pointerId)) {
              e.currentTarget.releasePointerCapture(e.pointerId)
            }
          }}
        >
          <button
            type="button"
            className="host-group-collapse"
            aria-label={sectionCollapsed ? 'Expand group' : 'Collapse group'}
            onClick={(e) => {
              e.stopPropagation()
              toggleGroupCollapse(sectionId)
            }}
          >
            {sectionCollapsed ? '▸' : '▾'}
          </button>
          {groupColor ? (
            <span className="host-group-color-dot" aria-hidden="true" title="Colored group" />
          ) : null}
          {isNamed && renamingGroupId === sectionId ? (
            <input
              className="host-group-title-input"
              value={renameDraft}
              autoFocus
              onChange={(e) => setRenameDraft(e.target.value)}
              onBlur={commitRenameGroup}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  commitRenameGroup()
                } else if (e.key === 'Escape') {
                  e.preventDefault()
                  cancelRenameGroup()
                }
              }}
              onClick={(e) => e.stopPropagation()}
            />
          ) : (
            <span className="host-group-title">{title}</span>
          )}
          <span className="host-group-count">{hostIds.length}</span>
        </div>
        <div className="host-group-hosts">
          {hostIds.length === 0 ? (
            <div className="host-list-empty">No hosts</div>
          ) : (
            hostIds.map((id) => {
              const host = hostById.get(id)
              return host ? renderHostRow(host, sectionId) : null
            })
          )}
        </div>
      </section>
    )
  }

  const groupDragFromIndex =
    draggingGroupId !== null
      ? organization.groups.findIndex((g) => g.id === draggingGroupId)
      : -1

  const listDraggingClass =
    draggingHostId !== null
      ? ' is-dragging-host'
      : draggingGroupId !== null
        ? ' is-dragging-group'
        : ''

  return (
    <aside className={`sidebar${collapsed ? ' collapsed' : ''}`}>
      <div className="sidebar-header">
        {!collapsed ? <div className="sidebar-title">Hosts</div> : null}
        <button
          type="button"
          className="icon-btn"
          onClick={onToggleCollapse}
          title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
        >
          {collapsed ? '»' : '«'}
        </button>
      </div>
      {!collapsed ? (
        <>
          <div className={`host-list${listDraggingClass}`} ref={listRef}>
            {hosts.length === 0 && organization.groups.length === 0 ? (
              <div className="host-item-meta">No saved hosts yet.</div>
            ) : (
              <>
                {organization.groups.map((group) =>
                  renderSection(group.id, group.name, group.hostIds, group.collapsed, {
                    named: true,
                    group
                  })
                )}
                {renderSection(
                  UNGROUPED_SECTION_ID,
                  'Ungrouped',
                  organization.ungroupedHostIds,
                  organization.ungroupedCollapsed,
                  { named: false }
                )}
              </>
            )}
          </div>
          <div className="sidebar-footer">
            <button type="button" className="primary" onClick={onNewHost}>
              New host…
            </button>
            <button type="button" onClick={handleNewGroup}>
              New group
            </button>
          </div>
          {hostMenu && menuHost ? (
            <div
              className="host-context-menu"
              role="menu"
              style={{ left: hostMenu.left, top: hostMenu.top }}
            >
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  closeHostMenu()
                  onConnect(menuHost)
                }}
              >
                Open
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  closeHostMenu()
                  onEdit(menuHost)
                }}
              >
                Edit
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  closeHostMenu()
                  onDuplicate(menuHost)
                }}
              >
                Duplicate
              </button>
              <button
                type="button"
                role="menuitem"
                className="danger"
                onClick={() => {
                  closeHostMenu()
                  onDelete(menuHost)
                }}
              >
                Delete
              </button>
            </div>
          ) : null}
          {groupMenu && menuGroup ? (
            <div
              className="host-context-menu host-group-context-menu"
              role="menu"
              style={{ left: groupMenu.left, top: groupMenu.top }}
            >
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  closeGroupMenu()
                  onAddHostToGroup(menuGroup.id)
                }}
              >
                Add host
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  closeGroupMenu()
                  setGroupColorMenu({
                    groupId: menuGroup.id,
                    left: groupMenu.left,
                    top: groupMenu.top
                  })
                }}
              >
                Color…
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => startRenameGroup(menuGroup)}
              >
                Rename
              </button>
              <button
                type="button"
                role="menuitem"
                className="danger"
                onClick={() => handleDeleteGroup(menuGroup.id)}
              >
                Delete
              </button>
            </div>
          ) : null}
          {groupColorMenu && menuColorGroup ? (
            <div
              className="host-group-color-pop"
              role="dialog"
              aria-label={`Color for ${menuColorGroup.name}`}
              style={{
                left: Math.min(
                  groupColorMenu.left,
                  Math.max(0, window.innerWidth - GROUP_COLOR_POP_WIDTH_PX - 8)
                ),
                top: Math.min(
                  groupColorMenu.top,
                  Math.max(0, window.innerHeight - GROUP_COLOR_POP_ESTIMATED_HEIGHT_PX)
                )
              }}
            >
              <div className="host-group-color-title">Group color</div>
              <div className="host-group-color-swatches">
                {GROUP_COLOR_PRESETS.map((c) => (
                  <button
                    key={c}
                    type="button"
                    className={`host-group-color-swatch${menuColorGroup.color === c ? ' selected' : ''}`}
                    style={{ background: c }}
                    title={c}
                    aria-label={c}
                    onClick={() => setGroupColor(menuColorGroup.id, c)}
                  />
                ))}
              </div>
              <div className="host-group-color-custom">
                <input
                  type="color"
                  value={menuColorGroup.color ?? GROUP_COLOR_FALLBACK}
                  title="Custom color"
                  onChange={(e) => setGroupColor(menuColorGroup.id, e.target.value)}
                />
                <ColorHexInput
                  value={menuColorGroup.color ?? ''}
                  onChange={(hex) => setGroupColor(menuColorGroup.id, hex)}
                />
              </div>
              {menuColorGroup.color ? (
                <button
                  type="button"
                  className="host-group-color-clear"
                  onClick={() => setGroupColor(menuColorGroup.id, undefined)}
                >
                  Remove color
                </button>
              ) : null}
            </div>
          ) : null}
          {hostDragGhost && ghostHost ? (
            <div
              className="host-drag-ghost"
              aria-hidden="true"
              data-tab-color={
                resolveSessionStyle(ghostHost, styleDefaults).tabColor || undefined
              }
              style={{
                ...sessionAccentStyle(resolveSessionStyle(ghostHost, styleDefaults).tabColor),
                left: hostDragGhost.x + HOST_DRAG_GHOST_OFFSET_X_PX,
                top: hostDragGhost.y + HOST_DRAG_GHOST_OFFSET_Y_PX
              }}
            >
              <div className="host-drag-ghost-name">{hostDisplayName(ghostHost)}</div>
              <div className="host-drag-ghost-meta">{hostSubtitle(ghostHost)}</div>
            </div>
          ) : null}
          {groupDragGhost && ghostGroup ? (
            <div
              className="host-drag-ghost"
              aria-hidden="true"
              data-tab-color={ghostGroup.color}
              style={{
                ...(ghostGroup.color ? sessionAccentStyle(ghostGroup.color) : {}),
                left: groupDragGhost.x + HOST_DRAG_GHOST_OFFSET_X_PX,
                top: groupDragGhost.y + HOST_DRAG_GHOST_OFFSET_Y_PX
              }}
            >
              <div className="host-drag-ghost-name">{ghostGroup.name}</div>
              <div className="host-drag-ghost-meta">
                {ghostGroup.hostIds.length === 1
                  ? '1 host'
                  : `${ghostGroup.hostIds.length} hosts`}
              </div>
            </div>
          ) : null}
        </>
      ) : null}
    </aside>
  )
}
