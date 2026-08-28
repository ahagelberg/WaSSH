import type { SessionStatus } from '@shared/types'

export interface TabInfo {
  id: string
  title: string
  status: SessionStatus
  active: boolean
}

interface Props {
  tabs: TabInfo[]
  onSelect: (id: string) => void
  onClose: (id: string) => void
}

export default function TabBar({ tabs, onSelect, onClose }: Props) {
  if (tabs.length === 0) {
    return null
  }
  return (
    <div className="tab-bar">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          className={`tab${tab.active ? ' active' : ''}`}
          onClick={() => onSelect(tab.id)}
        >
          <span className={`tab-status-dot ${tab.status}`} />
          <span className="tab-title">{tab.title}</span>
          <span
            className="tab-close"
            role="button"
            tabIndex={0}
            onClick={(e) => {
              e.stopPropagation()
              onClose(tab.id)
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
    </div>
  )
}
