import { useEffect, useState } from 'react'
import type { HostProfile } from '@shared/types'
import { hostDisplayName } from '@shared/connection'

interface Props {
  hosts: HostProfile[]
  collapsed: boolean
  onToggleCollapse: () => void
  onConnect: (host: HostProfile) => void
  onEdit: (host: HostProfile) => void
  onDelete: (host: HostProfile) => void
  onNewHost: () => void
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
  const [menuHostId, setMenuHostId] = useState<string | null>(null)

  useEffect(() => {
    if (!menuHostId) {
      return
    }
    const close = (ev: MouseEvent): void => {
      const target = ev.target
      if (!(target instanceof Element)) {
        return
      }
      if (target.closest('.host-item-menu')) {
        return
      }
      setMenuHostId(null)
    }
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [menuHostId])

  const closeMenu = (): void => setMenuHostId(null)

  const toggleMenu = (hostId: string): void => {
    setMenuHostId((prev) => (prev === hostId ? null : hostId))
  }

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
                const menuOpen = menuHostId === host.id
                return (
                  <div key={host.id} className="host-item">
                    <div
                      className="host-item-main"
                      onDoubleClick={() => onConnect(host)}
                      title="Double-click to connect"
                    >
                      <div className="host-item-name">{hostDisplayName(host)}</div>
                      <div className="host-item-meta">
                        {host.username ? `${host.username}@` : ''}
                        {host.host}:{host.port}
                        {proxy ? ` · via ${hostDisplayName(proxy)}` : ''}
                      </div>
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
                          toggleMenu(host.id)
                        }}
                      >
                        ⚙
                      </button>
                      {menuOpen ? (
                        <div className="host-context-menu" role="menu">
                          <button
                            type="button"
                            role="menuitem"
                            onClick={() => {
                              closeMenu()
                              onConnect(host)
                            }}
                          >
                            Open
                          </button>
                          <button
                            type="button"
                            role="menuitem"
                            onClick={() => {
                              closeMenu()
                              onEdit(host)
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
                              onDelete(host)
                            }}
                          >
                            Delete
                          </button>
                        </div>
                      ) : null}
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
        </>
      ) : null}
    </aside>
  )
}
