import type { PluginManifest } from '../../../shared/pluginApi'
import { PLUGIN_ID_SFTP, SFTP_PANEL_VIEW_ID } from './id'

export const sftpManifest: PluginManifest = {
  id: PLUGIN_ID_SFTP,
  name: 'SFTP files',
  version: '1.0.0',
  description: 'Remote file manager over SFTP with drag-and-drop upload onto the terminal.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Files' },
    views: [{ id: SFTP_PANEL_VIEW_ID, placement: 'split-right', title: 'Files' }],
    terminalFileDrop: {
      label: 'Upload dropped files over SFTP'
    }
  }
}
