import type { PluginListItem } from '@shared/plugins'
import { enabledToolbarPlugins } from './registry'

interface Props {
  plugins: PluginListItem[]
  activePluginIds: string[]
  disabled: boolean
  onToggle: (pluginId: string, nextActive: boolean) => void
}

export default function PluginToolbar({ plugins, activePluginIds, disabled, onToggle }: Props) {
  const items = enabledToolbarPlugins(plugins)
  if (items.length === 0) {
    return null
  }

  return (
    <div className="plugin-toolbar" role="toolbar" aria-label="Session plugins">
      {items.map((p) => {
        const active = activePluginIds.includes(p.id)
        const label = p.contributes.toolbar?.label || p.name
        return (
          <button
            key={p.id}
            type="button"
            className={`plugin-toolbar-btn${active ? ' active' : ''}`}
            disabled={disabled}
            aria-pressed={active}
            title={p.description}
            onClick={() => onToggle(p.id, !active)}
          >
            {label}
          </button>
        )
      })}
    </div>
  )
}
