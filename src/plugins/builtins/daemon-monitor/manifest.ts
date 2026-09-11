import type { PluginManifest } from '@plugin-api/shared'
import { DAEMON_MONITOR_SAMPLE_INTERVAL_SEC } from './defaults'
import { PLUGIN_ID_DAEMON_MONITOR } from './id'

export const daemonMonitorManifest: PluginManifest = {
  id: PLUGIN_ID_DAEMON_MONITOR,
  name: 'Daemon monitor',
  version: '1.0.0',
  description:
    'Lightweight persistent Linux monitoring with retained CPU, memory, disk, interface, and reachability history.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Daemon monitor' },
    hostSettingsHeading: 'Daemon monitor',
    hostSettingsSchema: [
      {
        key: 'pingTargets',
        label: 'Ping targets',
        type: 'stringList',
        default: [],
        description: 'Hostnames or IP addresses checked by the remote service.'
      }
    ],
    views: [{ id: 'panel', placement: 'split-right', title: 'Daemon monitor' }],
    api: {
      methods: [
        {
          name: 'get_status',
          description: `Return the latest daemon state and ${DAEMON_MONITOR_SAMPLE_INTERVAL_SEC}-second system sample.`,
          parameters: { type: 'object', properties: {} }
        }
      ]
    }
  }
}
