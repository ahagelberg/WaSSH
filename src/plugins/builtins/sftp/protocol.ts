/** SFTP entry kind derived from the remote mode bits */
export type SftpEntryType = 'file' | 'directory' | 'symlink' | 'other'

/** One row in the remote file manager listing */
export interface SftpEntry {
  name: string
  /** Full remote path (parent + name) */
  path: string
  type: SftpEntryType
  size: number
  /** Numeric permission bits (e.g. 0o755) */
  mode: number
  /** Human-readable permission string (e.g. "-rw-r--r--") */
  modeSymbolic: string
  /** Last modified time (epoch ms) */
  mtime: number
  uid?: number
  gid?: number
}

export type SftpErrorKind =
  | 'not_ssh'
  | 'not_found'
  | 'permission'
  | 'not_dir'
  | 'exists'
  | 'name_in_use'
  | 'io'
  | 'connection'
  | 'cancelled'
  | 'other'

export type SftpStatusState = 'idle' | 'connecting' | 'connected' | 'error'

/** Main → renderer: connection / cwd status */
export interface SftpStatusPayload {
  type: 'status'
  state: SftpStatusState
  /** Current remote working directory */
  cwd?: string
  reason?: string
  errorKind?: SftpErrorKind
}

/** Main → renderer: directory listing result */
export interface SftpListPayload {
  type: 'listResult'
  path: string
  cwd: string
  entries: SftpEntry[]
  error?: string
  errorKind?: SftpErrorKind
}

export type SftpOpName = 'mkdir' | 'rename' | 'chmod' | 'delete'

/** Main → renderer: result of a mutating operation */
export interface SftpOpResultPayload {
  type: 'opResult'
  op: SftpOpName
  path: string
  ok: boolean
  error?: string
  errorKind?: SftpErrorKind
}

export type SftpTransferDirection = 'upload' | 'download' | 'download-zip'

/** Main → renderer: byte progress for an active transfer */
export interface SftpTransferProgressPayload {
  type: 'transferProgress'
  direction: SftpTransferDirection
  remotePath: string
  transferredBytes: number
  totalBytes: number
}

/** Main → renderer: transfer finished */
export interface SftpTransferDonePayload {
  type: 'transferDone'
  direction: SftpTransferDirection
  remotePath: string
  state: 'done' | 'error' | 'cancelled'
  error?: string
  errorKind?: SftpErrorKind
}

/** Main → renderer: content fetched for the in-panel file viewer */
export interface SftpViewFilePayload {
  type: 'viewFileResult'
  path: string
  ok: boolean
  /** 'text' = decoded text; 'binary' = raw bytes sent as base64 */
  kind?: 'text' | 'binary'
  /** Decoded text when kind === 'text' */
  text?: string
  /** Base64-encoded bytes when kind === 'binary' */
  contentBase64?: string
  /** Number of bytes actually fetched (≤ view cap) */
  bytesRead: number
  /** Full remote size; 0 when the stat failed */
  totalBytes?: number
  /** True when the remote file is larger than the view cap */
  truncated: boolean
  error?: string
  errorKind?: SftpErrorKind
}

export type SftpMainPayload =
  | SftpStatusPayload
  | SftpListPayload
  | SftpOpResultPayload
  | SftpTransferProgressPayload
  | SftpTransferDonePayload
  | SftpViewFilePayload

/** Renderer → main SFTP commands */
export type SftpRendererMessage =
  | { type: 'getStatus' }
  | { type: 'list'; path?: string }
  | { type: 'mkdir'; path: string }
  | { type: 'rename'; oldPath: string; newPath: string }
  | { type: 'chmod'; path: string; mode: number }
  | { type: 'delete'; path: string }
  | { type: 'download'; path: string }
  | { type: 'downloadZip'; path: string }
  | { type: 'viewFile'; path: string }
  | { type: 'uploadDialog'; path?: string }
  | { type: 'uploadStart'; name: string; size: number; path?: string }
  | { type: 'uploadChunk'; name: string; data: Uint8Array }
  | { type: 'uploadEnd'; name: string }
  | { type: 'cancel' }
  | { type: 'resetCwd' }

export const SFTP_RENDERER_MESSAGE_TYPES = new Set<SftpRendererMessage['type']>([
  'getStatus',
  'list',
  'mkdir',
  'rename',
  'chmod',
  'delete',
  'download',
  'downloadZip',
  'uploadDialog',
  'uploadStart',
  'uploadChunk',
  'uploadEnd',
  'cancel',
  'resetCwd',
  'viewFile'
])
