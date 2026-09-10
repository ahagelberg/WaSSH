import type { ComponentType } from 'react'
import type { PluginViewProps, PluginRendererRegistration } from './api'
import ServerMonitorView from '../../../plugins/builtins/server-monitor/View'
import ScratchpadView from '../../../plugins/builtins/scratchpad/View'
import MacroPadView from '../../../plugins/builtins/macro-pad/View'
import MqttAnalyserView from '../../../plugins/builtins/mqtt-analyser/View'
import SftpView from '../../../plugins/builtins/sftp/View'
import AiAgentView from '../../../plugins/builtins/ai-agent/View'
import ConnectionLoggerView from '../../../plugins/builtins/connection-logger/View'
import { sftpFileDropHandler } from '../../../plugins/builtins/sftp/fileDrop'

export const BUILTIN_RENDERER_PLUGINS: PluginRendererRegistration[] = [
  { id: 'server-monitor', view: ServerMonitorView },
  { id: 'scratchpad', view: ScratchpadView },
  { id: 'macro-pad', view: MacroPadView },
  { id: 'mqtt-analyser', view: MqttAnalyserView },
  { id: 'sftp', view: SftpView, fileDrop: sftpFileDropHandler },
  { id: 'ai-agent', view: AiAgentView },
  { id: 'connection-logger', view: ConnectionLoggerView }
]

const VIEW_REGISTRY = new Map<string, ComponentType<PluginViewProps>>(
  BUILTIN_RENDERER_PLUGINS.flatMap(({ id, view }) => (view ? [[id, view]] : []))
)

export function getPluginView(pluginId: string): ComponentType<PluginViewProps> | null {
  return VIEW_REGISTRY.get(pluginId) ?? null
}

export function getPluginRenderer(pluginId: string): PluginRendererRegistration | undefined {
  return BUILTIN_RENDERER_PLUGINS.find((plugin) => plugin.id === pluginId)
}
