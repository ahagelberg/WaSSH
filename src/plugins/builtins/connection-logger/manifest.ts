import type { PluginManifest } from '@plugin-api/shared'
import { CONNECTION_LOGGER_DEFAULT_ENABLED, CONNECTION_LOGGER_DEFAULT_MAX_ENTRIES } from './defaults'
import { PLUGIN_ID_CONNECTION_LOGGER } from './id'

export const connectionLoggerManifest: PluginManifest = {
  id: PLUGIN_ID_CONNECTION_LOGGER,
  name: 'Connection logger',
  version: '1.0.0',
  description:
    'Logs connection and disconnection events to analyze network stability, with timeline graphs and uptime heatmaps.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Conn Log' },
    settingsHeading: 'Connection logger',
    settingsSchema: [
      {
        key: 'defaultEnabled',
        label: 'Enable connection logging by default',
        type: 'boolean',
        default: CONNECTION_LOGGER_DEFAULT_ENABLED,
        description: 'Record connection and disconnection events for new hosts and sessions.'
      },
      {
        key: 'retentionMaxEntries',
        label: 'Default log retention (entries)',
        type: 'number',
        default: CONNECTION_LOGGER_DEFAULT_MAX_ENTRIES,
        description: 'Maximum number of event log entries kept in history per host.'
      }
    ],
    hostSettingsHeading: 'Connection logger',
    hostSettingsSchema: [
      {
        key: 'enabled',
        label: 'Enable connection logging',
        type: 'boolean',
        default: CONNECTION_LOGGER_DEFAULT_ENABLED,
        description: 'Record all connection and disconnection events for this host.'
      },
      {
        key: 'maxEntries',
        label: 'Max log entries',
        type: 'number',
        default: CONNECTION_LOGGER_DEFAULT_MAX_ENTRIES,
        description: 'Maximum number of connection events to store for this host.'
      }
    ],
    views: [{ id: 'panel', placement: 'split-bottom', title: 'Connection Log' }]
  }
}
