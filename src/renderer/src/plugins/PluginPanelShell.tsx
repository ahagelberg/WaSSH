import type { PointerEvent as ReactPointerEvent, ReactNode } from 'react'
import {
  PLUGIN_VIEW_PLACEMENT_LABELS,
  PLUGIN_VIEW_PLACEMENTS,
  type PluginViewPlacement
} from '@shared/plugins'

interface Props {
  pluginId: string
  title: string
  placement: PluginViewPlacement
  dragging: boolean
  onPlacementChange: (placement: PluginViewPlacement) => void
  onGripPointerDown: (pluginId: string, event: ReactPointerEvent) => void
  children: ReactNode
}

export default function PluginPanelShell({
  pluginId,
  title,
  placement,
  dragging,
  onPlacementChange,
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
        <label className="plugin-panel-placement">
          <span className="visually-hidden">Panel position</span>
          <select
            value={placement}
            aria-label={`Position for ${title}`}
            onChange={(e) => onPlacementChange(e.target.value as PluginViewPlacement)}
          >
            {PLUGIN_VIEW_PLACEMENTS.map((p) => (
              <option key={p} value={p}>
                {PLUGIN_VIEW_PLACEMENT_LABELS[p]}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="plugin-panel-body">{children}</div>
    </div>
  )
}
