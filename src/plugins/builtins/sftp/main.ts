import type { PluginMainContext, PluginMainModule, SessionStatus } from '@plugin-api/main'
import { classifySftpError } from '@plugin-api/main'
import { handleViewFile } from './fileView'
import {
  instanceKey,
  isRendererMessage,
  resolveCwd,
  sendOpResult,
  sendStatus,
  sessionStates,
  stateFor,
  type SessionState,
  type SftpOpMessage
} from './helpers'
import type { SftpListPayload, SftpRendererMessage } from './protocol'
import { clearClipboardFor, copyFilesToClipboard, pasteFilesFromClipboard, sendClipboardState } from './clipboard'
import {
  cancelTransfers,
  handleChunkUploadChunk,
  handleChunkUploadEnd,
  handleChunkUploadStart,
  handleDownload,
  handleDownloadZip,
  handleUploadDialog,
  handleUploadUris,
} from './transfers'

/** Session status that means the SSH transport has come back up. */
const SESSION_STATUS_CONNECTED: SessionStatus = 'connected'

async function handleList(
  ctx: PluginMainContext,
  state: SessionState,
  path?: string
): Promise<void> {
  const target = path && path.trim() !== '' ? path.trim() : state.cwd || '/'
  const sftp = state.sftp
  if (!sftp) {
    ctx.sendToRenderer({
      type: 'listResult',
      path: target,
      cwd: state.cwd || '/',
      entries: [],
      error: 'File session is not connected',
      errorKind: 'connection'
    } satisfies SftpListPayload)
    return
  }

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

function opSubject(payload: SftpOpMessage): string {
  return payload.type === 'rename' ? payload.newPath : payload.path
}

async function runOp(
  ctx: PluginMainContext,
  state: SessionState,
  payload: SftpOpMessage
): Promise<void> {
  const op = payload.type
  const subject = opSubject(payload)
  const sftp = state.sftp
  if (!sftp) {
    sendOpResult(ctx, {
      op,
      path: subject,
      ok: false,
      error: 'File session is not connected',
      errorKind: 'connection'
    })
    return
  }

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
    }
    sendOpResult(ctx, { op, path: subject, ok: true })
  } catch (err) {
    const e = classifySftpError(err)
    sendOpResult(ctx, {
      op,
      path: subject,
      ok: false,
      error: e.message,
      errorKind: e.kind
    })
  }
}

async function handleDelete(
  ctx: PluginMainContext,
  state: SessionState,
  paths: string[]
): Promise<void> {
  const failures: Array<{ path: string; error: string }> = []
  let deleted = 0
  for (const path of paths) {
    try {
      if (!state.sftp) {
        throw new Error('File session is not connected')
      }
      await state.sftp.delete(path)
      deleted += 1
    } catch (err) {
      failures.push({ path, error: classifySftpError(err).message })
    }
  }
  ctx.sendToRenderer({
    type: 'deleteResult',
    total: paths.length,
    deleted,
    failures
  })
}

async function handleResetCwd(ctx: PluginMainContext, state: SessionState): Promise<void> {
  if (!state.sftp) {
    sendStatus(ctx, {
      state: 'error',
      reason: 'File session is not connected',
      errorKind: 'connection'
    })
    return
  }
  try {
    state.cwd = await resolveCwd(ctx, state)
    state.error = null
    state.errorKind = null
    sendStatus(ctx, { state: 'connected', cwd: state.cwd })
  } catch (err) {
    const e = classifySftpError(err)
    state.error = e.message
    state.errorKind = e.kind
    sendStatus(ctx, { state: 'error', reason: e.message, errorKind: e.kind })
  }
}

function handleGetStatus(ctx: PluginMainContext, state: SessionState): void {
  sendClipboardState(ctx)
  if (state.sftp) {
    sendStatus(ctx, { state: 'connected', cwd: state.cwd || '/' })
    return
  }
  sendStatus(ctx, {
    state: 'error',
    reason: state.error || 'File session is not connected',
    errorKind: state.errorKind || 'other'
  })
}

function teardown(state: SessionState): void {
  state.stopped = true
  cancelTransfers(state)
  try {
    state.sftp?.end()
  } catch {
    /* ignore */
  }
  state.sftp = null
}

/** Open the file session for the active transport. */
async function openSession(ctx: PluginMainContext, state: SessionState): Promise<void> {
  if (state.opening) {
    return
  }
  state.opening = true
  sendStatus(ctx, { state: 'connecting' })
  try {
    const sftp = ctx.isLocalSession()
      ? await ctx.openLocalFileSystem()
      : await ctx.openSftp()
    state.sftp = sftp
    try {
      state.home = await sftp.realpath('~')
    } catch {
      state.home = ''
    }
    state.cwd = await resolveCwd(ctx, state)
    state.error = null
    state.errorKind = null
    sendStatus(ctx, { state: 'connected', cwd: state.cwd })
  } catch (err) {
    const e = classifySftpError(err)
    state.error = e.message
    state.errorKind = e.kind
    sendStatus(ctx, { state: 'error', reason: e.message, errorKind: e.kind })
  } finally {
    state.opening = false
  }
}

/**
 * A rebuilt transport takes the SFTP channel down with it. ssh2 drops requests
 * against a closed channel without reporting anything, so the dead session has
 * to be discarded here or every later operation would hang forever. In-flight
 * transfers are abandoned with it: their entries would otherwise block the next
 * transfer behind a permanent "already running".
 */
function dropSession(state: SessionState): void {
  clearClipboardFor(state)
  cancelTransfers(state)
  state.download = null
  state.upload = null
  try {
    state.sftp?.end()
  } catch {
    /* ignore */
  }
  state.sftp = null
  state.cwd = null
  state.home = ''
}

async function handleMessage(
  ctx: PluginMainContext,
  state: SessionState,
  payload: SftpRendererMessage
): Promise<number | undefined> {
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
      await runOp(ctx, state, payload)
      break
    case 'delete':
      await handleDelete(ctx, state, payload.paths)
      break
    case 'download':
      await handleDownload(ctx, state, payload.path)
      break
    case 'downloadZip':
      await handleDownloadZip(ctx, state, payload.path)
      break
    case 'copy':
      await copyFilesToClipboard(ctx, state, payload.paths)
      break
    case 'paste':
      await pasteFilesFromClipboard(ctx, state, payload.path)
      break
    case 'viewFile':
      await handleViewFile(ctx, state.sftp, payload.path)
      break
    case 'uploadDialog':
      return handleUploadDialog(ctx, state, payload.path)
    case 'uploadUris':
      return handleUploadUris(ctx, state, payload.uris, payload.path)
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
      cancelTransfers(state)
      break
    case 'resetCwd':
      await handleResetCwd(ctx, state)
      break
  }
}

export const sftpMain: PluginMainModule = {
  async onActivate(ctx) {
    const state: SessionState = {
      ctx,
      sftp: null,
      cwd: null,
      home: '',
      stopped: false,
      opening: false,
      error: null,
      errorKind: null,
      download: null,
      upload: null,
      chunkUpload: null
    }
    sessionStates.set(instanceKey(ctx), state)

    ctx.onDeactivateCleanup(() => {
      clearClipboardFor(state)
      teardown(state)
      sessionStates.delete(instanceKey(ctx))
    })

    if (!ctx.isSshSession() && !ctx.isLocalSession()) {
      state.error = 'Files are available for SSH and Local sessions'
      state.errorKind = 'not_ssh'
      sendStatus(ctx, {
        state: 'error',
        errorKind: 'not_ssh',
        reason: 'Files are available for SSH and Local sessions'
      })
      return
    }

    await openSession(ctx, state)
  },

  async onDeactivate(ctx) {
    const state = stateFor(ctx)
    if (state) {
      clearClipboardFor(state)
      teardown(state)
      sessionStates.delete(instanceKey(ctx))
    }
  },

  async onSessionStatus(ctx, event) {
    const state = stateFor(ctx)
    if (!state || state.stopped) {
      return
    }
    if (event.status === SESSION_STATUS_CONNECTED) {
      if (!state.sftp) {
        await openSession(ctx, state)
      }
      return
    }
    dropSession(state)
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
