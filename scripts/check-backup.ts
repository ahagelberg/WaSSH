// Smoke-tests the backup zip round-trip and manifest validation.
// Run: npx tsx scripts/check-backup.ts
import { readZip, writeZip } from '../src/main/store/zipArchive'
import {
  BACKUP_FORMAT_VERSION,
  BACKUP_MANIFEST_ENTRY,
  isBackupManifest,
  type BackupManifest
} from '../src/shared/backup'

function assert(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(`FAIL: ${message}`)
  }
  console.log(`ok: ${message}`)
}

// --- zip round-trip -------------------------------------------------------
const entries = [
  { name: 'manifest.json', data: Buffer.from('{"a":1}', 'utf8') },
  { name: 'settings.json', data: Buffer.from('{"theme":"dark"}', 'utf8') },
  { name: 'hosts.json', data: Buffer.from(JSON.stringify({ hosts: [] }), 'utf8') },
  // Non-ASCII + a large-ish payload to exercise deflate across chunks.
  { name: 'plugin-scratchpad.json', data: Buffer.from('åäö '.repeat(5000), 'utf8') },
  { name: 'vault.json', data: Buffer.from('{"k":"v"}', 'utf8') }
]

async function main(): Promise<void> {
  const archive = await writeZip(entries)
  assert(archive.length > 0, 'archive written')
  assert(archive.readUInt32LE(0) === 0x04034b50, 'archive starts with a local file header')

  const read = readZip(archive)
  assert(read.length === entries.length, `entry count round-trips (got ${read.length})`)
  for (const entry of entries) {
    const found = read.find((candidate) => candidate.name === entry.name)
    assert(found !== undefined, `entry present: ${entry.name}`)
    assert(
      found!.data.equals(entry.data),
      `entry bytes round-trip: ${entry.name} (${found!.data.length} vs ${entry.data.length})`
    )
  }

  // --- manifest validation ------------------------------------------------
  const manifest: BackupManifest = {
    formatVersion: BACKUP_FORMAT_VERSION,
    appVersion: '1.2.0',
    createdAt: Date.now(),
    entries: entries.map((entry) => entry.name)
  }
  assert(isBackupManifest(manifest), 'valid manifest accepted')
  assert(!isBackupManifest({ ...manifest, formatVersion: '1' }), 'string formatVersion rejected')
  assert(!isBackupManifest({ ...manifest, entries: [1, 2] }), 'non-string entries rejected')
  assert(!isBackupManifest(null), 'null rejected')
  assert(!isBackupManifest({ formatVersion: 1 }), 'missing fields rejected')

  // --- malformed input ----------------------------------------------------
  let threw = false
  try {
    readZip(Buffer.from('this is not a zip file at all', 'utf8'))
  } catch {
    threw = true
  }
  assert(threw, 'non-zip buffer is rejected')

  // An empty archive (EOCD only) yields no entries rather than throwing.
  const empty = Buffer.alloc(22)
  empty.writeUInt32LE(0x06054b50, 0)
  assert(readZip(empty).length === 0, 'empty archive yields no entries')

  console.log('all backup checks passed')
}

void main()
