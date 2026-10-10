import type { PluginManifest } from '@plugin-api/shared'
import { PLUGIN_ID_SFTP, SFTP_PANEL_VIEW_ID } from './id'

export const sftpManifest: PluginManifest = {
  id: PLUGIN_ID_SFTP,
  name: 'Files',
  version: '1.0.0',
  description: 'Browse and manage files for SSH and Local sessions.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Files' },
    views: [{ id: SFTP_PANEL_VIEW_ID, placement: 'split-right', title: 'Files' }]
  }
}
