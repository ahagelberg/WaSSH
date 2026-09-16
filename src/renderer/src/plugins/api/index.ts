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

export interface PluginRendererRegistration {
  id: string
  view?: ComponentType<PluginViewProps>
}

export { default as PluginColorInput } from './PluginColorInput'
export { default as PluginButton } from './PluginButton'
export { default as PluginField } from './PluginField'
export { default as PluginSettingsFieldList } from './PluginSettingsFieldList'
export { default as PluginFieldEditor } from './PluginFieldEditor'
export { isFileDrag, collectDroppedFiles, type FileDragEventLike } from './fileDrag'
