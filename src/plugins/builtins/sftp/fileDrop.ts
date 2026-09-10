import type { PluginActiveStateEvent, PluginMessageEvent } from '../../../shared/pluginApi'
import type { PluginFileDropHandler, PluginFileDropProgress } from '../../../renderer/src/plugins/api'
import { PLUGIN_ID_SFTP } from './id'
import type { SftpRendererMessage, SftpStatusPayload } from './protocol'

interface UploadFilesOptions {
  tabId: string
  files: File[]
  path?: string
  pluginId?: string
  onProgress?: (progress: PluginFileDropProgress | null) => void
}

interface ActiveUploadState {
  cancelled: boolean
}

type FileDragEventLike = {
  dataTransfer: DataTransfer
}

type WebKitItem = DataTransferItem & {
  webkitGetAsEntry?: () => { isDirectory?: boolean } | null
}

const SFTP_UPLOAD_CHUNK_SIZE = 256 * 1024
const readyTabs = new Set<string>()
const activeUploads = new Map<string, ActiveUploadState>()
let trackingStarted = false

function sendSftpMessage(
  tabId: string,
  payload: SftpRendererMessage,
  pluginId = PLUGIN_ID_SFTP
): Promise<unknown> {
  return window.wassh.sendPluginMessage(tabId, pluginId, payload)
}

function handlePluginActive(ev: PluginActiveStateEvent): void {
  if (ev.pluginId !== PLUGIN_ID_SFTP) {
    return
  }
  if (ev.active) {
    return
  }
  readyTabs.delete(ev.tabId)
  activeUploads.delete(ev.tabId)
}

function handlePluginMessage(ev: PluginMessageEvent): void {
  if (ev.pluginId !== PLUGIN_ID_SFTP) {
    return
  }
  const payload = ev.payload as SftpStatusPayload | null
  if (!payload || payload.type !== 'status') {
    return
  }
  if (payload.state === 'connected') {
    readyTabs.add(ev.tabId)
  } else {
    readyTabs.delete(ev.tabId)
  }
}

function startTracking(): void {
  if (trackingStarted) {
    return
  }
  trackingStarted = true
  window.wassh.onPluginActive((ev) => handlePluginActive(ev))
  window.wassh.onPluginMessage((ev) => handlePluginMessage(ev))
  window.wassh.onSessionStatus((ev) => {
    if (ev.status !== 'closed') {
      return
    }
    readyTabs.delete(ev.tabId)
    activeUploads.delete(ev.tabId)
  })
}

export function isFileDrag(e: FileDragEventLike): boolean {
  return Array.from(e.dataTransfer.types).includes('Files')
}

/** Files dropped from the OS; directories are skipped (cannot be chunk-uploaded). */
export function collectDroppedFiles(dt: DataTransfer | null): File[] {
  if (!dt) {
    return []
  }
  const items = dt.items
  if (items && items.length > 0) {
    const files: File[] = []
    for (const item of Array.from(items)) {
      if (item.kind !== 'file') {
        continue
      }
      const entry = (item as WebKitItem).webkitGetAsEntry?.()
      if (entry?.isDirectory) {
        continue
      }
      const file = item.getAsFile()
      if (file) {
        files.push(file)
      }
    }
    return files
  }
  return Array.from(dt.files)
}

export function cancelSftpFileDrop(tabId: string, pluginId = PLUGIN_ID_SFTP): void {
  const active = activeUploads.get(tabId)
  if (active) {
    active.cancelled = true
    activeUploads.delete(tabId)
  }
  void sendSftpMessage(tabId, { type: 'cancel' }, pluginId)
}

export async function uploadFilesOverSftp({
  tabId,
  files,
  path,
  pluginId = PLUGIN_ID_SFTP,
  onProgress
}: UploadFilesOptions): Promise<void> {
  startTracking()
  if (activeUploads.has(tabId)) {
    cancelSftpFileDrop(tabId, pluginId)
  }
  const active: ActiveUploadState = { cancelled: false }
  activeUploads.set(tabId, active)
  try {
    for (const file of files) {
      if (active.cancelled) {
        break
      }
      onProgress?.({
        name: file.name,
        transferredBytes: 0,
        totalBytes: file.size
      })
      try {
        await sendSftpMessage(tabId, { type: 'uploadStart', name: file.name, size: file.size, path }, pluginId)
        let offset = 0
        while (offset < file.size) {
          if (active.cancelled) {
            throw new Error('cancelled')
          }
          const end = Math.min(offset + SFTP_UPLOAD_CHUNK_SIZE, file.size)
          const buffer = await file.slice(offset, end).arrayBuffer()
          if (active.cancelled) {
            throw new Error('cancelled')
          }
          await sendSftpMessage(
            tabId,
            { type: 'uploadChunk', name: file.name, data: new Uint8Array(buffer) },
            pluginId
          )
          offset = end
          onProgress?.({
            name: file.name,
            transferredBytes: offset,
            totalBytes: file.size
          })
        }
        await sendSftpMessage(tabId, { type: 'uploadEnd', name: file.name }, pluginId)
      } catch {
        if (active.cancelled) {
          break
        }
        void sendSftpMessage(tabId, { type: 'cancel' }, pluginId)
      } finally {
        onProgress?.(null)
      }
    }
  } finally {
    if (activeUploads.get(tabId) === active) {
      activeUploads.delete(tabId)
    }
    onProgress?.(null)
  }
}

export const sftpFileDropHandler: PluginFileDropHandler = {
  isReady(tabId) {
    startTracking()
    return readyTabs.has(tabId)
  },
  onFilesDropped(tabId, files, onProgress) {
    return uploadFilesOverSftp({
      tabId,
      files,
      onProgress
    })
  }
}
