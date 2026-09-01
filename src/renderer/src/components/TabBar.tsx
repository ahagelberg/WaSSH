import { useEffect, useRef, useState } from 'react'
import type { SessionStatus } from '@shared/types'
import { sessionAccentStyle } from '../sessionStyleCss'

/** Drop before the tab if the pointer is left of this fraction of its width */
const TAB_DROP_BEFORE_RATIO = 0.5
/** Pointer movement (px) before a press becomes a tab drag */
const TAB_DRAG_THRESHOLD_PX = 4

export interface TabInfo {
  id: string
  title: string
  status: SessionStatus
  active: boolean
  tabColor: string
  canSaveAsHost: boolean
}

interface Props {
  tabs: TabInfo[]
  onSelect: (id: string) => void
  onClose: (id: string) => void
  onReconnect: (id: string) => void
  onConfigure: (id: string) => void
  onSaveAsHost: (id: string) => void
  onReorder: (fromId: string, insertIndex: number) => void
}

interface TabMenu {
  id: string
  x: number
  y: number
  canSaveAsHost: boolean
  canReconnect: boolean
}

/** Statuses where a manual reconnect is useful */
const RECONNECT_STATUSES = new Set<SessionStatus>(['disconnected', 'failed'])

interface DragState {
  id: string
  startX: number
  startY: number
  active: boolean
  /** Gap index 0..tabs.length where the tab would be inserted */
  dropGap: number | null
}

interface DropIndicator {
  left: number
}

/** Gap index under the pointer: 0 = before first tab, length = after last. */
function gapIndexAtX(bar: HTMLElement, clientX: number, tabCount: number): number {
  const nodes = bar.querySelectorAll<HTMLElement>('[data-tab-id]')
  for (let i = 0; i < nodes.length; i++) {
    const rect = nodes[i].getBoundingClientRect()
    const mid = rect.left + rect.width * TAB_DROP_BEFORE_RATIO
    if (clientX < mid) {
      return i
    }
  }
  return tabCount
}

/** Pixel offset of the drop line relative to the tab bar, for gap `gap`. */
function indicatorLeftForGap(bar: HTMLElement, gap: number): number {
  const nodes = bar.querySelectorAll<HTMLElement>('[data-tab-id]')
  const barRect = bar.getBoundingClientRect()
  if (nodes.length === 0) {
    return 0
  }
  if (gap >= nodes.length) {
    const last = nodes[nodes.length - 1]
    return last.getBoundingClientRect().right - barRect.left
  }
  return nodes[gap].getBoundingClientRect().left - barRect.left
}

/**
 * Convert a visual gap index to the insert index expected by onReorder
 * (index in the array after the dragged tab is removed).
 * Returns null when the drop would not change order.
 */
function insertIndexFromGap(from: number, gap: number): number | null {
  if (gap === from || gap === from + 1) {
    return null
  }
  return gap > from ? gap - 1 : gap
}

export default function TabBar({
  tabs,
  onSelect,
  onClose,
  onReconnect,
  onConfigure,
  onSaveAsHost,
  onReorder
}: Props) {
  const [menu, setMenu] = useState<TabMenu | null>(null)
  const [draggingId, setDraggingId] = useState<string | null>(null)
  const [dropIndicator, setDropIndicator] = useState<DropIndicator | null>(null)
  const suppressClick = useRef(false)
  const barRef = useRef<HTMLDivElement>(null)
  const dragRef = useRef<DragState | null>(null)
  const tabsRef = useRef(tabs)
  const onReorderRef = useRef(onReorder)

  tabsRef.current = tabs
  onReorderRef.current = onReorder

  useEffect(() => {
    if (!menu) {
      return
    }
    const close = (ev: MouseEvent): void => {
      const target = ev.target
      if (!(target instanceof Element)) {
        return
      }
      if (target.closest('.tab-context-menu')) {
        return
      }
      setMenu(null)
    }
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [menu])

  useEffect(() => {
    const onMove = (e: PointerEvent): void => {
      const drag = dragRef.current
      if (!drag) {
        return
      }
      if (!drag.active) {
        const dx = e.clientX - drag.startX
        const dy = e.clientY - drag.startY
        if (dx * dx + dy * dy < TAB_DRAG_THRESHOLD_PX * TAB_DRAG_THRESHOLD_PX) {
          return
        }
        drag.active = true
        setMenu(null)
        setDraggingId(drag.id)
      }

      const bar = barRef.current
      if (!bar) {
        return
      }
      const list = tabsRef.current
      const barRect = bar.getBoundingClientRect()
      if (
        e.clientY < barRect.top ||
        e.clientY > barRect.bottom ||
        e.clientX < barRect.left ||
        e.clientX > barRect.right
      ) {
        return
      }

      const from = list.findIndex((t) => t.id === drag.id)
      if (from < 0) {
        return
      }
      const gap = gapIndexAtX(bar, e.clientX, list.length)
      drag.dropGap = gap
      const insertIndex = insertIndexFromGap(from, gap)
      if (insertIndex === null) {
        setDropIndicator(null)
        return
      }
      setDropIndicator({ left: indicatorLeftForGap(bar, gap) })
    }

    const onUp = (): void => {
      const drag = dragRef.current
      if (!drag) {
        return
      }
      if (drag.active) {
        suppressClick.current = true
        const list = tabsRef.current
        const from = list.findIndex((t) => t.id === drag.id)
        if (from >= 0 && drag.dropGap !== null) {
          const insertIndex = insertIndexFromGap(from, drag.dropGap)
          if (insertIndex !== null) {
            onReorderRef.current(drag.id, insertIndex)
          }
        }
      }
      dragRef.current = null
      setDraggingId(null)
      setDropIndicator(null)
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

  if (tabs.length === 0) {
    return null
  }

  const run = (action: (id: string) => void): void => {
    if (!menu) {
      return
    }
    const id = menu.id
    setMenu(null)
    action(id)
  }

  return (
    <div className={`tab-bar${draggingId ? ' is-dragging' : ''}`} ref={barRef}>
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          data-tab-id={tab.id}
          className={`tab${tab.active && draggingId !== tab.id ? ' active' : ''}${draggingId === tab.id ? ' dragging' : ''}`}
          data-tab-color={tab.tabColor || undefined}
          style={sessionAccentStyle(tab.tabColor)}
          onMouseDown={(e) => {
            // Don't move focus into the tab bar — TerminalView refocuses the active pane.
            // HTML5 drag cannot start after preventDefault, so reorder uses pointer events.
            if (e.button === 0) {
              e.preventDefault()
            }
          }}
          onPointerDown={(e) => {
            if (e.button !== 0) {
              return
            }
            if (e.target instanceof Element && e.target.closest('.tab-close')) {
              return
            }
            dragRef.current = {
              id: tab.id,
              startX: e.clientX,
              startY: e.clientY,
              active: false,
              dropGap: null
            }
          }}
          onClick={() => {
            if (suppressClick.current) {
              suppressClick.current = false
              return
            }
            onSelect(tab.id)
          }}
          onDoubleClick={(e) => {
            if (e.target instanceof Element && e.target.closest('.tab-close')) {
              return
            }
            onConfigure(tab.id)
          }}
          onContextMenu={(e) => {
            e.preventDefault()
            e.stopPropagation()
            onSelect(tab.id)
            setMenu({
              id: tab.id,
              x: e.clientX,
              y: e.clientY,
              canSaveAsHost: tab.canSaveAsHost,
              canReconnect: RECONNECT_STATUSES.has(tab.status)
            })
          }}
        >
          <span className={`tab-status-dot ${tab.status}`} />
          <span className="tab-title">{tab.title}</span>
          <span
            className="tab-close"
            role="button"
            tabIndex={-1}
            onClick={(e) => {
              e.stopPropagation()
              onClose(tab.id)
            }}
            onMouseDown={(e) => {
              e.preventDefault()
              e.stopPropagation()
            }}
            onPointerDown={(e) => {
              e.stopPropagation()
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.stopPropagation()
                onClose(tab.id)
              }
            }}
          >
            ×
          </span>
        </button>
      ))}
      {dropIndicator ? (
        <div
          className="tab-drop-indicator"
          aria-hidden="true"
          style={{ left: dropIndicator.left }}
        />
      ) : null}
      {menu ? (
        <div
          className="tab-context-menu"
          role="menu"
          style={{ left: menu.x, top: menu.y }}
        >
          {menu.canReconnect ? (
            <button type="button" role="menuitem" onClick={() => run(onReconnect)}>
              Reconnect
            </button>
          ) : null}
          <button type="button" role="menuitem" onClick={() => run(onConfigure)}>
            Session settings
          </button>
          {menu.canSaveAsHost ? (
            <button type="button" role="menuitem" onClick={() => run(onSaveAsHost)}>
              Save as host…
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
