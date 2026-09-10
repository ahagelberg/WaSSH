import type { PluginManifest } from '@plugin-api/shared'
import {
  SERVER_MONITOR_DEFAULT_INTERVAL_MS,
  SERVER_MONITOR_SHOW_GAUGES_DEFAULT,
  SERVER_MONITOR_SHOW_NETWORK_DEFAULT,
  SERVER_MONITOR_SHOW_PROCESSES_DEFAULT,
  SERVER_MONITOR_SHOW_SPARKS_DEFAULT,
  SERVER_MONITOR_SHOW_STATUS_DEFAULT
} from './defaults'
import { PLUGIN_ID_SERVER_MONITOR } from './id'

export const serverMonitorManifest: PluginManifest = {
  id: PLUGIN_ID_SERVER_MONITOR,
  name: 'Server monitor',
  version: '1.1.0',
  description:
    'htop/btop-style remote stats: per-core CPU, memory breakdown, disk I/O, processes, network, and temps via SSH exec.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Monitor' },
    hostSettingsHeading: 'Server monitor',
    hostSettingsSchema: [
      {
        key: 'intervalMs',
        label: 'Poll interval (ms)',
        type: 'number',
        default: SERVER_MONITOR_DEFAULT_INTERVAL_MS,
        description: 'How often to refresh remote stats for this session.'
      },
      {
        key: 'showGauges',
        label: 'Show gauges',
        type: 'boolean',
        default: SERVER_MONITOR_SHOW_GAUGES_DEFAULT,
        description: 'CPU, memory, and swap gauges, per-core bars, and memory breakdown.'
      },
      {
        key: 'showSparks',
        label: 'Show history graphs',
        type: 'boolean',
        default: SERVER_MONITOR_SHOW_SPARKS_DEFAULT,
        description: 'Sparkline history inside CPU, network, and disk panels.'
      },
      {
        key: 'showStatus',
        label: 'Show status',
        type: 'boolean',
        default: SERVER_MONITOR_SHOW_STATUS_DEFAULT,
        description: 'OS, uptime, load, temperatures, kernel, and task counts.'
      },
      {
        key: 'showProcesses',
        label: 'Show process list',
        type: 'boolean',
        default: SERVER_MONITOR_SHOW_PROCESSES_DEFAULT,
        description: 'Top processes with state, nice, threads, and sort by CPU/mem.'
      },
      {
        key: 'showNetwork',
        label: 'Show network',
        type: 'boolean',
        default: SERVER_MONITOR_SHOW_NETWORK_DEFAULT,
        description: 'Network panel with rate history, totals, and per-interface stats.'
      }
    ],
    views: [{ id: 'panel', placement: 'split-right', title: 'Server' }],
    api: {
      methods: [
        {
          name: 'get_snapshot',
          description:
            'Return the latest sampled server-monitor stats snapshot for this session (CPU, memory, disk, network, processes, temperatures).',
          parameters: { type: 'object', properties: {} }
        }
      ]
    }
  }
}
