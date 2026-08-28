import { useEffect, useRef, useState } from 'react'
import type { SessionStatus } from '@shared/types'

/** Drop before the tab if the pointer is left of this fraction of its width */
const TAB_DROP_BEFORE_RATIO = 0.5

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
}

function insertIndexForHover(
  tabs: TabInfo[],
  fromId: string,
  overId: string,
  clientX: number,
  tabLeft: number,
  tabWidth: number
): number | null {
  const from = tabs.findIndex((t) => t.id === fromId)
  const over = tabs.findIndex((t) => t.id === overId)
  if (from < 0 || over < 0) {
    return null
  }
  const before = clientX < tabLeft + tabWidth * TAB_DROP_BEFORE_RATIO
  let insertIndex = over + (before ? 0 : 1)
  if (from < insertIndex) {
    insertIndex -= 1
  }
  if (insertIndex === from) {
    return null
  }
  return insertIndex
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
  const suppressClick = useRef(false)

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
    <div
      className="tab-bar"
      onDragOver={(e) => {
        if (!draggingId) {
          return
        }
        if (e.target !== e.currentTarget) {
          return
        }
        e.preventDefault()
        e.dataTransfer.dropEffect = 'move'
        const last = tabs.length - 1
        if (tabs[last]?.id !== draggingId) {
          onReorder(draggingId, last)
        }
      }}
    >
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          draggable
          className={`tab${tab.active ? ' active' : ''}${draggingId === tab.id ? ' dragging' : ''}`}
          data-tab-color={tab.tabColor || undefined}
          onClick={() => {
            if (suppressClick.current) {
              suppressClick.current = false
              return
            }
            onSelect(tab.id)
          }}
          onContextMenu={(e) => {
            e.preventDefault()
            e.stopPropagation()
            onSelect(tab.id)
            setMenu({
              id: tab.id,
              x: e.clientX,
              y: e.clientY,
              canSaveAsHost: tab.canSaveAsHost
            })
          }}
          onDragStart={(e) => {
            setMenu(null)
            setDraggingId(tab.id)
            e.dataTransfer.effectAllowed = 'move'
            e.dataTransfer.setData('text/plain', tab.id)
            onSelect(tab.id)
          }}
          onDragOver={(e) => {
            if (!draggingId) {
              return
            }
            e.preventDefault()
            e.dataTransfer.dropEffect = 'move'
            const rect = e.currentTarget.getBoundingClientRect()
            const nextIndex = insertIndexForHover(
              tabs,
              draggingId,
              tab.id,
              e.clientX,
              rect.left,
              rect.width
            )
            if (nextIndex !== null) {
              onReorder(draggingId, nextIndex)
            }
          }}
          onDrop={(e) => {
            e.preventDefault()
            setDraggingId(null)
          }}
          onDragEnd={() => {
            suppressClick.current = true
            setDraggingId(null)
          }}
        >
          <span className={`tab-status-dot ${tab.status}`} />
          <span className="tab-title">{tab.title}</span>
          <span
            className="tab-close"
            draggable={false}
            role="button"
            tabIndex={0}
            onClick={(e) => {
              e.stopPropagation()
              onClose(tab.id)
            }}
            onMouseDown={(e) => e.stopPropagation()}
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
      {menu ? (
        <div
          className="tab-context-menu"
          role="menu"
          style={{ left: menu.x, top: menu.y }}
        >
          <button type="button" role="menuitem" onClick={() => run(onReconnect)}>
            Reconnect
          </button>
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
