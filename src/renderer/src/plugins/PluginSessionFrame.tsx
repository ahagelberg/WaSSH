import type { ReactNode } from 'react'
import type { AppSettings } from '@shared/types'
import type { PluginListItem, PluginViewPlacement } from '@shared/plugins'
import { mergePluginSettings } from '@shared/plugins'
import { getPluginView, viewPlacementFor } from './registry'
import MacroPadView from './builtins/MacroPadView'

interface Props {
  tabId: string
  active: boolean
  plugins: PluginListItem[]
  activePluginIds: string[]
  settings: AppSettings
  onPluginSettingsPatch: (pluginId: string, partial: Record<string, unknown>) => void
  children: ReactNode
}

function placementClass(placement: PluginViewPlacement): string {
  switch (placement) {
    case 'overlay':
      return 'plugin-slot-overlay'
    case 'split-left':
      return 'plugin-slot-split-left'
    case 'split-right':
      return 'plugin-slot-split-right'
    case 'split-top':
      return 'plugin-slot-split-top'
    case 'split-bottom':
      return 'plugin-slot-split-bottom'
    default:
      return 'plugin-slot-overlay'
  }
}

export default function PluginSessionFrame({
  tabId,
  active,
  plugins,
  activePluginIds,
  settings,
  onPluginSettingsPatch,
  children
}: Props) {
  const activePlugins = plugins.filter((p) => p.enabled && activePluginIds.includes(p.id))

  const slots: Record<PluginViewPlacement, PluginListItem[]> = {
    overlay: [],
    'split-left': [],
    'split-right': [],
    'split-top': [],
    'split-bottom': []
  }

  for (const plugin of activePlugins) {
    const placement = viewPlacementFor(plugin)
    slots[placement].push(plugin)
  }

  const hasLeft = slots['split-left'].length > 0
  const hasRight = slots['split-right'].length > 0
  const hasTop = slots['split-top'].length > 0
  const hasBottom = slots['split-bottom'].length > 0

  const renderPlugin = (plugin: PluginListItem): ReactNode => {
    const View = getPluginView(plugin.id)
    if (!View) {
      return null
    }
    const pluginSettings = mergePluginSettings(
      plugin.contributes.settingsSchema,
      settings.pluginSettings[plugin.id]
    )
    const props = {
      tabId,
      pluginId: plugin.id,
      settings: pluginSettings,
      onSettingsPatch: (partial: Record<string, unknown>) =>
        onPluginSettingsPatch(plugin.id, partial)
    }
    if (plugin.id === 'macro-pad') {
      return <MacroPadView key={plugin.id} {...props} activeTab={active} />
    }
    return <View key={plugin.id} {...props} />
  }

  const layoutClass = [
    'plugin-session-frame',
    hasLeft ? 'has-left' : '',
    hasRight ? 'has-right' : '',
    hasTop ? 'has-top' : '',
    hasBottom ? 'has-bottom' : ''
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <div className={layoutClass}>
      {hasTop ? (
        <div className={`plugin-slot ${placementClass('split-top')}`}>
          {slots['split-top'].map(renderPlugin)}
        </div>
      ) : null}
      <div className="plugin-session-mid">
        {hasLeft ? (
          <div className={`plugin-slot ${placementClass('split-left')}`}>
            {slots['split-left'].map(renderPlugin)}
          </div>
        ) : null}
        <div className="plugin-session-terminal">{children}</div>
        {hasRight ? (
          <div className={`plugin-slot ${placementClass('split-right')}`}>
            {slots['split-right'].map(renderPlugin)}
          </div>
        ) : null}
      </div>
      {hasBottom ? (
        <div className={`plugin-slot ${placementClass('split-bottom')}`}>
          {slots['split-bottom'].map(renderPlugin)}
        </div>
      ) : null}
      {slots.overlay.length > 0 ? (
        <div className={`plugin-slot ${placementClass('overlay')}`}>
          {slots.overlay.map(renderPlugin)}
        </div>
      ) : null}
    </div>
  )
}
