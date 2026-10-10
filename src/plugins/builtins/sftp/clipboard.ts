import { isAbsolute, posix, relative, resolve, sep } from 'node:path'
import { Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import type { PluginMainContext, PluginFileSession } from '@plugin-api/main'
import { classifySftpError, joinRemotePath } from '@plugin-api/main'
import { sessionStates } from './helpers'
import type { SessionState } from './helpers'
import { availableCopyPath } from './filePaths'
import type {
  SftpClipboardResultPayload,
  SftpClipboardStatePayload,
  SftpTransferDonePayload,
  SftpTransferProgressPayload
} from './protocol'

interface FileClipboard {
  source: SessionState
  paths: string[]
  names: string[]
}

let fileClipboard: FileClipboard | null = null
let pasteInProgress = false

function clipboardState(): Omit<SftpClipboardStatePayload, 'type'> {
  const available = Boolean(fileClipboard?.source.sftp && !fileClipboard.source.stopped)
  return {
    available,
    count: available ? fileClipboard?.paths.length ?? 0 : 0,
    names: available ? fileClipboard?.names ?? [] : []
  }
}

function broadcastClipboardState(): void {
  const payload = { type: 'clipboardState', ...clipboardState() } satisfies SftpClipboardStatePayload
  for (const state of sessionStates.values()) {
    state.ctx.sendToRenderer(payload)
  }
}

export function sendClipboardState(ctx: PluginMainContext): void {
  ctx.sendToRenderer({ type: 'clipboardState', ...clipboardState() } satisfies SftpClipboardStatePayload)
}

function sendClipboardResult(
  ctx: PluginMainContext,
  action: 'copy' | 'paste',
  ok: boolean,
  message: string
): void {
  ctx.sendToRenderer({ type: 'clipboardResult', action, ok, message } satisfies SftpClipboardResultPayload)
}

export async function copyFilesToClipboard(
  ctx: PluginMainContext,
  state: SessionState,
  paths: string[]
): Promise<void> {
  const source = state.sftp
  const uniquePaths = [...new Set(paths.filter((path) => typeof path === 'string' && path.length > 0))]
  if (!source || uniquePaths.length === 0) {
    sendClipboardResult(ctx, 'copy', false, source ? 'Select files to copy' : 'File session is not connected')
    return
  }

  try {
    const names: string[] = []
    for (const path of uniquePaths) {
      await source.lstat(path)
      names.push(posix.basename(path.replace(/\\/g, '/')) || path)
    }
    fileClipboard = { source: state, paths: uniquePaths, names }
    if (ctx.isLocalSession()) {
      await ctx.writeClipboardFileUris(uniquePaths).catch(() => {
        /* Keep in-app copy working when the OS rejects file URI clipboard data. */
      })
    }
    broadcastClipboardState()
    sendClipboardResult(ctx, 'copy', true, `Copied ${names.length} item${names.length === 1 ? '' : 's'}`)
  } catch (err) {
    sendClipboardResult(ctx, 'copy', false, classifySftpError(err).message)
  }
}

function isSameFilesystem(source: SessionState, target: SessionState): boolean {
  if (source === target || (source.ctx.isLocalSession() && target.ctx.isLocalSession())) {
    return true
  }
  return source.ctx.isSshSession() && target.ctx.isSshSession() &&
    source.ctx.getSessionScopeId() === target.ctx.getSessionScopeId()
}

function isWithin(parent: string, candidate: string, local: boolean): boolean {
  const relativePath = local
    ? relative(resolve(parent), resolve(candidate))
    : posix.relative(posix.normalize(parent), posix.normalize(candidate))
  if (!relativePath) {
    return true
  }
  return relativePath !== '..' &&
    !relativePath.startsWith(`..${local ? sep : '/'}`) &&
    !(local ? isAbsolute(relativePath) : posix.isAbsolute(relativePath))
}

async function ensureNotCopyingIntoSelf(
  sourceState: SessionState,
  targetState: SessionState,
  source: PluginFileSession,
  target: PluginFileSession,
  sourcePath: string,
  targetDirectory: string
): Promise<void> {
  if (!isSameFilesystem(sourceState, targetState) || !(await source.lstat(sourcePath)).isDirectory()) {
    return
  }
  const sourcePathReal = await source.realpath(sourcePath)
  const targetPathReal = await target.realpath(targetDirectory)
  if (isWithin(sourcePathReal, targetPathReal, sourceState.ctx.isLocalSession())) {
    throw new Error('Cannot paste a folder into itself or one of its subfolders')
  }
}

function sendProgress(ctx: PluginMainContext, payload: Omit<SftpTransferProgressPayload, 'type'>): void {
  ctx.sendToRenderer({ type: 'transferProgress', ...payload } satisfies SftpTransferProgressPayload)
}

function sendDone(ctx: PluginMainContext, payload: Omit<SftpTransferDonePayload, 'type'>): void {
  ctx.sendToRenderer({ type: 'transferDone', ...payload } satisfies SftpTransferDonePayload)
}

async function copyTree(
  ctx: PluginMainContext,
  source: PluginFileSession,
  target: PluginFileSession,
  sourcePath: string,
  targetPath: string
): Promise<void> {
  const stats = await source.lstat(sourcePath)
  if (stats.isDirectory()) {
    await target.mkdir(targetPath)
    for (const entry of await source.list(sourcePath)) {
      await copyTree(ctx, source, target, entry.path, joinRemotePath(targetPath, entry.name))
    }
    return
  }

  const totalBytes = (await source.stat(sourcePath)).size
  let transferredBytes = 0
  const counter = new Transform({
    transform(chunk: Buffer, _encoding, callback) {
      transferredBytes += chunk.length
      sendProgress(ctx, {
        direction: 'copy',
        remotePath: targetPath,
        transferredBytes,
        totalBytes
      })
      callback(null, chunk)
    }
  })
  try {
    await pipeline(source.createReadStream(sourcePath), counter, target.createWriteStream(targetPath))
    sendDone(ctx, { direction: 'copy', remotePath: targetPath, state: 'done' })
  } catch (err) {
    const error = classifySftpError(err)
    sendDone(ctx, {
      direction: 'copy',
      remotePath: targetPath,
      state: 'error',
      error: error.message,
      errorKind: error.kind
    })
    throw err
  }
}

export async function pasteFilesFromClipboard(
  ctx: PluginMainContext,
  targetState: SessionState,
  path?: string
): Promise<void> {
  const clipboard = fileClipboard
  const target = targetState.sftp
  const source = clipboard?.source.sftp
  if (!clipboard || !source || clipboard.source.stopped) {
    sendClipboardResult(ctx, 'paste', false, 'The file clipboard is empty or its source session closed')
    return
  }
  if (!target) {
    sendClipboardResult(ctx, 'paste', false, 'Destination file session is not connected')
    return
  }
  if (pasteInProgress) {
    sendClipboardResult(ctx, 'paste', false, 'Another paste operation is already running')
    return
  }

  pasteInProgress = true
  const targetDirectory = path?.trim() || targetState.cwd || '/'
  let copied = 0
  const pastedPaths: string[] = []
  try {
    for (let index = 0; index < clipboard.paths.length; index += 1) {
      const sourcePath = clipboard.paths[index]
      const name = clipboard.names[index]
      await ensureNotCopyingIntoSelf(
        clipboard.source,
        targetState,
        source,
        target,
        sourcePath,
        targetDirectory
      )
      const targetPath = await availableCopyPath(target, targetDirectory, name)
      await copyTree(ctx, source, target, sourcePath, targetPath)
      pastedPaths.push(targetPath)
      copied += 1
    }
    if (targetState.ctx.isLocalSession()) {
      await ctx.writeClipboardFileUris(pastedPaths).catch(() => {
        /* Keep in-app paste successful when the OS rejects file URI data. */
      })
    }
    sendClipboardResult(ctx, 'paste', true, `Pasted ${copied} item${copied === 1 ? '' : 's'}`)
  } catch (err) {
    sendClipboardResult(ctx, 'paste', false, classifySftpError(err).message)
  } finally {
    pasteInProgress = false
  }
}

export function clearClipboardFor(state: SessionState): void {
  if (fileClipboard?.source === state) {
    fileClipboard = null
    broadcastClipboardState()
  }
}