import { once } from 'events'
import {
  createReadStream as fsCreateReadStream,
  createWriteStream as fsCreateWriteStream,
  statSync as fsStatSync,
  unlink as fsUnlink
} from 'fs'
import { basename } from 'path'
import { BrowserWindow, dialog, type OpenDialogOptions, type SaveDialogOptions } from 'electron'
import type {
  SftpErrorKind,
  SftpListPayload,
  SftpOpResultPayload,
  SftpRendererMessage,
  SftpStatusPayload,
  SftpTransferDonePayload,
  SftpTransferProgressPayload
} from '../../../shared/plugins'
import type { PluginMainContext, PluginMainModule } from '../PluginHost'
import {
  classifySftpError,
  joinRemotePath,
  type SftpError,
  type SftpSession
} from '../SftpSession'
import type { ReadStream as SftpReadStream, WriteStream as SftpWriteStream } from 'ssh2'
import type { ReadStream as FsReadStream, WriteStream as FsWriteStream } from 'fs'

/** Transfer running through a remote SSH read/write stream + local fs stream. */
interface FileTransferState {
  cancelled: boolean
  done: boolean
  remotePath: string
  localPath: string
}

interface DownloadState extends FileTransferState {
  remote: SftpReadStream
  local: FsWriteStream
  transferred: number
}

interface UploadState extends FileTransferState {
  remote: SftpWriteStream
  local: FsReadStream
  transferred: number
}

/** Drop-upload fed by renderer in chunks (uploadStart / uploadChunk / uploadEnd). */
interface ChunkUploadState {
  cancelled: boolean
  done: boolean
  name: string
  remotePath: string
  total: number
  received: number
  write: SftpWriteStream | null
  /** Serializes chunk writes + finalization so chunks never interleave. */
  queue: Promise<void>
}

interface SessionState {
  sftp: SftpSession | null
  /** Remote upload target directory (pwd, Downloads, home, tmp). */
  cwd: string | null
  home: string
  stopped: boolean
  /** Last activation/reset error, so a late-mounting view can replay status. */
  error: string | null
  errorKind: SftpErrorKind | null
  download: DownloadState | null
  upload: UploadState | null
  chunkUpload: ChunkUploadState | null
}

const sessionStates = new Map<string, SessionState>()

function instanceKey(ctx: PluginMainContext): string {
  return `${ctx.tabId}::${ctx.pluginId}`
}

function stateFor(ctx: PluginMainContext): SessionState | undefined {
  return sessionStates.get(instanceKey(ctx))
}

const SFTP_MESSAGE_TYPES = new Set<string>([
  'getStatus',
  'list',
  'mkdir',
  'rename',
  'chmod',
  'delete',
  'download',
  'uploadDialog',
  'uploadStart',
  'uploadChunk',
  'uploadEnd',
  'cancel',
  'resetCwd'
])

function isRendererMessage(payload: unknown): payload is SftpRendererMessage {
  if (!payload || typeof payload !== 'object') {
    return false
  }
  const type = (payload as { type?: unknown }).type
  return typeof type === 'string' && SFTP_MESSAGE_TYPES.has(type)
}

function focusedWindow(): BrowserWindow | null {
  return BrowserWindow.getFocusedWindow()
}

/**
 * Resolve the upload target directory.
 * 1. Live SSH session cwd (pwd) when available.
 * 2. Remote ~/Downloads (then ~/Download) when it exists.
 * 3. Remote home directory.
 */
async function resolveCwd(ctx: PluginMainContext, state: SessionState): Promise<string> {
  try {
    const pwd = (await ctx.execCapture('pwd')).trim()
    if (pwd && pwd.startsWith('/')) {
      return pwd
    }
  } catch {
    /* fall through to home-based resolution */
  }

  const sftp = state.sftp
  if (!sftp) {
    return state.home || '/tmp'
  }
  let home = state.home
  if (!home) {
    try {
      home = await sftp.realpath('~')
    } catch {
      home = ''
    }
    state.home = home
  }
  if (!home) {
    return '/tmp'
  }
  for (const name of ['Downloads', 'Download']) {
    const candidate = joinRemotePath(home, name)
    const st = await sftp.statSafe(candidate)
    if (st?.isDirectory()) {
      return candidate
    }
  }
  return home
}

async function handleList(
  ctx: PluginMainContext,
  state: SessionState,
  path?: string
): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    return
  }
  const target = path && path.trim() !== '' ? path.trim() : state.cwd || '/'
  try {
    const entries = await sftp.list(target)
    ctx.sendToRenderer({
      type: 'listResult',
      path: target,
      cwd: state.cwd || '/',
      entries
    } satisfies SftpListPayload)
  } catch (err) {
    const e = classifySftpError(err)
    ctx.sendToRenderer({
      type: 'listResult',
      path: target,
      cwd: state.cwd || '/',
      entries: [],
      error: e.message,
      errorKind: e.kind
    } satisfies SftpListPayload)
  }
}

type SftpOpMessage = Extract<SftpRendererMessage, { type: 'mkdir' | 'rename' | 'chmod' | 'delete' }>

async function runOp(
  ctx: PluginMainContext,
  state: SessionState,
  payload: SftpOpMessage
): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    return
  }
  const op = payload.type
  const subject = op === 'rename' ? payload.newPath : payload.path
  try {
    switch (op) {
      case 'mkdir':
        await sftp.mkdir(payload.path)
        break
      case 'rename':
        await sftp.rename(payload.oldPath, payload.newPath)
        break
      case 'chmod':
        await sftp.chmod(payload.path, payload.mode)
        break
      case 'delete':
        await sftp.delete(payload.path)
        break
    }
    ctx.sendToRenderer({
      type: 'opResult',
      op,
      path: subject,
      ok: true
    } satisfies SftpOpResultPayload)
  } catch (err) {
    const e = classifySftpError(err)
    ctx.sendToRenderer({
      type: 'opResult',
      op,
      path: subject,
      ok: false,
      error: e.message,
      errorKind: e.kind
    } satisfies SftpOpResultPayload)
  }
}

async function handleDownload(ctx: PluginMainContext, state: SessionState, path: string): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    return
  }
  if (state.download && !state.download.done) {
    return
  }
  const opts: SaveDialogOptions = {
    title: 'Save downloaded file',
    defaultPath: basename(path),
    buttonLabel: 'Download'
  }
  const win = focusedWindow()
  const result = win ? await dialog.showSaveDialog(win, opts) : await dialog.showSaveDialog(opts)
  if (result.canceled || !result.filePath) {
    ctx.sendToRenderer({
      type: 'transferDone',
      direction: 'download',
      remotePath: path,
      state: 'cancelled'
    } satisfies SftpTransferDonePayload)
    return
  }

  let total = 0
  try {
    const stats = await sftp.stat(path)
    total = stats.size
  } catch {
    total = 0
  }

  const localPath = result.filePath
  const remote = sftp.createReadStream(path)
  const local = fsCreateWriteStream(localPath)
  const dl: DownloadState = {
    cancelled: false,
    done: false,
    remotePath: path,
    localPath,
    remote,
    local,
    transferred: 0
  }
  state.download = dl

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
        settle(dl.cancelled ? 'cancelled' : 'error', dl.cancelled ? undefined : classifySftpError(err))
      )
      remote.on('data', (chunk: Buffer) => {
        if (dl.cancelled) {
          return
        }
        dl.transferred += chunk.length
        ctx.sendToRenderer({
          type: 'transferProgress',
          direction: 'download',
          remotePath: path,
          transferredBytes: dl.transferred,
          totalBytes: total
        } satisfies SftpTransferProgressPayload)
        if (!local.write(chunk)) {
          remote.pause()
          local.once('drain', () => {
            if (!dl.cancelled) {
              remote.resume()
            }
          })
        }
      })
      remote.on('end', () => {
        local.end(() => settle(dl.cancelled ? 'cancelled' : 'done'))
      })
      remote.on('close', () => {
        if (dl.cancelled) {
          settle('cancelled')
        }
      })
    }
  )

  dl.done = true
  state.download = null
  if (outcome.state === 'error') {
    try {
      fsUnlink(localPath, () => {
        /* ignore */
      })
    } catch {
      /* ignore */
    }
  }
  ctx.sendToRenderer({
    type: 'transferDone',
    direction: 'download',
    remotePath: path,
    state: outcome.state,
    error: outcome.error?.message,
    errorKind: outcome.error?.kind
  } satisfies SftpTransferDonePayload)
}

async function uploadFile(
  ctx: PluginMainContext,
  state: SessionState,
  localPath: string,
  targetDir: string
): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    return
  }
  if (state.upload && !state.upload.done) {
    return
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
  const up: UploadState = {
    cancelled: false,
    done: false,
    remotePath,
    localPath,
    remote,
    local,
    transferred: 0
  }
  state.upload = up

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
        settle(up.cancelled ? 'cancelled' : 'error', up.cancelled ? undefined : classifySftpError(err))
      )
      local.on('data', (chunk: Buffer) => {
        if (up.cancelled) {
          return
        }
        up.transferred += chunk.length
        ctx.sendToRenderer({
          type: 'transferProgress',
          direction: 'upload',
          remotePath,
          transferredBytes: up.transferred,
          totalBytes: size
        } satisfies SftpTransferProgressPayload)
        if (!remote.write(chunk)) {
          local.pause()
          remote.once('drain', () => {
            if (!up.cancelled) {
              local.resume()
            }
          })
        }
      })
      local.on('end', () => {
        remote.end(() => settle(up.cancelled ? 'cancelled' : 'done'))
      })
      remote.on('close', () => {
        if (up.cancelled) {
          settle('cancelled')
        }
      })
    }
  )

  up.done = true
  state.upload = null
  ctx.sendToRenderer({
    type: 'transferDone',
    direction: 'upload',
    remotePath,
    state: outcome.state,
    error: outcome.error?.message,
    errorKind: outcome.error?.kind
  } satisfies SftpTransferDonePayload)
}

async function handleUploadDialog(
  ctx: PluginMainContext,
  state: SessionState,
  path?: string
): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    return
  }
  const opts: OpenDialogOptions = {
    title: 'Upload files',
    properties: ['openFile', 'multiSelections'],
    buttonLabel: 'Upload'
  }
  const win = focusedWindow()
  const result = win ? await dialog.showOpenDialog(win, opts) : await dialog.showOpenDialog(opts)
  if (result.canceled || result.filePaths.length === 0) {
    return
  }
  const targetDir = path && path.trim() !== '' ? path.trim() : state.cwd || '/'
  for (const localPath of result.filePaths) {
    if (state.stopped) {
      break
    }
    await uploadFile(ctx, state, localPath, targetDir)
  }
}

async function handleChunkUploadStart(
  ctx: PluginMainContext,
  state: SessionState,
  payload: { name: string; size: number }
): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    return
  }
  const existing = state.chunkUpload
  if (existing && !existing.done && !existing.cancelled) {
    return
  }
  const remotePath = joinRemotePath(state.cwd || '/tmp', payload.name)
  const cu: ChunkUploadState = {
    cancelled: false,
    done: false,
    name: payload.name,
    remotePath,
    total: payload.size,
    received: 0,
    write: sftp.createWriteStream(remotePath),
    queue: Promise.resolve()
  }
  state.chunkUpload = cu
  ctx.sendToRenderer({
    type: 'transferProgress',
    direction: 'upload',
    remotePath,
    transferredBytes: 0,
    totalBytes: payload.size
  } satisfies SftpTransferProgressPayload)
}

async function handleChunkUploadChunk(
  ctx: PluginMainContext,
  state: SessionState,
  data: Uint8Array
): Promise<void> {
  const cu = state.chunkUpload
  if (!cu || cu.cancelled || cu.done) {
    return
  }
  const buf = Buffer.from(data)
  const prev = cu.queue
  cu.queue = prev
    .catch(() => {
      /* continue after prior error */
    })
    .then(async () => {
      if (cu.cancelled || cu.done || !cu.write) {
        return
      }
      if (!cu.write.write(buf)) {
        await once(cu.write, 'drain')
      }
      cu.received += buf.length
      ctx.sendToRenderer({
        type: 'transferProgress',
        direction: 'upload',
        remotePath: cu.remotePath,
        transferredBytes: cu.received,
        totalBytes: cu.total
      } satisfies SftpTransferProgressPayload)
    })
  await prev.catch(() => {
    /* ignore */
  })
}

async function handleChunkUploadEnd(ctx: PluginMainContext, state: SessionState): Promise<void> {
  const cu = state.chunkUpload
  if (!cu || cu.cancelled) {
    return
  }
  const write = cu.write
  const prev = cu.queue
  cu.queue = prev
    .catch(() => {
      /* continue after prior error */
    })
    .then(async () => {
      if (!write || cu.done) {
        return
      }
      await new Promise<void>((resolve) => {
        write.end(() => resolve())
      })
      if (cu.cancelled) {
        return
      }
      cu.done = true
      ctx.sendToRenderer({
        type: 'transferDone',
        direction: 'upload',
        remotePath: cu.remotePath,
        state: 'done'
      } satisfies SftpTransferDonePayload)
    })
  await cu.queue.catch(() => {
    /* ignore */
  })
  state.chunkUpload = null
}

function cancelDownload(state: SessionState): void {
  const dl = state.download
  if (!dl) {
    return
  }
  dl.cancelled = true
  try {
    dl.remote.destroy()
  } catch {
    /* ignore */
  }
  try {
    dl.local.destroy()
  } catch {
    /* ignore */
  }
}

function cancelUpload(state: SessionState): void {
  const up = state.upload
  if (!up) {
    return
  }
  up.cancelled = true
  try {
    up.local.destroy()
  } catch {
    /* ignore */
  }
  try {
    up.remote.destroy()
  } catch {
    /* ignore */
  }
}

function handleCancel(state: SessionState): void {
  cancelDownload(state)
  cancelUpload(state)
  const cu = state.chunkUpload
  if (cu) {
    cu.cancelled = true
    try {
      cu.write?.destroy()
    } catch {
      /* ignore */
    }
  }
}

async function handleResetCwd(ctx: PluginMainContext, state: SessionState): Promise<void> {
  const sftp = state.sftp
  if (!sftp) {
    return
  }
  try {
    state.cwd = await resolveCwd(ctx, state)
    state.error = null
    state.errorKind = null
    ctx.sendToRenderer({
      type: 'status',
      state: 'connected',
      cwd: state.cwd
    } satisfies SftpStatusPayload)
  } catch (err) {
    const e = classifySftpError(err)
    state.error = e.message
    state.errorKind = e.kind
    ctx.sendToRenderer({
      type: 'status',
      state: 'error',
      reason: e.message,
      errorKind: e.kind
    } satisfies SftpStatusPayload)
  }
}

/** Replay the latest connection status for a late-mounting view. */
function handleGetStatus(ctx: PluginMainContext, state: SessionState): void {
  if (state.sftp) {
    ctx.sendToRenderer({
      type: 'status',
      state: 'connected',
      cwd: state.cwd || '/'
    } satisfies SftpStatusPayload)
    return
  }
  ctx.sendToRenderer({
    type: 'status',
    state: 'error',
    reason: state.error || 'SFTP session is not connected',
    errorKind: state.errorKind || 'other'
  } satisfies SftpStatusPayload)
}

function teardown(state: SessionState): void {
  state.stopped = true
  cancelDownload(state)
  cancelUpload(state)
  const cu = state.chunkUpload
  if (cu) {
    cu.cancelled = true
    try {
      cu.write?.destroy()
    } catch {
      /* ignore */
    }
  }
  try {
    state.sftp?.end()
  } catch {
    /* ignore */
  }
  state.sftp = null
}

async function handleMessage(
  ctx: PluginMainContext,
  state: SessionState,
  payload: SftpRendererMessage
): Promise<void> {
  switch (payload.type) {
    case 'getStatus':
      handleGetStatus(ctx, state)
      break
    case 'list':
      await handleList(ctx, state, payload.path)
      break
    case 'mkdir':
    case 'rename':
    case 'chmod':
    case 'delete':
      await runOp(ctx, state, payload)
      break
    case 'download':
      await handleDownload(ctx, state, payload.path)
      break
    case 'uploadDialog':
      await handleUploadDialog(ctx, state, payload.path)
      break
    case 'uploadStart':
      await handleChunkUploadStart(ctx, state, payload)
      break
    case 'uploadChunk':
      await handleChunkUploadChunk(ctx, state, payload.data)
      break
    case 'uploadEnd':
      await handleChunkUploadEnd(ctx, state)
      break
    case 'cancel':
      handleCancel(state)
      break
    case 'resetCwd':
      await handleResetCwd(ctx, state)
      break
  }
}

export const sftpMain: PluginMainModule = {
  async onActivate(ctx) {
    const state: SessionState = {
      sftp: null,
      cwd: null,
      home: '',
      stopped: false,
      error: null,
      errorKind: null,
      download: null,
      upload: null,
      chunkUpload: null
    }
    sessionStates.set(instanceKey(ctx), state)

    ctx.onDeactivateCleanup(() => {
      teardown(state)
      sessionStates.delete(instanceKey(ctx))
    })

    if (!ctx.isSshSession()) {
      state.error = 'SFTP requires an SSH session'
      state.errorKind = 'not_ssh'
      ctx.sendToRenderer({
        type: 'status',
        state: 'error',
        errorKind: 'not_ssh',
        reason: 'SFTP requires an SSH session'
      } satisfies SftpStatusPayload)
      return
    }

    ctx.sendToRenderer({
      type: 'status',
      state: 'connecting'
    } satisfies SftpStatusPayload)

    try {
      const sftp = await ctx.openSftp()
      state.sftp = sftp
      try {
        state.home = await sftp.realpath('~')
      } catch {
        state.home = ''
      }
      state.cwd = await resolveCwd(ctx, state)
      state.error = null
      state.errorKind = null
      ctx.sendToRenderer({
        type: 'status',
        state: 'connected',
        cwd: state.cwd
      } satisfies SftpStatusPayload)
    } catch (err) {
      const e = classifySftpError(err)
      state.error = e.message
      state.errorKind = e.kind
      ctx.sendToRenderer({
        type: 'status',
        state: 'error',
        reason: e.message,
        errorKind: e.kind
      } satisfies SftpStatusPayload)
    }
  },

  async onDeactivate(ctx) {
    const state = stateFor(ctx)
    if (state) {
      teardown(state)
      sessionStates.delete(instanceKey(ctx))
    }
  },

  onMessage(ctx, payload) {
    if (!isRendererMessage(payload)) {
      return
    }
    const state = stateFor(ctx)
    if (!state || state.stopped) {
      return
    }
    return handleMessage(ctx, state, payload)
  }
}
