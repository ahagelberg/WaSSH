import type { PluginManifest } from '@plugin-api/shared'
import { DEFAULT_MACRO_BUTTONS, MACRO_PAD_UNGROUPED_COLLAPSED_DEFAULT } from './defaults'
import { PLUGIN_ID_MACRO_PAD, MACRO_PAD_VIEW_ID } from './id'

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
    // Options dialog hides this section (macros are configured in the pane).
    // Keys below exist so patch routing + merging keep the extra settings keys.
    settingsSchema: [
      {
        key: 'buttons',
        label: 'Buttons',
        type: 'macroList',
        default: DEFAULT_MACRO_BUTTONS,
        description: 'Label, text to send, and optional hotkey per button.'
      },
      {
        key: 'groups',
        label: 'Groups',
        type: 'macroList',
        default: [],
        description: 'Macro groups (name, color, collapsed state).'
      },
      {
        key: 'ungroupedCollapsed',
        label: 'Collapse ungrouped',
        type: 'boolean',
        default: MACRO_PAD_UNGROUPED_COLLAPSED_DEFAULT,
        description: 'Whether the ungrouped macro section is collapsed.'
      }
    ],
    views: [{ id: MACRO_PAD_VIEW_ID, placement: 'split-right', title: 'Macros' }]
  }
}
