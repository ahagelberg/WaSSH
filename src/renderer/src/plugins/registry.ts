import type { ComponentType } from 'react'
import type { PluginListItem, PluginViewPlacement } from '@shared/pluginApi'
import { getPluginView as getBuiltinPluginView } from './builtinRegistry'
import type { PluginViewProps } from './api'

export type { PluginViewProps } from './api'

export function getPluginView(pluginId: string): ComponentType<PluginViewProps> | null {
  return getBuiltinPluginView(pluginId)
}

export function viewPlacementFor(
  plugin: PluginListItem,
  viewId = 'panel'
): PluginViewPlacement {
  const view = plugin.contributes.views?.find((v) => v.id === viewId)
  return view?.placement ?? 'overlay'
}

export function enabledToolbarPlugins(plugins: PluginListItem[]): PluginListItem[] {
  return plugins.filter((p) => p.enabled && p.contributes.toolbar)
}
