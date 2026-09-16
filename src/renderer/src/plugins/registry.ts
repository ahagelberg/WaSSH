import type { ComponentType } from 'react'
import type { PluginListItem } from '@shared/pluginApi'
import { getPluginView as getBuiltinPluginView } from './builtinRegistry'
import type { PluginViewProps } from './api'

export type { PluginViewProps } from './api'

export function getPluginView(pluginId: string): ComponentType<PluginViewProps> | null {
  return getBuiltinPluginView(pluginId)
}

export function enabledToolbarPlugins(plugins: PluginListItem[]): PluginListItem[] {
  return plugins.filter((p) => p.enabled && p.contributes.toolbar)
}
