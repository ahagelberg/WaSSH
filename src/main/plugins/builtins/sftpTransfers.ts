import { once } from 'events'
import {
  createReadStream as fsCreateReadStream,
  createWriteStream as fsCreateWriteStream,
  statSync as fsStatSync,
  unlink as fsUnlink
} from 'fs'
import type { ReadStream as FsReadStream, WriteStream as FsWriteStream } from 'fs'
import { basename } from 'path'
import { BrowserWindow, dialog, type OpenDialogOptions, type SaveDialogOptions } from 'electron'
import type { ReadStream as SftpReadStream, WriteStream as SftpWriteStream } from 'ssh2'
import { ZipFile } from 'yazl'
import type { SftpTransferDonePayload, SftpTransferProgressPayload } from '../../../shared/plugins'
import type { PluginMainContext } from '../PluginHost'
import { classifySftpError, joinRemotePath, type SftpError, type SftpSession } from '../SftpSession'

interface FileTransferState {
  cancelled: boolean
  done: boolean
  remotePath: string
  localPath: string
}

export interface SftpDownloadState extends FileTransferState {
  remote: SftpReadStream | null
  local: FsWriteStream
  transferred: number
  archive?: ZipFile
}

export interface SftpUploadState extends FileTransferState {
  remote: SftpWriteStream
  local: FsReadStream
  transferred: number
}

export interface SftpChunkUploadState {
  cancelled: boolean
  done: boolean
  name: string
  remotePath: string
  total: number
  received: number
  write: SftpWriteStream | null
  queue: Promise<void>
}

export interface SftpTransferState {
  sftp: SftpSession | null
  cwd: string | null
  stopped: boolean
  download: SftpDownloadState | null
  upload: SftpUploadState | null
  chunkUpload: SftpChunkUploadState | null
}

const ARCHIVE_EMPTY_SIZE = 0
const ARCHIVE_ROOT_MTIME = 0

interface ArchiveEntry {
  remotePath: string
  archivePath: string
  size: number
  mtime: number
  mode: number
  directory: boolean
}

function focusedWindow(): BrowserWindow | null {
  const win = BrowserWindow.getFocusedWindow()
  if (win && !win.isDestroyed()) {
    return win
  }
  const all = BrowserWindow.getAllWindows()
  for (const w of all) {
    if (!w.isDestroyed()) {
      return w
    }
  }
  return null
}

function showSaveDialog(opts: SaveDialogOptions): Promise<Electron.SaveDialogReturnValue> {
  const win = focusedWindow()
  return win ? dialog.showSaveDialog(win, opts) : dialog.showSaveDialog(opts)
}

function showOpenDialog(opts: OpenDialogOptions): Promise<Electron.OpenDialogReturnValue> {
  const win = focusedWindow()
  return win ? dialog.showOpenDialog(win, opts) : dialog.showOpenDialog(opts)
}

function sendTransferDone(
  ctx: PluginMainContext,
  payload: Omit<SftpTransferDonePayload, 'type'>
): void {
  ctx.sendToRenderer({ type: 'transferDone', ...payload } satisfies SftpTransferDonePayload)
}

function sendTransferProgress(
  ctx: PluginMainContext,
  payload: Omit<SftpTransferProgressPayload, 'type'>
): void {
  ctx.sendToRenderer({ type: 'transferProgress', ...payload } satisfies SftpTransferProgressPayload)
}

function archiveRootName(path: string): string {
  return basename(path.replace(/\/+$/, '')) || 'archive'
}

async function collectArchiveEntries(
  sftp: SftpSession,
  remotePath: string,
  archivePath: string
): Promise<ArchiveEntry[]> {
  const entries: ArchiveEntry[] = [
    {
      remotePath,
      archivePath: `${archivePath}/`,
      size: ARCHIVE_EMPTY_SIZE,
      mtime: ARCHIVE_ROOT_MTIME,
      mode: ARCHIVE_EMPTY_SIZE,
      directory: true
    }
  ]
  for (const child of await sftp.list(remotePath)) {
    if (child.type === 'symlink' || child.type === 'other') {
      continue
    }
    const childArchivePath = `${archivePath}/${child.name}`
    if (child.type === 'directory') {
      entries.push(...(await collectArchiveEntries(sftp, child.path, childArchivePath)))
    } else {
      entries.push({
        remotePath: child.path,
        archivePath: childArchivePath,
        size: child.size,
        mtime: child.mtime,
        mode: child.mode,
        directory: false
      })
    }
  }
  return entries
}

export async function handleDownload(
  ctx: PluginMainContext,
  state: SftpTransferState,
  path: string
): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    sendTransferDone(ctx, {
      direction: 'download',
      remotePath: path,
      state: 'error',
      error: 'SFTP session is not connected',
      errorKind: 'connection'
    })
    return
  }

  if (state.download && !state.download.done) {
    sendTransferDone(ctx, {
      direction: 'download',
      remotePath: path,
      state: 'error',
      error: 'Another download is already running',
      errorKind: 'io'
    })
    return
  }

  const result = await showSaveDialog({
    title: 'Save downloaded file',
    defaultPath: basename(path),
    buttonLabel: 'Download'
  })
  if (result.canceled || !result.filePath) {
    sendTransferDone(ctx, { direction: 'download', remotePath: path, state: 'cancelled' })
    return
  }

  let total = 0
  try {
    total = (await sftp.stat(path)).size
  } catch {
    total = 0
  }

  const localPath = result.filePath
  const remote = sftp.createReadStream(path)
  const local = fsCreateWriteStream(localPath)
  const download: SftpDownloadState = {
    cancelled: false,
    done: false,
    remotePath: path,
    localPath,
    remote,
    local,
    transferred: 0
  }
  state.download = download

  const outcome = await new Promise<{ state: 'done' | 'error' | 'cancelled'; error?: SftpError }>(
    (resolve) => {
      let settled = false
      const settle = (
        s: 'done' | 'error' | 'cancelled',
        error?: SftpError
      ): void => {
        if (settled) {
          return
        }
        settled = true
        resolve({ state: s, error })
      }

      remote.on('error', (err: Error) => settle('error', classifySftpError(err)))
      local.on('error', (err: Error) =>
        settle(
          download.cancelled ? 'cancelled' : 'error',
          download.cancelled ? undefined : classifySftpError(err)
        )
      )
      remote.on('data', (chunk: Buffer) => {
        if (download.cancelled) {
          return
        }
        download.transferred += chunk.length
        sendTransferProgress(ctx, {
          direction: 'download',
          remotePath: path,
          transferredBytes: download.transferred,
          totalBytes: total
        })
        if (!local.write(chunk)) {
          remote.pause()
          local.once('drain', () => {
            if (!download.cancelled) {
              remote.resume()
            }
          })
        }
      })
      remote.on('end', () => {
        local.end(() => settle(download.cancelled ? 'cancelled' : 'done'))
      })
      remote.on('close', () => {
        if (download.cancelled) {
          settle('cancelled')
        }
      })
    }
  )

  download.done = true
  state.download = null
  if (outcome.state === 'error') {
    fsUnlink(localPath, () => {
      /* ignore */
    })
  }
  sendTransferDone(ctx, {
    direction: 'download',
    remotePath: path,
    state: outcome.state,
    error: outcome.error?.message,
    errorKind: outcome.error?.kind
  })
}

export async function handleDownloadZip(
  ctx: PluginMainContext,
  state: SftpTransferState,
  path: string
): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    sendTransferDone(ctx, {
      direction: 'download-zip',
      remotePath: path,
      state: 'error',
      error: 'SFTP session is not connected',
      errorKind: 'connection'
    })
    return
  }
  if (state.download && !state.download.done) {
    sendTransferDone(ctx, {
      direction: 'download-zip',
      remotePath: path,
      state: 'error',
      error: 'Another download is already running',
      errorKind: 'io'
    })
    return
  }

  const rootName = archiveRootName(path)
  const result = await showSaveDialog({
    title: 'Save folder as ZIP',
    defaultPath: `${rootName}.zip`,
    buttonLabel: 'Download',
    filters: [{ name: 'ZIP archive (*.zip)', extensions: ['zip'] }]
  })
  if (result.canceled || !result.filePath) {
    sendTransferDone(ctx, { direction: 'download-zip', remotePath: path, state: 'cancelled' })
    return
  }

  const local = fsCreateWriteStream(result.filePath)
  const archive = new ZipFile()
  const download: SftpDownloadState = {
    cancelled: false,
    done: false,
    remotePath: path,
    localPath: result.filePath,
    remote: null,
    local,
    transferred: ARCHIVE_EMPTY_SIZE,
    archive
  }
  state.download = download

  let outcome: 'done' | 'error' | 'cancelled' = 'done'
  let error: SftpError | undefined
  try {
    const entries = await collectArchiveEntries(sftp, path, rootName)
    const totalBytes = entries.reduce((total, entry) => total + entry.size, ARCHIVE_EMPTY_SIZE)
    const archiveDone = new Promise<void>((resolve, reject) => {
      archive.on('error', reject)
      local.on('error', reject)
      local.on('finish', resolve)
    })
    archive.outputStream.pipe(local)

    for (const entry of entries) {
      if (entry.directory) {
        archive.addEmptyDirectory(entry.archivePath, {
          mtime: entry.mtime > 0 ? new Date(entry.mtime) : new Date(),
          mode: entry.mode > 0 ? entry.mode : undefined
        })
      } else {
        archive.addReadStreamLazy(
          entry.archivePath,
          {
            size: entry.size,
            mtime: entry.mtime > 0 ? new Date(entry.mtime) : new Date(),
            mode: entry.mode > 0 ? entry.mode : undefined
          },
          (cb) => {
            if (download.cancelled) {
              cb(new Error('Transfer cancelled'), null as unknown as SftpReadStream)
              return
            }
            const remote = sftp.createReadStream(entry.remotePath)
            download.remote = remote
            remote.on('error', (err: Error) => {
              archive.emit('error', err)
            })
            remote.on('data', (chunk: Buffer) => {
              if (download.cancelled) {
                return
              }
              download.transferred += chunk.length
              sendTransferProgress(ctx, {
                direction: 'download-zip',
                remotePath: path,
                transferredBytes: download.transferred,
                totalBytes
              })
            })
            cb(null, remote)
          }
        )
      }
    }

    archive.end()
    await archiveDone
  } catch (err) {
    outcome = download.cancelled ? 'cancelled' : 'error'
    if (outcome === 'error') {
      error = classifySftpError(err)
    }
  }

  download.done = true
  state.download = null
  if (outcome !== 'done') {
    try {
      download.remote?.destroy()
    } catch {
      /* ignore */
    }
    try {
      download.local.destroy()
    } catch {
      /* ignore */
    }
    fsUnlink(download.localPath, () => {
      /* ignore */
    })
  }
  sendTransferDone(ctx, {
    direction: 'download-zip',
    remotePath: path,
    state: outcome,
    error: error?.message,
    errorKind: error?.kind
  })
}

async function uploadFile(
  ctx: PluginMainContext,
  state: SftpTransferState,
  localPath: string,
  targetDir: string
): Promise<boolean> {
  const sftp = state.sftp
  if (!sftp || (state.upload && !state.upload.done)) {
    return false
  }

  let size = 0
  try {
    size = fsStatSync(localPath).size
  } catch {
    size = 0
  }
  const name = basename(localPath)
  const remotePath = joinRemotePath(targetDir, name)
  const remote = sftp.createWriteStream(remotePath)
  const local = fsCreateReadStream(localPath)
  const upload: SftpUploadState = {
    cancelled: false,
    done: false,
    remotePath,
    localPath,
    remote,
    local,
    transferred: 0
  }
  state.upload = upload

  const outcome = await new Promise<{ state: 'done' | 'error' | 'cancelled'; error?: SftpError }>(
    (resolve) => {
      let settled = false
      const settle = (
        s: 'done' | 'error' | 'cancelled',
        error?: SftpError
      ): void => {
        if (settled) {
          return
        }
        settled = true
        resolve({ state: s, error })
      }

      local.on('error', (err: Error) => settle('error', classifySftpError(err)))
      remote.on('error', (err: Error) =>
        settle(upload.cancelled ? 'cancelled' : 'error', upload.cancelled ? undefined : classifySftpError(err))
      )
      local.on('data', (chunk: Buffer) => {
        if (upload.cancelled) {
          return
        }
        upload.transferred += chunk.length
        sendTransferProgress(ctx, {
          direction: 'upload',
          remotePath,
          transferredBytes: upload.transferred,
          totalBytes: size
        })
        if (!remote.write(chunk)) {
          local.pause()
          remote.once('drain', () => {
            if (!upload.cancelled) {
              local.resume()
            }
          })
        }
      })
      local.on('end', () => {
        remote.end(() => settle(upload.cancelled ? 'cancelled' : 'done'))
      })
      remote.on('close', () => {
        if (upload.cancelled) {
          settle('cancelled')
        }
      })
    }
  )

  upload.done = true
  state.upload = null
  sendTransferDone(ctx, {
    direction: 'upload',
    remotePath,
    state: outcome.state,
    error: outcome.error?.message,
    errorKind: outcome.error?.kind
  })
  return outcome.state === 'done'
}

export async function handleUploadDialog(
  ctx: PluginMainContext,
  state: SftpTransferState,
  path?: string
): Promise<number> {
  if (!state.sftp) {
    return 0
  }
  const result = await showOpenDialog({
    title: 'Upload files',
    properties: ['openFile', 'multiSelections'],
    buttonLabel: 'Upload'
  })
  if (result.canceled || result.filePaths.length === 0) {
    return 0
  }
  const targetDir = path && path.trim() !== '' ? path.trim() : state.cwd || '/'
  let uploaded = 0
  for (const localPath of result.filePaths) {
    if (state.stopped) {
      break
    }
    if (await uploadFile(ctx, state, localPath, targetDir)) {
      uploaded += 1
    }
  }
  return uploaded
}

export async function handleChunkUploadStart(
  ctx: PluginMainContext,
  state: SftpTransferState,
  payload: { name: string; size: number; path?: string }
): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    const targetDir =
      payload.path && payload.path.trim() !== '' ? payload.path.trim() : state.cwd || '/tmp'
    sendTransferDone(ctx, {
      direction: 'upload',
      remotePath: joinRemotePath(targetDir, payload.name),
      state: 'error',
      error: 'SFTP session is not connected',
      errorKind: 'connection'
    })
    return
  }
  const existing = state.chunkUpload
  if (existing && !existing.done && !existing.cancelled) {
    existing.cancelled = true
    try {
      existing.write?.destroy()
    } catch {
      /* ignore */
    }
  }
  const targetDir =
    payload.path && payload.path.trim() !== '' ? payload.path.trim() : state.cwd || '/tmp'
  const remotePath = joinRemotePath(targetDir, payload.name)
  const write = sftp.createWriteStream(remotePath, { autoClose: false })
  const chunkUpload: SftpChunkUploadState = {
    cancelled: false,
    done: false,
    name: payload.name,
    remotePath,
    total: payload.size,
    received: 0,
    write,
    queue: Promise.resolve()
  }
  write.on('error', (err: Error) => {
    if (chunkUpload.done || chunkUpload.cancelled) {
      return
    }
    chunkUpload.cancelled = true
    if (state.chunkUpload === chunkUpload) {
      state.chunkUpload = null
    }
    try {
      write.destroy()
    } catch {
      /* ignore */
    }
    const e = classifySftpError(err)
    sendTransferDone(ctx, {
      direction: 'upload',
      remotePath,
      state: 'error',
      error: e.message,
      errorKind: e.kind
    })
  })
  state.chunkUpload = chunkUpload
  sendTransferProgress(ctx, {
    direction: 'upload',
    remotePath,
    transferredBytes: 0,
    totalBytes: payload.size
  })
}

export async function handleChunkUploadChunk(
  ctx: PluginMainContext,
  state: SftpTransferState,
  data: Uint8Array
): Promise<void> {
  const chunkUpload = state.chunkUpload
  if (!chunkUpload || chunkUpload.cancelled || chunkUpload.done) {
    return
  }
  const buf = Buffer.from(data)
  const prev = chunkUpload.queue
  chunkUpload.queue = prev
    .catch(() => {
      /* continue after prior error */
    })
    .then(async () => {
      if (chunkUpload.cancelled || chunkUpload.done || !chunkUpload.write) {
        return
      }
      if (!chunkUpload.write.write(buf)) {
        await once(chunkUpload.write, 'drain')
        if (chunkUpload.cancelled || chunkUpload.done || !chunkUpload.write) {
          return
        }
      }
      chunkUpload.received += buf.length
      sendTransferProgress(ctx, {
        direction: 'upload',
        remotePath: chunkUpload.remotePath,
        transferredBytes: chunkUpload.received,
        totalBytes: chunkUpload.total
      })
    })
  await prev.catch(() => {
    /* ignore */
  })
}

export async function handleChunkUploadEnd(
  ctx: PluginMainContext,
  state: SftpTransferState
): Promise<void> {
  const chunkUpload = state.chunkUpload
  if (!chunkUpload || chunkUpload.cancelled) {
    return
  }
  const write = chunkUpload.write
  const sftp = state.sftp
  const prev = chunkUpload.queue
  chunkUpload.queue = prev
    .catch(() => {
      /* continue after prior error */
    })
    .then(async () => {
      if (!write || chunkUpload.done) {
        return
      }
      await new Promise<void>((resolve) => {
        write.end(() => resolve())
      })
      if (chunkUpload.cancelled) {
        return
      }
      // ssh2 marks WriteStream finished before the server confirms close, so
      // wait for the remote close before refreshing the directory listing.
      const handle = (write as unknown as { handle?: Buffer | null }).handle
      if (sftp && handle) {
        try {
          await sftp.close(handle)
        } catch (err) {
          if (chunkUpload.cancelled) {
            return
          }
          const e = classifySftpError(err)
          chunkUpload.cancelled = true
          if (state.chunkUpload === chunkUpload) {
            state.chunkUpload = null
          }
          sendTransferDone(ctx, {
            direction: 'upload',
            remotePath: chunkUpload.remotePath,
            state: 'error',
            error: e.message,
            errorKind: e.kind
          })
          return
        }
        ;(write as unknown as { handle?: Buffer | null }).handle = null
      }
      try {
        write.destroy()
      } catch {
        /* ignore */
      }
      if (chunkUpload.cancelled) {
        return
      }
      chunkUpload.done = true
      sendTransferDone(ctx, {
        direction: 'upload',
        remotePath: chunkUpload.remotePath,
        state: 'done'
      })
    })
  await chunkUpload.queue.catch(() => {
    /* ignore */
  })
  state.chunkUpload = null
}

function cancelDownload(state: SftpTransferState): void {
  const download = state.download
  if (!download) {
    return
  }
  download.cancelled = true
  try {
    download.remote?.destroy()
  } catch {
    /* ignore */
  }
  try {
    download.local.destroy()
  } catch {
    /* ignore */
  }
}

function cancelUpload(state: SftpTransferState): void {
  const upload = state.upload
  if (!upload) {
    return
  }
  upload.cancelled = true
  try {
    upload.local.destroy()
  } catch {
    /* ignore */
  }
  try {
    upload.remote.destroy()
  } catch {
    /* ignore */
  }
}

export function cancelTransfers(state: SftpTransferState): void {
  cancelDownload(state)
  cancelUpload(state)
  const chunkUpload = state.chunkUpload
  if (chunkUpload) {
    chunkUpload.cancelled = true
    try {
      chunkUpload.write?.destroy()
    } catch {
      /* ignore */
    }
  }
}
