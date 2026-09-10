import type { PluginMainRegistration } from './api'
import type { PluginManifest } from '../../shared/pluginApi'
import { aiAgentMain } from '../../plugins/builtins/ai-agent/main'
import { aiAgentManifest } from '../../plugins/builtins/ai-agent/manifest'
import { connectionLoggerMain } from '../../plugins/builtins/connection-logger/main'
import { connectionLoggerManifest } from '../../plugins/builtins/connection-logger/manifest'
import { macroPadMain } from '../../plugins/builtins/macro-pad/main'
import { macroPadManifest } from '../../plugins/builtins/macro-pad/manifest'
import { mqttAnalyserMain } from '../../plugins/builtins/mqtt-analyser/main'
import { mqttAnalyserManifest } from '../../plugins/builtins/mqtt-analyser/manifest'
import { scratchpadMain } from '../../plugins/builtins/scratchpad/main'
import { scratchpadManifest } from '../../plugins/builtins/scratchpad/manifest'
import { serverMonitorMain } from '../../plugins/builtins/server-monitor/main'
import { serverMonitorManifest } from '../../plugins/builtins/server-monitor/manifest'
import { sftpMain } from '../../plugins/builtins/sftp/main'
import { sftpManifest } from '../../plugins/builtins/sftp/manifest'

export interface BuiltinPluginDefinition {
  manifest: PluginManifest
  registration: PluginMainRegistration
}

export const BUILTIN_PLUGIN_DEFINITIONS: BuiltinPluginDefinition[] = [
  { manifest: serverMonitorManifest, registration: { id: serverMonitorManifest.id, module: serverMonitorMain } },
  { manifest: scratchpadManifest, registration: { id: scratchpadManifest.id, module: scratchpadMain } },
  { manifest: macroPadManifest, registration: { id: macroPadManifest.id, module: macroPadMain } },
  { manifest: mqttAnalyserManifest, registration: { id: mqttAnalyserManifest.id, module: mqttAnalyserMain } },
  {
    manifest: sftpManifest,
    registration: {
      id: sftpManifest.id,
      module: sftpMain,
      backgroundActivation: 'ssh',
      retainBackgroundOnViewClose: true
    }
  },
  { manifest: aiAgentManifest, registration: { id: aiAgentManifest.id, module: aiAgentMain } },
  {
    manifest: connectionLoggerManifest,
    registration: {
      id: connectionLoggerManifest.id,
      module: connectionLoggerMain,
      backgroundActivation: 'session'
    }
  }
]

export const BUILTIN_MANIFESTS = BUILTIN_PLUGIN_DEFINITIONS.map(({ manifest }) => manifest)
export const DEFAULT_ENABLED_PLUGIN_IDS = BUILTIN_MANIFESTS.map(({ id }) => id)
