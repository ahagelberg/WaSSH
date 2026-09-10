import type { PluginMainContext, PluginMainModule } from '../../../main/plugins/api'
import { classifySftpError } from '../../../main/plugins/api'
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
import {
  cancelTransfers,
  handleChunkUploadChunk,
  handleChunkUploadEnd,
  handleChunkUploadStart,
  handleDownload,
  handleDownloadZip,
  handleUploadDialog,
} from './transfers'

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
      error: 'SFTP session is not connected',
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
      error: 'SFTP session is not connected',
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
      case 'delete':
        await sftp.delete(payload.path)
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

async function handleResetCwd(ctx: PluginMainContext, state: SessionState): Promise<void> {
  if (!state.sftp) {
    sendStatus(ctx, {
      state: 'error',
      reason: 'SFTP session is not connected',
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
  if (state.sftp) {
    sendStatus(ctx, { state: 'connected', cwd: state.cwd || '/' })
    return
  }
  sendStatus(ctx, {
    state: 'error',
    reason: state.error || 'SFTP session is not connected',
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
    case 'delete':
      await runOp(ctx, state, payload)
      break
    case 'download':
      await handleDownload(ctx, state, payload.path)
      break
    case 'downloadZip':
      await handleDownloadZip(ctx, state, payload.path)
      break
    case 'viewFile':
      await handleViewFile(ctx, state.sftp, payload.path)
      break
    case 'uploadDialog':
      return handleUploadDialog(ctx, state, payload.path)
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
      sendStatus(ctx, {
        state: 'error',
        errorKind: 'not_ssh',
        reason: 'SFTP requires an SSH session'
      })
      return
    }

    sendStatus(ctx, { state: 'connecting' })

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
      sendStatus(ctx, { state: 'connected', cwd: state.cwd })
    } catch (err) {
      const e = classifySftpError(err)
      state.error = e.message
      state.errorKind = e.kind
      sendStatus(ctx, { state: 'error', reason: e.message, errorKind: e.kind })
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
