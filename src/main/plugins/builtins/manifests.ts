import type { PluginManifest, PluginMacroButton } from '../../../shared/plugins'
import {
  PLUGIN_ID_MACRO_PAD,
  PLUGIN_ID_SCRATCHPAD,
  PLUGIN_ID_SERVER_MONITOR,
  SERVER_MONITOR_DEFAULT_INTERVAL_MS
} from '../../../shared/plugins'

export const DEFAULT_MACRO_BUTTONS: PluginMacroButton[] = [
  { id: 'm1', label: 'ls', text: 'ls -la\n', hotkey: '' },
  { id: 'm2', label: 'pwd', text: 'pwd\n', hotkey: '' },
  { id: 'm3', label: 'clear', text: 'clear\n', hotkey: '' }
]

export const serverMonitorManifest: PluginManifest = {
  id: PLUGIN_ID_SERVER_MONITOR,
  name: 'Server monitor',
  version: '1.0.0',
  description: 'CPU, memory, and disk stats via SSH exec without disturbing the shell.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Monitor' },
    settingsHeading: 'Server monitor',
    settingsSchema: [
      {
        key: 'intervalMs',
        label: 'Poll interval (ms)',
        type: 'number',
        default: SERVER_MONITOR_DEFAULT_INTERVAL_MS,
        description: 'How often to refresh remote stats.'
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
  activation: 'auto',
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
    views: [{ id: 'panel', placement: 'split-bottom', title: 'Macros' }]
  }
}

export const BUILTIN_MANIFESTS: PluginManifest[] = [
  serverMonitorManifest,
  scratchpadManifest,
  macroPadManifest
]
