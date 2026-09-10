import type { PluginManifest } from '@plugin-api/shared'
import { PLUGIN_ID_SCRATCHPAD, SCRATCHPAD_VIEW_ID } from './id'

export const scratchpadManifest: PluginManifest = {
  id: PLUGIN_ID_SCRATCHPAD,
  name: 'Scratchpad',
  version: '1.0.0',
  description: 'Notes shared across sessions of the same host.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Scratch' },
    views: [{ id: SCRATCHPAD_VIEW_ID, placement: 'split-right', title: 'Scratchpad' }]
  }
}
