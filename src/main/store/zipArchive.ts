import { inflateRawSync } from 'zlib'
import { ZipFile } from 'yazl'

/**
 * Minimal zip reader for backup archives. `yazl` only writes, so reading uses
 * the central directory directly; entries are stored deflated or uncompressed
 * (both produced by `yazl`), which is all a backup needs.
 */

/** End-of-central-directory signature */
const EOCD_SIGNATURE = 0x06054b50

/** Central directory file header signature */
const CENTRAL_SIGNATURE = 0x02014b50

/** Local file header signature */
const LOCAL_SIGNATURE = 0x04034b50

/** EOCD fixed size before the trailing comment */
const EOCD_MIN_SIZE = 22

/** Largest comment the EOCD scan will walk back over */
const MAX_COMMENT_SIZE = 0xffff

/** Compression methods a backup may use */
const METHOD_STORE = 0
const METHOD_DEFLATE = 8

/** One entry read from an archive */
export interface ZipEntry {
  name: string
  data: Buffer
}

interface CentralEntry {
  name: string
  method: number
  compressedSize: number
  uncompressedSize: number
  localHeaderOffset: number
}

/** Write a zip containing `entries` (names must be unique and relative) */
export function writeZip(entries: Array<{ name: string; data: Buffer }>): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const archive = new ZipFile()
    const chunks: Buffer[] = []
    archive.outputStream.on('data', (chunk: Buffer) => chunks.push(chunk))
    archive.outputStream.on('error', reject)
    archive.outputStream.on('end', () => resolve(Buffer.concat(chunks)))
    for (const entry of entries) {
      archive.addBuffer(entry.data, entry.name)
    }
    archive.end()
  })
}

/** Read every entry from a zip buffer; throws when the archive is malformed */
export function readZip(buffer: Buffer): ZipEntry[] {
  const central = readCentralDirectory(buffer)
  return central.map((entry) => ({
    name: entry.name,
    data: readEntryData(buffer, entry)
  }))
}

/** Locate and parse the central directory */
function readCentralDirectory(buffer: Buffer): CentralEntry[] {
  const eocd = findEocd(buffer)
  if (eocd < 0) {
    throw new Error('Not a zip archive (no end-of-central-directory record)')
  }
  const entryCount = buffer.readUInt16LE(eocd + 10)
  let offset = buffer.readUInt32LE(eocd + 16)
  const entries: CentralEntry[] = []
  for (let i = 0; i < entryCount; i += 1) {
    if (offset + 46 > buffer.length || buffer.readUInt32LE(offset) !== CENTRAL_SIGNATURE) {
      throw new Error('Corrupt zip central directory')
    }
    const method = buffer.readUInt16LE(offset + 10)
    const compressedSize = buffer.readUInt32LE(offset + 20)
    const uncompressedSize = buffer.readUInt32LE(offset + 24)
    const nameLength = buffer.readUInt16LE(offset + 28)
    const extraLength = buffer.readUInt16LE(offset + 30)
    const commentLength = buffer.readUInt16LE(offset + 32)
    const localHeaderOffset = buffer.readUInt32LE(offset + 42)
    const name = buffer.toString('utf8', offset + 46, offset + 46 + nameLength)
    entries.push({ name, method, compressedSize, uncompressedSize, localHeaderOffset })
    offset += 46 + nameLength + extraLength + commentLength
  }
  return entries
}

/** Scan backwards for the EOCD record, allowing for a trailing comment */
function findEocd(buffer: Buffer): number {
  const minOffset = Math.max(0, buffer.length - EOCD_MIN_SIZE - MAX_COMMENT_SIZE)
  for (let offset = buffer.length - EOCD_MIN_SIZE; offset >= minOffset; offset -= 1) {
    if (buffer.readUInt32LE(offset) === EOCD_SIGNATURE) {
      return offset
    }
  }
  return -1
}

/** Read and inflate one entry's data via its local file header */
function readEntryData(buffer: Buffer, entry: CentralEntry): Buffer {
  const offset = entry.localHeaderOffset
  if (offset + 30 > buffer.length || buffer.readUInt32LE(offset) !== LOCAL_SIGNATURE) {
    throw new Error(`Corrupt zip entry header: ${entry.name}`)
  }
  const nameLength = buffer.readUInt16LE(offset + 26)
  const extraLength = buffer.readUInt16LE(offset + 28)
  const start = offset + 30 + nameLength + extraLength
  const raw = buffer.subarray(start, start + entry.compressedSize)
  if (entry.method === METHOD_STORE) {
    return Buffer.from(raw)
  }
  if (entry.method !== METHOD_DEFLATE) {
    throw new Error(`Unsupported zip compression method ${entry.method} for ${entry.name}`)
  }
  const inflated = inflateRawSync(raw)
  if (inflated.length !== entry.uncompressedSize) {
    throw new Error(`Zip entry size mismatch: ${entry.name}`)
  }
  return inflated
}
