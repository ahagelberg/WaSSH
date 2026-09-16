import { PLUGIN_ID_SFTP } from './id'
import type { SftpRendererMessage } from './protocol'

interface UploadFilesOptions {
  tabId: string
  files: File[]
  path?: string
  pluginId?: string
}

/** Chunk size for streamed uploads; keeps each IPC payload small. */
const SFTP_UPLOAD_CHUNK_SIZE = 256 * 1024

function sendSftpMessage(
  tabId: string,
  payload: SftpRendererMessage,
  pluginId = PLUGIN_ID_SFTP
): Promise<unknown> {
  return window.wassh.sendPluginMessage(tabId, pluginId, payload)
}

/** Stream `files` to the plugin's main module, which writes them under `path`. */
export async function uploadFilesOverSftp({
  tabId,
  files,
  path,
  pluginId = PLUGIN_ID_SFTP
}: UploadFilesOptions): Promise<void> {
  for (const file of files) {
    try {
      await sendSftpMessage(
        tabId,
        { type: 'uploadStart', name: file.name, size: file.size, path },
        pluginId
      )
      let offset = 0
      while (offset < file.size) {
        const end = Math.min(offset + SFTP_UPLOAD_CHUNK_SIZE, file.size)
        const buffer = await file.slice(offset, end).arrayBuffer()
        await sendSftpMessage(
          tabId,
          { type: 'uploadChunk', name: file.name, data: new Uint8Array(buffer) },
          pluginId
        )
        offset = end
      }
      await sendSftpMessage(tabId, { type: 'uploadEnd', name: file.name }, pluginId)
    } catch {
      void sendSftpMessage(tabId, { type: 'cancel' }, pluginId)
    }
  }
}
