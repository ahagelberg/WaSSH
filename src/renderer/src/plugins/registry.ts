import type { ComponentType } from 'react'
import type { PluginListItem } from '@shared/pluginApi'
import {
  getPluginView as getBuiltinPluginView,
  getPluginSettingsView as getBuiltinPluginSettingsView
} from './builtinRegistry'
import type { PluginViewProps, PluginSettingsViewProps } from './api'

export type { PluginViewProps, PluginSettingsViewProps } from './api'

export function getPluginView(pluginId: string): ComponentType<PluginViewProps> | null {
  return getBuiltinPluginView(pluginId)
}

export function getPluginSettingsView(
  pluginId: string
): ComponentType<PluginSettingsViewProps> | null {
  return getBuiltinPluginSettingsView(pluginId)
}

export function enabledToolbarPlugins(plugins: PluginListItem[]): PluginListItem[] {
  return plugins.filter((p) => p.enabled && p.contributes.toolbar)
}
