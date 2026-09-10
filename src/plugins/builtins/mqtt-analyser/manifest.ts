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
    views: [{ id: 'panel', placement: 'split-right', title: 'MQTT' }],
    api: {
      methods: [
        {
          name: 'get_topics',
          description:
            'List known MQTT topics for this session, each with its last known payload and message count.',
          parameters: { type: 'object', properties: {} }
        },
        {
          name: 'get_topic_value',
          description: 'Get the latest value of one MQTT topic, optionally with its recent message history.',
          parameters: {
            type: 'object',
            properties: {
              topic: { type: 'string', description: 'Exact MQTT topic to read' },
              includeHistory: { type: 'boolean', description: 'Include recent message history (default false)' }
            },
            required: ['topic']
          }
        },
        {
          name: 'publish',
          description: 'Publish a text value to an MQTT topic.',
          defaultPermission: 'ask',
          parameters: {
            type: 'object',
            properties: {
              topic: { type: 'string', description: 'MQTT topic to publish to' },
              payload: { type: 'string', description: 'Text payload to publish' },
              qos: { type: 'number', description: 'QoS level 0, 1, or 2 (default 0)' },
              retain: { type: 'boolean', description: 'Whether to set the retain flag (default false)' }
            },
            required: ['topic', 'payload']
          }
        }
      ]
    }
  }
}
