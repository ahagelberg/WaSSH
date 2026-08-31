import type { FileEntryWithStats, SFTPWrapper, ReadStream, WriteStream, Stats } from 'ssh2'
import type { SftpEntry, SftpEntryType, SftpErrorKind } from '../../shared/plugins'

/** OpenSSH SFTPv3 status codes (superset of the base protocol) */
const SFTP_STATUS = {
  NO_SUCH_FILE: 2,
  PERMISSION_DENIED: 3,
  FAILURE: 4,
  NO_CONNECTION: 6,
  CONNECTION_LOST: 7,
  OP_UNSUPPORTED: 8,
  NO_SUCH_PATH: 10,
  FILE_ALREADY_EXISTS: 11,
  WRITE_PROTECT: 12,
  NO_SPACE_ON_FILESYSTEM: 14,
  QUOTA_EXCEEDED: 15,
  DIR_NOT_EMPTY: 18,
  NOT_A_DIRECTORY: 19,
  INVALID_FILENAME: 20,
  FILE_IS_A_DIRECTORY: 24
} as const

export interface SftpError {
  message: string
  kind: SftpErrorKind
}

/** Map a thrown SFTP error to a structured { message, kind } pair. */
export function classifySftpError(err: unknown): SftpError {
  const raw = err instanceof Error ? err : new Error(String(err))
  const code = (err as { code?: number | string } | null)?.code
  let kind: SftpErrorKind = 'other'
  if (typeof code === 'number') {
    switch (code) {
      case SFTP_STATUS.NO_SUCH_FILE:
      case SFTP_STATUS.NO_SUCH_PATH:
        kind = 'not_found'
        break
      case SFTP_STATUS.PERMISSION_DENIED:
      case SFTP_STATUS.WRITE_PROTECT:
        kind = 'permission'
        break
      case SFTP_STATUS.NO_CONNECTION:
      case SFTP_STATUS.CONNECTION_LOST:
        kind = 'connection'
        break
      case SFTP_STATUS.NOT_A_DIRECTORY:
        kind = 'not_dir'
        break
      case SFTP_STATUS.FILE_ALREADY_EXISTS:
        kind = 'exists'
        break
      case SFTP_STATUS.NO_SPACE_ON_FILESYSTEM:
      case SFTP_STATUS.QUOTA_EXCEEDED:
      case SFTP_STATUS.FAILURE:
      case SFTP_STATUS.DIR_NOT_EMPTY:
      case SFTP_STATUS.FILE_IS_A_DIRECTORY:
      case SFTP_STATUS.INVALID_FILENAME:
      case SFTP_STATUS.OP_UNSUPPORTED:
        kind = 'io'
        break
    }
  } else {
    const msg = raw.message.toLowerCase()
    if (msg.includes('no such file') || msg.includes('no such path')) {
      kind = 'not_found'
    } else if (msg.includes('permission')) {
      kind = 'permission'
    } else if (msg.includes('connection') || msg.includes('channel')) {
      kind = 'connection'
    }
  }
  return { message: raw.message, kind }
}

/** Join a remote POSIX path segment onto a parent directory (normalized). */
export function joinRemotePath(parent: string, name: string): string {
  if (parent === '/' || parent === '') {
    return `/${name}`
  }
  return `${parent.replace(/\/+$/, '')}/${name}`
}

function entryTypeFromMode(mode: number): SftpEntryType {
  const type = mode & 0o170000
  if (type === 0o040000) return 'directory'
  if (type === 0o120000) return 'symlink'
  if (type === 0o100000) return 'file'
  return 'other'
}

function symbolicMode(mode: number): string {
  const type = mode & 0o170000
  const typeChar =
    type === 0o040000 ? 'd' : type === 0o120000 ? 'l' : type === 0o100000 ? '-' : '?'
  const chars = ['r', 'w', 'x', 'r', 'w', 'x', 'r', 'w', 'x']
  const bits = [0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001]
  let out = typeChar
  for (let i = 0; i < 9; i++) {
    out += mode & bits[i] ? chars[i] : '-'
  }
  // Special bits (setuid / setgid / sticky)
  if (mode & 0o4000) {
    out = out.slice(0, 3) + (out[3] === 'x' ? 's' : 'S') + out.slice(4)
  }
  if (mode & 0o2000) {
    out = out.slice(0, 6) + (out[6] === 'x' ? 's' : 'S') + out.slice(7)
  }
  if (mode & 0o1000) {
    out = out.slice(0, 9) + (out[9] === 'x' ? 't' : 'T') + out.slice(10)
  }
  return out
}

function entryToSftpEntry(entry: FileEntryWithStats, parent: string): SftpEntry {
  const attrs = entry.attrs
  return {
    name: entry.filename,
    path: joinRemotePath(parent, entry.filename),
    type: entryTypeFromMode(attrs.mode),
    size: attrs.size,
    mode: attrs.mode,
    modeSymbolic: symbolicMode(attrs.mode),
    mtime: attrs.mtime * 1000,
    uid: attrs.uid,
    gid: attrs.gid
  }
}

/** Promisified wrapper around an ssh2 SFTPWrapper (one per live SSH client). */
export class SftpSession {
  readonly sftp: SFTPWrapper

  constructor(sftp: SFTPWrapper) {
    this.sftp = sftp
  }

  /** List a directory, mapping entries to renderer-friendly SftpEntry rows. */
  list(path: string): Promise<SftpEntry[]> {
    return new Promise((resolve, reject) => {
      this.sftp.readdir(path, (err, list) => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        const parent = path === '/' ? '' : path.replace(/\/+$/, '')
        resolve(list.map((entry) => entryToSftpEntry(entry, parent)))
      })
    })
  }

  mkdir(path: string): Promise<void> {
    return new Promise((resolve, reject) => {
      this.sftp.mkdir(path, (err) => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        resolve()
      })
    })
  }

  rename(oldPath: string, newPath: string): Promise<void> {
    return new Promise((resolve, reject) => {
      this.sftp.rename(oldPath, newPath, (err) => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        resolve()
      })
    })
  }

  chmod(path: string, mode: number): Promise<void> {
    return new Promise((resolve, reject) => {
      this.sftp.chmod(path, mode, (err) => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        resolve()
      })
    })
  }

  unlink(path: string): Promise<void> {
    return new Promise((resolve, reject) => {
      this.sftp.unlink(path, (err) => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        resolve()
      })
    })
  }

  rmdir(path: string): Promise<void> {
    return new Promise((resolve, reject) => {
      this.sftp.rmdir(path, (err) => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        resolve()
      })
    })
  }

  /**
   * Delete a file or directory (recursively for directories).
   * Uses lstat so symlinks are removed, never followed.
   */
  async delete(path: string): Promise<void> {
    const stats = await this.lstat(path)
    if (stats.isDirectory()) {
      const children = await this.list(path)
      for (const child of children) {
        await this.delete(child.path)
      }
      await this.rmdir(path)
    } else {
      await this.unlink(path)
    }
  }

  stat(path: string): Promise<Stats> {
    return new Promise((resolve, reject) => {
      this.sftp.stat(path, (err, stats) => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        resolve(stats)
      })
    })
  }

  lstat(path: string): Promise<Stats> {
    return new Promise((resolve, reject) => {
      this.sftp.lstat(path, (err, stats) => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        resolve(stats)
      })
    })
  }

  /** stat() that resolves null instead of rejecting (e.g. existence checks). */
  async statSafe(path: string): Promise<Stats | null> {
    try {
      return await this.stat(path)
    } catch {
      return null
    }
  }

  /**
   * Resolve a path to an absolute remote path.
   * Uses the OpenSSH tilde-expansion extension when available (handles "~", "~/..."),
   * otherwise falls back to plain realpath (no tilde support).
   */
  realpath(path: string): Promise<string> {
    return new Promise((resolve, reject) => {
      const expand = this.sftp.ext_openssh_expandPath as
        | ((p: string, cb: (err: Error | undefined, absPath: string) => void) => void)
        | undefined
      const finish = (err: Error | undefined, absPath: string): void => {
        if (err) {
          reject(classifySftpError(err))
          return
        }
        resolve(absPath)
      }
      if (typeof expand === 'function') {
        expand(path, finish)
      } else {
        this.sftp.realpath(path, finish)
      }
    })
  }

  createReadStream(path: string): ReadStream {
    return this.sftp.createReadStream(path)
  }

  createWriteStream(path: string): WriteStream {
    return this.sftp.createWriteStream(path)
  }

  end(): void {
    try {
      this.sftp.end()
    } catch {
      // already closed
    }
  }
}
