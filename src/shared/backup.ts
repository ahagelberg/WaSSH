/**
 * Backup archive format shared by the main process (writer/reader) and the
 * renderer (result types). The archive is a zip containing `manifest.json`
 * plus one entry per config file, so a user can inspect or hand-edit it.
 */

/** Archive entry holding the format version + provenance */
export const BACKUP_MANIFEST_ENTRY = 'manifest.json'

/** Backup format version; bump when the archive layout changes */
export const BACKUP_FORMAT_VERSION = 1

/** File extension offered in the save/open dialogs */
export const BACKUP_FILE_EXTENSION = 'zip'

/** Manifest stored as the first entry of every backup archive */
export interface BackupManifest {
  /** Archive layout version (see `BACKUP_FORMAT_VERSION`) */
  formatVersion: number
  /** App version that produced the archive (`APP_VERSION`) */
  appVersion: string
  /** Creation time (ms since epoch) */
  createdAt: number
  /** Config files included, in archive order */
  entries: string[]
}

/** Why a restore was refused; `null` when the archive is usable */
export type BackupImportProblem =
  | 'not-a-zip'
  | 'missing-manifest'
  | 'bad-manifest'
  | 'newer-format'
  | 'empty'

/** Result of reading a backup archive */
export interface BackupImportResult {
  ok: boolean
  /** Failure reason when `ok` is false */
  problem?: BackupImportProblem
  /** Human-readable failure detail */
  error?: string
  /** Archive metadata when readable */
  manifest?: BackupManifest
  /** True when the archive's app version differs from the running app */
  versionMismatch?: boolean
}

/** Result of writing a backup archive */
export interface BackupExportResult {
  ok: boolean
  /** Written file path when `ok` is true */
  path?: string
  /** Number of config files written */
  entryCount?: number
  error?: string
  /** True when the user dismissed the save dialog */
  cancelled?: boolean
}

/** Result of applying a backup archive */
export interface BackupRestoreResult {
  ok: boolean
  /** Config files restored */
  restored?: string[]
  error?: string
}

/** Whether a manifest is structurally usable */
export function isBackupManifest(value: unknown): value is BackupManifest {
  if (!value || typeof value !== 'object') {
    return false
  }
  const manifest = value as Partial<BackupManifest>
  return (
    typeof manifest.formatVersion === 'number' &&
    typeof manifest.appVersion === 'string' &&
    typeof manifest.createdAt === 'number' &&
    Array.isArray(manifest.entries) &&
    manifest.entries.every((entry) => typeof entry === 'string')
  )
}
