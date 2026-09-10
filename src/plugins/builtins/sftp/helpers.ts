import type { PluginMainContext } from '../../../main/plugins/api'
import { joinRemotePath, type SftpSession } from '../../../main/plugins/api'
import type {
  SftpErrorKind,
  SftpOpResultPayload,
  SftpRendererMessage,
  SftpStatusPayload
} from './protocol'
import { SFTP_RENDERER_MESSAGE_TYPES } from './protocol'
import type {
  SftpChunkUploadState,
  SftpDownloadState,
  SftpTransferState,
  SftpUploadState
} from './transfers'

export interface SessionState extends SftpTransferState {
  sftp: SftpSession | null
  cwd: string | null
  home: string
  stopped: boolean
  error: string | null
  errorKind: SftpErrorKind | null
  download: SftpDownloadState | null
  upload: SftpUploadState | null
  chunkUpload: SftpChunkUploadState | null
}

export type SftpOpMessage = Extract<
  SftpRendererMessage,
  { type: 'mkdir' | 'rename' | 'chmod' | 'delete' }
>

export const sessionStates = new Map<string, SessionState>()

export function instanceKey(ctx: PluginMainContext): string {
  return `${ctx.tabId}::${ctx.pluginId}`
}

export function stateFor(ctx: PluginMainContext): SessionState | undefined {
  return sessionStates.get(instanceKey(ctx))
}

export function isRendererMessage(payload: unknown): payload is SftpRendererMessage {
  if (!payload || typeof payload !== 'object') {
    return false
  }
  const type = (payload as { type?: unknown }).type
  return typeof type === 'string' && SFTP_RENDERER_MESSAGE_TYPES.has(type as SftpRendererMessage['type'])
}

export function sendStatus(
  ctx: PluginMainContext,
  payload: Omit<SftpStatusPayload, 'type'>
): void {
  ctx.sendToRenderer({ type: 'status', ...payload } satisfies SftpStatusPayload)
}

export function sendOpResult(
  ctx: PluginMainContext,
  payload: Omit<SftpOpResultPayload, 'type'>
): void {
  ctx.sendToRenderer({ type: 'opResult', ...payload } satisfies SftpOpResultPayload)
}

export async function resolveCwd(ctx: PluginMainContext, state: SessionState): Promise<string> {
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
