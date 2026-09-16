import type { ComponentType } from 'react'
import type { PluginViewProps, PluginSettingsViewProps, PluginRendererRegistration } from './api'
import ServerMonitorView from '../../../plugins/builtins/server-monitor/View'
import ScratchpadView from '../../../plugins/builtins/scratchpad/View'
import MacroPadView from '../../../plugins/builtins/macro-pad/View'
import MqttAnalyserView from '../../../plugins/builtins/mqtt-analyser/View'
import SftpView from '../../../plugins/builtins/sftp/View'
import AiAgentView from '../../../plugins/builtins/ai-agent/View'
import AiAgentProviderSettings from '../../../plugins/builtins/ai-agent/ProviderSettings'
import ConnectionLoggerView from '../../../plugins/builtins/connection-logger/View'
import DaemonMonitorView from '../../../plugins/builtins/daemon-monitor/View'
import { PLUGIN_ID_SERVER_MONITOR } from '../../../plugins/builtins/server-monitor/id'
import { PLUGIN_ID_DAEMON_MONITOR } from '../../../plugins/builtins/daemon-monitor/id'
import { PLUGIN_ID_SCRATCHPAD } from '../../../plugins/builtins/scratchpad/id'
import { PLUGIN_ID_MACRO_PAD } from '../../../plugins/builtins/macro-pad/id'
import { PLUGIN_ID_MQTT_ANALYSER } from '../../../plugins/builtins/mqtt-analyser/id'
import { PLUGIN_ID_SFTP } from '../../../plugins/builtins/sftp/id'
import { PLUGIN_ID_AI_AGENT } from '../../../plugins/builtins/ai-agent/id'
import { PLUGIN_ID_CONNECTION_LOGGER } from '../../../plugins/builtins/connection-logger/id'

const BUILTIN_RENDERER_PLUGINS: PluginRendererRegistration[] = [
  { id: PLUGIN_ID_SERVER_MONITOR, view: ServerMonitorView },
  { id: PLUGIN_ID_DAEMON_MONITOR, view: DaemonMonitorView },
  { id: PLUGIN_ID_SCRATCHPAD, view: ScratchpadView },
  { id: PLUGIN_ID_MACRO_PAD, view: MacroPadView },
  { id: PLUGIN_ID_MQTT_ANALYSER, view: MqttAnalyserView },
  { id: PLUGIN_ID_SFTP, view: SftpView },
  { id: PLUGIN_ID_AI_AGENT, view: AiAgentView, settingsView: AiAgentProviderSettings },
  { id: PLUGIN_ID_CONNECTION_LOGGER, view: ConnectionLoggerView }
]

const VIEW_REGISTRY = new Map<string, ComponentType<PluginViewProps>>(
  BUILTIN_RENDERER_PLUGINS.flatMap(({ id, view }) => (view ? [[id, view]] : []))
)

const SETTINGS_VIEW_REGISTRY = new Map<string, ComponentType<PluginSettingsViewProps>>(
  BUILTIN_RENDERER_PLUGINS.flatMap(({ id, settingsView }) =>
    settingsView ? [[id, settingsView]] : []
  )
)

export function getPluginView(pluginId: string): ComponentType<PluginViewProps> | null {
  return VIEW_REGISTRY.get(pluginId) ?? null
}

export function getPluginSettingsView(
  pluginId: string
): ComponentType<PluginSettingsViewProps> | null {
  return SETTINGS_VIEW_REGISTRY.get(pluginId) ?? null
}
