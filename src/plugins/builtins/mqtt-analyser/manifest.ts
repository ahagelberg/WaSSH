import type { PluginManifest } from '@plugin-api/shared'
import { MQTT_ANALYSER_DEFAULT_HOST, MQTT_ANALYSER_DEFAULT_PORT } from './defaults'
import { PLUGIN_ID_MQTT_ANALYSER } from './id'

export const mqttAnalyserManifest: PluginManifest = {
  id: PLUGIN_ID_MQTT_ANALYSER,
  name: 'MQTT Analyser',
  version: '1.0.0',
  description: 'Browse topics and messages on an MQTT broker on the SSH host.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'MQTT' },
    hostSettingsHeading: 'MQTT',
    hostSettingsSchema: [
      {
        key: 'host',
        label: 'Broker host',
        type: 'string',
        default: MQTT_ANALYSER_DEFAULT_HOST,
        description: 'Broker address as seen from the remote SSH host (usually 127.0.0.1).'
      },
      {
        key: 'username',
        label: 'MQTT username',
        type: 'string',
        default: '',
        description: 'Optional MQTT username for this host.'
      },
      {
        key: 'password',
        label: 'MQTT password',
        type: 'string',
        default: '',
        secret: true,
        description: 'Optional MQTT password for this host.'
      },
      {
        key: 'port',
        label: 'Server port',
        type: 'number',
        default: MQTT_ANALYSER_DEFAULT_PORT,
        description: 'Plain MQTT port (no TLS).'
      }
    ],
    views: [{ id: 'panel', placement: 'split-right', title: 'MQTT' }]
  }
}
