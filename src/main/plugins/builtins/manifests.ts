import type { PluginManifest, PluginMacroButton } from '../../../shared/plugins'
import {
  PLUGIN_ID_AI_AGENT,
  PLUGIN_ID_MACRO_PAD,
  PLUGIN_ID_MQTT_EXPLORER,
  PLUGIN_ID_SCRATCHPAD,
  PLUGIN_ID_SERVER_MONITOR,
  PLUGIN_ID_SFTP,
  AI_AGENT_SAFE_RULES,
  AI_AGENT_SETTING_DEFAULT_ALLOW_RULES,
  AI_AGENT_SETTING_DEFAULT_DENY_RULES,
  AI_AGENT_SETTING_HOST_ALLOW_RULES,
  AI_AGENT_SETTING_HOST_DENY_RULES,
  MQTT_EXPLORER_DEFAULT_HOST,
  MQTT_EXPLORER_DEFAULT_PORT,
  SERVER_MONITOR_DEFAULT_INTERVAL_MS,
  SERVER_MONITOR_SHOW_GAUGES_DEFAULT,
  SERVER_MONITOR_SHOW_NETWORK_DEFAULT,
  SERVER_MONITOR_SHOW_PROCESSES_DEFAULT,
  SERVER_MONITOR_SHOW_SPARKS_DEFAULT,
  SERVER_MONITOR_SHOW_STATUS_DEFAULT
} from '../../../shared/plugins'

export const DEFAULT_MACRO_BUTTONS: PluginMacroButton[] = [
  {
    id: 'm1',
    label: 'syslog',
    text: 'tail -n 100 /var/log/syslog\n',
    hotkey: ''
  },
  {
    id: 'm2',
    label: 'apt upgrade',
    text: 'sudo apt update;sudo apt upgrade -y;sudo apt autoremove -y\n',
    hotkey: ''
  },
  {
    id: 'm3',
    label: 'create venv',
    text: 'python3 -m venv .venv\n',
    hotkey: ''
  }
]

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
    views: [{ id: 'panel', placement: 'split-right', title: 'Server' }]
  }
}

export const scratchpadManifest: PluginManifest = {
  id: PLUGIN_ID_SCRATCHPAD,
  name: 'Scratchpad',
  version: '1.0.0',
  description: 'Notes and scratch text shared across sessions.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Scratch' },
    views: [{ id: 'panel', placement: 'split-bottom', title: 'Scratchpad' }]
  }
}

export const macroPadManifest: PluginManifest = {
  id: PLUGIN_ID_MACRO_PAD,
  name: 'Macro pad',
  version: '1.0.0',
  description: 'Configurable buttons and hotkeys that inject text into the terminal.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Macros' },
    settingsHeading: 'Macro pad',
    settingsSchema: [
      {
        key: 'buttons',
        label: 'Buttons',
        type: 'macroList',
        default: DEFAULT_MACRO_BUTTONS,
        description: 'Label, text to send, and optional hotkey per button.'
      }
    ],
    views: [{ id: 'panel', placement: 'split-right', title: 'Macros' }]
  }
}

export const mqttExplorerManifest: PluginManifest = {
  id: PLUGIN_ID_MQTT_EXPLORER,
  name: 'MQTT explorer',
  version: '1.0.0',
  description: 'Browse topics and messages on an MQTT broker on the SSH host.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'MQTT' },
    settingsHeading: 'MQTT explorer',
    settingsSchema: [
      {
        key: 'host',
        label: 'Broker host',
        type: 'string',
        default: MQTT_EXPLORER_DEFAULT_HOST,
        description: 'Broker address as seen from the remote SSH host (usually 127.0.0.1).'
      }
    ],
    hostSettingsHeading: 'MQTT',
    hostSettingsSchema: [
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
        default: MQTT_EXPLORER_DEFAULT_PORT,
        description: 'Plain MQTT port (no TLS).'
      }
    ],
    views: [{ id: 'panel', placement: 'split-right', title: 'MQTT' }]
  }
}

export const sftpManifest: PluginManifest = {
  id: PLUGIN_ID_SFTP,
  name: 'SFTP files',
  version: '1.0.0',
  description:
    'Remote file manager over SFTP with drag-and-drop upload onto the terminal.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Files' },
    views: [{ id: 'panel', placement: 'split-right', title: 'Files' }]
  }
}

export const aiAgentManifest: PluginManifest = {
  id: PLUGIN_ID_AI_AGENT,
  name: 'AI agent',
  version: '1.0.0',
  description:
    'Chat with an LLM that can run commands on the remote host. Commands outside the safe/allow lists ask for approval.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'AI' },
    settingsHeading: 'AI agent',
    settingsSchema: [
      {
        key: AI_AGENT_SETTING_DEFAULT_ALLOW_RULES,
        label: 'Default allow rules',
        type: 'stringList',
        default: AI_AGENT_SAFE_RULES,
        description:
          'App-wide command patterns that run without asking (applies to every host). One per line; glob by default, prefix a line with "regex:" for a regular expression. Host allow rules are checked first.'
      },
      {
        key: AI_AGENT_SETTING_DEFAULT_DENY_RULES,
        label: 'Default deny rules',
        type: 'stringList',
        default: [],
        description:
          'App-wide command patterns that are always blocked. Deny always wins over allow.'
      }
    ],
    hostSettingsHeading: 'AI agent',
    hostSettingsSchema: [
      {
        key: AI_AGENT_SETTING_HOST_ALLOW_RULES,
        label: 'Allow rules',
        type: 'stringList',
        default: [],
        description:
          'Command patterns for this host that run without asking. One per line; glob by default, prefix with "regex:" for a regular expression.'
      },
      {
        key: AI_AGENT_SETTING_HOST_DENY_RULES,
        label: 'Deny rules',
        type: 'stringList',
        default: [],
        description:
          'Command patterns for this host that are always blocked. Deny always wins over allow.'
      }
    ],
    views: [{ id: 'panel', placement: 'split-right', title: 'AI agent' }]
  }
}

export const BUILTIN_MANIFESTS: PluginManifest[] = [
  serverMonitorManifest,
  scratchpadManifest,
  macroPadManifest,
  mqttExplorerManifest,
  sftpManifest,
  aiAgentManifest
]
