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
    views: [{ id: SCRATCHPAD_VIEW_ID, placement: 'split-right', title: 'Scratchpad' }],
    api: {
      methods: [
        {
          name: 'read_notes',
          description: 'Read the current scratchpad notes text for this session/host.',
          parameters: { type: 'object', properties: {} }
        },
        {
          name: 'write_notes',
          description: 'Overwrite the scratchpad notes with new text.',
          parameters: {
            type: 'object',
            properties: { content: { type: 'string', description: 'Full replacement notes text' } },
            required: ['content']
          }
        },
        {
          name: 'append_notes',
          description: 'Append a line of text to the end of the existing scratchpad notes.',
          parameters: {
            type: 'object',
            properties: { text: { type: 'string', description: 'Text to append' } },
            required: ['text']
          }
        }
      ]
    }
  }
}
