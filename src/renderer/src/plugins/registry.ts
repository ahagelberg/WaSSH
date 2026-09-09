import type { ComponentType } from 'react'
import type { PluginListItem, PluginViewPlacement } from '@shared/plugins'
import {
  PLUGIN_ID_AI_AGENT,
  PLUGIN_ID_CONNECTION_LOGGER,
  PLUGIN_ID_MACRO_PAD,
  PLUGIN_ID_MQTT_ANALYSER,
  PLUGIN_ID_SCRATCHPAD,
  PLUGIN_ID_SERVER_MONITOR,
  PLUGIN_ID_SFTP
} from '@shared/plugins'
import ServerMonitorView from './builtins/ServerMonitorView'
import ScratchpadView from './builtins/ScratchpadView'
import MacroPadView from './builtins/MacroPadView'
import MqttAnalyserView from './builtins/MqttAnalyserView'
import SftpView from './builtins/SftpView'
import AiAgentView from './builtins/AiAgentView'
import ConnectionLoggerView from './builtins/ConnectionLoggerView'

export interface PluginViewProps {
  tabId: string
  pluginId: string
  /** Saved host profile id this session is bound to; null for unsaved sessions */
  hostId: string | null
  settings: Record<string, unknown>
  onSettingsPatch: (partial: Record<string, unknown>) => void
}

const VIEW_REGISTRY: Record<string, ComponentType<PluginViewProps>> = {
  [PLUGIN_ID_SERVER_MONITOR]: ServerMonitorView,
  [PLUGIN_ID_SCRATCHPAD]: ScratchpadView,
  [PLUGIN_ID_MACRO_PAD]: MacroPadView,
  [PLUGIN_ID_MQTT_ANALYSER]: MqttAnalyserView,
  [PLUGIN_ID_SFTP]: SftpView,
  [PLUGIN_ID_AI_AGENT]: AiAgentView,
  [PLUGIN_ID_CONNECTION_LOGGER]: ConnectionLoggerView
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
