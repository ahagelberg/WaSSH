// Reproduces the SFTP folder-download bug: attaching a `data` listener to a
// stream that yazl will pipe makes the archive hang/emit nothing. Verifies the
// pass-through fix completes and preserves bytes.
// Run: npx tsx scripts/check-zip-pump.ts
import { PassThrough, Readable } from 'stream'
import { ZipFile } from 'yazl'
import { readZip } from '../src/main/store/zipArchive'

function assert(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(`FAIL: ${message}`)
  }
  console.log(`ok: ${message}`)
}

const PAYLOAD = Buffer.from('hello world '.repeat(2000), 'utf8')

/** Build a zip whose single entry is fed lazily, like the SFTP download does */
function buildZip(): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const archive = new ZipFile()
    const chunks: Buffer[] = []
    const timer = setTimeout(() => reject(new Error('timed out (archive never finished)')), 3000)
    archive.outputStream.on('data', (chunk: Buffer) => chunks.push(chunk))
    archive.outputStream.on('error', (err: Error) => {
      clearTimeout(timer)
      reject(err)
    })
    archive.outputStream.on('end', () => {
      clearTimeout(timer)
      resolve(Buffer.concat(chunks))
    })

    archive.addReadStreamLazy('dir/file.txt', { size: PAYLOAD.length }, (cb) => {
      const remote = Readable.from([PAYLOAD])
      // The fix: yazl owns the pipe, progress is counted on a pass-through.
      const progress = new PassThrough()
      progress.on('data', () => undefined)
      remote.pipe(progress)
      cb(null, progress)
    })
    archive.end()
  })
}

/** Same, but with several lazy entries to exercise sequential pumping */
function buildMultiEntryZip(): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const archive = new ZipFile()
    const chunks: Buffer[] = []
    const timer = setTimeout(() => reject(new Error('timed out (archive never finished)')), 3000)
    archive.outputStream.on('data', (chunk: Buffer) => chunks.push(chunk))
    archive.outputStream.on('error', (err: Error) => {
      clearTimeout(timer)
      reject(err)
    })
    archive.outputStream.on('end', () => {
      clearTimeout(timer)
      resolve(Buffer.concat(chunks))
    })

    archive.addEmptyDirectory('dir/')
    for (let i = 0; i < 2; i += 1) {
      const payload = Buffer.from(`entry ${i} `.repeat(500), 'utf8')
      archive.addReadStreamLazy(`dir/file${i}.txt`, { size: payload.length }, (cb) => {
        const remote = Readable.from([payload])
        const progress = new PassThrough()
        progress.on('data', () => undefined)
        remote.pipe(progress)
        cb(null, progress)
      })
    }
    archive.end()
  })
}

async function main(): Promise<void> {
  // The fixed pattern must finish and contain the payload.
  const fixed = await buildZip()
  assert(fixed.length > 0, `pass-through archive is non-empty (${fixed.length} bytes)`)
  const entries = readZip(fixed)
  assert(entries.length === 1, `archive has one entry (got ${entries.length})`)
  assert(entries[0].name === 'dir/file.txt', `entry name preserved (${entries[0].name})`)
  assert(
    entries[0].data.equals(PAYLOAD),
    `entry bytes match the source (${entries[0].data.length} vs ${PAYLOAD.length})`
  )

  // Multiple lazy entries must all be pumped in order.
  const multi = await buildMultiEntryZip()
  const multiEntries = readZip(multi)
  assert(multiEntries.length === 3, `multi-entry archive has 3 entries (got ${multiEntries.length})`)
  const files = multiEntries.filter((entry) => !entry.name.endsWith('/'))
  assert(files.length === 2, `multi-entry archive has 2 files (got ${files.length})`)
  assert(
    files.every((entry) => entry.data.length > 0),
    'every multi-entry file has data'
  )
  assert(
    files[0].data.toString('utf8').startsWith('entry 0'),
    'entries keep their order and content'
  )

  console.log('all zip-pump checks passed')
}

void main()
