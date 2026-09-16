import type { ComponentType } from 'react'

export interface PluginViewProps {
  tabId: string
  pluginId: string
  /** Saved host profile id this session is bound to; null for unsaved sessions. */
  hostId: string | null
  /** Whether this tab is currently visible. */
  active: boolean
  settings: Record<string, unknown>
  onSettingsPatch: (partial: Record<string, unknown>) => void
}

/** Props for a plugin's own settings editor, opened from the Options dialog. */
export interface PluginSettingsViewProps {
  /** Tab the settings dialog was opened from; null when no session is active. */
  tabId: string | null
  /** Merged app+host settings for this plugin. */
  settings: Record<string, unknown>
  /** Persist a partial settings patch. */
  onSettingsPatch: (partial: Record<string, unknown>) => void
  /** Close the settings editor. */
  onClose: () => void
}

export interface PluginRendererRegistration {
  id: string
  view?: ComponentType<PluginViewProps>
  /**
   * Custom settings editor. Required when the manifest sets
   * `settingsPresentation: 'view'`; the host renders a button that opens it.
   */
  settingsView?: ComponentType<PluginSettingsViewProps>
}

export { default as PluginColorInput } from './PluginColorInput'
export { default as PluginButton } from './PluginButton'
export { default as PluginField } from './PluginField'
export { default as PluginSettingsFieldList } from './PluginSettingsFieldList'
export { default as PluginFieldEditor } from './PluginFieldEditor'
export { isFileDrag, collectDroppedFiles, type FileDragEventLike } from './fileDrag'
