import type { ComponentType } from 'react'
import type { PluginListItem, PluginViewPlacement } from '@shared/plugins'
import ServerMonitorView from './builtins/ServerMonitorView'
import ScratchpadView from './builtins/ScratchpadView'
import MacroPadView from './builtins/MacroPadView'
import MqttExplorerView from './builtins/MqttExplorerView'
import SftpView from './builtins/SftpView'

export interface PluginViewProps {
  tabId: string
  pluginId: string
  settings: Record<string, unknown>
  onSettingsPatch: (partial: Record<string, unknown>) => void
}

const VIEW_REGISTRY: Record<string, ComponentType<PluginViewProps>> = {
  'server-monitor': ServerMonitorView,
  scratchpad: ScratchpadView,
  'macro-pad': MacroPadView,
  'mqtt-explorer': MqttExplorerView,
  sftp: SftpView
}

export function getPluginView(pluginId: string): ComponentType<PluginViewProps> | null {
  return VIEW_REGISTRY[pluginId] ?? null
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
