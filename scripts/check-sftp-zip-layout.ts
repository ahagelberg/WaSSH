// Verifies the folder→zip archive layout: contents at the zip root, no
// wrapping folder, and empty subdirectories preserved.
// Run: npx tsx scripts/check-sftp-zip-layout.ts
import {
  collectArchiveEntries,
  type ArchiveRemoteEntry
} from '../src/plugins/builtins/sftp/archiveLayout'

function assert(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(`FAIL: ${message}`)
  }
  console.log(`ok: ${message}`)
}

/** Fake remote tree: /root/{a.txt, docs/{b.txt, empty/}, link} */
const TREE: Record<string, ArchiveRemoteEntry[]> = {
  '/root': [
    { name: 'a.txt', path: '/root/a.txt', type: 'file', size: 10, mtime: 1, mode: 0o644 },
    { name: 'docs', path: '/root/docs', type: 'directory', size: 0, mtime: 1, mode: 0o755 },
    { name: 'link', path: '/root/link', type: 'symlink', size: 0, mtime: 1, mode: 0o777 }
  ],
  '/root/docs': [
    { name: 'b.txt', path: '/root/docs/b.txt', type: 'file', size: 20, mtime: 1, mode: 0o644 },
    { name: 'empty', path: '/root/docs/empty', type: 'directory', size: 0, mtime: 1, mode: 0o755 }
  ],
  '/root/docs/empty': []
}

const listDir = async (path: string): Promise<ArchiveRemoteEntry[]> => TREE[path] ?? []

async function main(): Promise<void> {
  const entries = await collectArchiveEntries(listDir, '/root', '')
  const paths = entries.map((entry) => entry.archivePath)

  assert(!paths.includes('/'), 'no wrapping root folder entry')
  assert(!paths.some((path) => path.startsWith('root/')), 'no folder name prefix on entries')
  assert(paths.includes('a.txt'), 'top-level file sits at the zip root')
  assert(paths.includes('docs/'), 'subfolder keeps a directory entry')
  assert(paths.includes('docs/b.txt'), 'nested file keeps its relative path')
  assert(paths.includes('docs/empty/'), 'empty subfolder is preserved')
  assert(!paths.some((path) => path.includes('link')), 'symlinks are skipped')

  const dirs = entries.filter((entry) => entry.directory).map((entry) => entry.archivePath)
  assert(dirs.length === 2, `two directory entries (got ${dirs.length}: ${dirs.join(', ')})`)
  assert(
    entries.every((entry) => !entry.archivePath.startsWith('/')),
    'no entry path is absolute'
  )

  const file = entries.find((entry) => entry.archivePath === 'a.txt')
  assert(file?.size === 10, 'file size is carried through')
  assert(file?.directory === false, 'file is not marked as a directory')

  // A nested call keeps the parent prefix.
  const nested = await collectArchiveEntries(listDir, '/root/docs', 'docs')
  const nestedPaths = nested.map((entry) => entry.archivePath)
  assert(nestedPaths.includes('docs/b.txt'), 'nested call prefixes with the parent path')
  assert(nestedPaths.includes('docs/empty/'), 'nested call keeps empty dirs')

  // An empty folder yields no entries (an empty zip, not a root entry).
  const empty = await collectArchiveEntries(listDir, '/root/docs/empty', '')
  assert(empty.length === 0, 'empty folder yields no entries')

  console.log('all sftp zip-layout checks passed')
}

void main()
