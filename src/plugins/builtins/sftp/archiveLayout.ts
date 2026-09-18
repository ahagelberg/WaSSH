/**
 * Pure archive-layout helpers for the SFTP folder→zip download. Kept separate
 * from `transfers.ts` so the layout can be unit-tested without an SFTP session
 * (see `scripts/check-sftp-zip-layout.ts`).
 */

/** Size used for directory entries (they carry no data) */
export const ARCHIVE_EMPTY_SIZE = 0

/** One entry to write into the archive */
export interface ArchiveEntry {
  remotePath: string
  archivePath: string
  size: number
  mtime: number
  mode: number
  directory: boolean
}

/** Remote directory entry shape the collector needs (subset of SFTP list) */
export interface ArchiveRemoteEntry {
  name: string
  path: string
  type: string
  size: number
  mtime: number
  mode: number
}

/** Lists one remote directory (injected so tests need no SFTP session) */
export type ArchiveListDir = (remotePath: string) => Promise<ArchiveRemoteEntry[]>

/**
 * Entries for a folder's contents, placed at the archive root when
 * `archivePath` is empty (no wrapping folder entry). Subfolders keep their own
 * directory entries so an empty directory still survives the round trip.
 * Symlinks and other special files are skipped.
 */
export async function collectArchiveEntries(
  listDir: ArchiveListDir,
  remotePath: string,
  archivePath: string
): Promise<ArchiveEntry[]> {
  const entries: ArchiveEntry[] = []
  for (const child of await listDir(remotePath)) {
    if (child.type === 'symlink' || child.type === 'other') {
      continue
    }
    const childArchivePath = archivePath ? `${archivePath}/${child.name}` : child.name
    if (child.type === 'directory') {
      entries.push({
        remotePath: child.path,
        archivePath: `${childArchivePath}/`,
        size: ARCHIVE_EMPTY_SIZE,
        mtime: child.mtime,
        mode: child.mode,
        directory: true
      })
      entries.push(...(await collectArchiveEntries(listDir, child.path, childArchivePath)))
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
