import { useEffect, useState } from 'react'
import type { HostProfile } from '@shared/types'
import { hostDisplayName, hostSubtitle } from '@shared/connection'

interface Props {
  hosts: HostProfile[]
  collapsed: boolean
  onToggleCollapse: () => void
  onConnect: (host: HostProfile) => void
  onEdit: (host: HostProfile) => void
  onDelete: (host: HostProfile) => void
  onNewHost: () => void
}

/** Gap between gear button and menu */
const HOST_MENU_GAP_PX = 4

/** Approximate menu height (3 items) for flip-above calculation */
const HOST_MENU_ESTIMATED_HEIGHT_PX = 120

/** Minimum menu width; keep in sync with --host-menu-min-width */
const HOST_MENU_MIN_WIDTH_PX = 120

interface HostMenuState {
  hostId: string
  left: number
  top: number
}

function menuPositionForGear(gear: DOMRect): { left: number; top: number } {
  let left = gear.right - HOST_MENU_MIN_WIDTH_PX
  if (left < 0) {
    left = gear.left
  }
  if (left + HOST_MENU_MIN_WIDTH_PX > window.innerWidth) {
    left = Math.max(0, window.innerWidth - HOST_MENU_MIN_WIDTH_PX)
  }

  const openBelow = gear.bottom + HOST_MENU_GAP_PX
  const openAbove = gear.top - HOST_MENU_GAP_PX - HOST_MENU_ESTIMATED_HEIGHT_PX
  const fitsBelow =
    openBelow + HOST_MENU_ESTIMATED_HEIGHT_PX <= window.innerHeight
  const top = fitsBelow ? openBelow : Math.max(0, openAbove)

  return { left, top }
}

export default function SessionsSidebar({
  hosts,
  collapsed,
  onToggleCollapse,
  onConnect,
  onEdit,
  onDelete,
  onNewHost
}: Props) {
  const [menu, setMenu] = useState<HostMenuState | null>(null)

  useEffect(() => {
    if (!menu) {
      return
    }
    const close = (ev: MouseEvent): void => {
      const target = ev.target
      if (!(target instanceof Element)) {
        return
      }
      if (target.closest('.host-item-menu') || target.closest('.host-context-menu')) {
        return
      }
      setMenu(null)
    }
    const closeOnScroll = (): void => setMenu(null)
    document.addEventListener('mousedown', close)
    document.addEventListener('scroll', closeOnScroll, true)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('scroll', closeOnScroll, true)
    }
  }, [menu])

  const closeMenu = (): void => setMenu(null)

  const toggleMenu = (hostId: string, gearEl: HTMLElement): void => {
    setMenu((prev) => {
      if (prev?.hostId === hostId) {
        return null
      }
      const { left, top } = menuPositionForGear(gearEl.getBoundingClientRect())
      return { hostId, left, top }
    })
  }

  const menuHost = menu ? hosts.find((h) => h.id === menu.hostId) : undefined

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
          <div className="host-list">
            {hosts.length === 0 ? (
              <div className="host-item-meta">No saved hosts yet.</div>
            ) : (
              hosts.map((host) => {
                const proxy = host.proxyHostId
                  ? hosts.find((h) => h.id === host.proxyHostId)
                  : null
                const menuOpen = menu?.hostId === host.id
                return (
                  <div key={host.id} className="host-item">
                    <div
                      className="host-item-main"
                      onDoubleClick={() => onConnect(host)}
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
                    <div className={`host-item-menu${menuOpen ? ' open' : ''}`}>
                      <button
                        type="button"
                        className="icon-btn host-item-gear"
                        aria-label={`Actions for ${hostDisplayName(host)}`}
                        aria-expanded={menuOpen}
                        aria-haspopup="menu"
                        onClick={(e) => {
                          e.stopPropagation()
                          toggleMenu(host.id, e.currentTarget)
                        }}
                      >
                        ⚙
                      </button>
                    </div>
                  </div>
                )
              })
            )}
          </div>
          <div className="quick-connect">
            <button type="button" className="primary" onClick={onNewHost}>
              New host…
            </button>
          </div>
          {menu && menuHost ? (
            <div
              className="host-context-menu"
              role="menu"
              style={{ left: menu.left, top: menu.top }}
            >
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  closeMenu()
                  onConnect(menuHost)
                }}
              >
                Open
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  closeMenu()
                  onEdit(menuHost)
                }}
              >
                Edit
              </button>
              <button
                type="button"
                role="menuitem"
                className="danger"
                onClick={() => {
                  closeMenu()
                  onDelete(menuHost)
                }}
              >
                Delete
              </button>
            </div>
          ) : null}
        </>
      ) : null}
    </aside>
  )
}
