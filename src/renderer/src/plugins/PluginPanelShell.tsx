import type { PointerEvent as ReactPointerEvent, ReactNode } from 'react'

interface Props {
  pluginId: string
  title: string
  dragging: boolean
  onClose: (pluginId: string) => void
  onGripPointerDown: (pluginId: string, event: ReactPointerEvent) => void
  children: ReactNode
}

export default function PluginPanelShell({
  pluginId,
  title,
  dragging,
  onClose,
  onGripPointerDown,
  children
}: Props) {
  return (
    <div className={`plugin-panel-shell${dragging ? ' dragging' : ''}`}>
      <div className="plugin-panel-chrome">
        <span
          className="plugin-panel-drag"
          title="Drag to dock or split"
          aria-label={`Move ${title}`}
          role="button"
          tabIndex={0}
          onPointerDown={(e) => {
            if (e.button !== 0) {
              return
            }
            e.preventDefault()
            e.stopPropagation()
            onGripPointerDown(pluginId, e)
          }}
        >
          ⋮⋮
        </span>
        <span className="plugin-panel-chrome-title">{title}</span>
        <button
          type="button"
          className="plugin-panel-close"
          title={`Close ${title}`}
          aria-label={`Close ${title}`}
          onClick={() => onClose(pluginId)}
        >
          ×
        </button>
      </div>
      <div className="plugin-panel-body">{children}</div>
    </div>
  )
}
