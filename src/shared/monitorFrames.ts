import { CENTI_PER_C, BYTES_PER_KIB, type MonitorSeries } from './monitorSeries'

/**
 * Wire format for the remote daemon stream: length-prefixed binary frames.
 * A header frame describes the series set; sample frames carry one fixed-layout
 * record each. Both are parsed in the main process.
 */

/** Big-endian frame length prefix (bytes) */
export const FRAME_LENGTH_BYTES = 4

/** Frame types */
export const FRAME_KIND_HEADER = 1
export const FRAME_KIND_SAMPLE = 2

/** Marker byte the daemon writes before any frame (lets the app resync) */
export const FRAME_MAGIC = 0x57

/** Fixed part of a sample record, excluding the temperature/interface tails. */
const SAMPLE_FIXED_BYTES =
  4 + // epoch
  2 + // cpu tenths
  4 + // mem used KiB
  4 + // mem total KiB
  4 + // disk used KiB
  4 + // disk total KiB
  2 + // temperature count
  4 + // disk read B/s
  4 + // disk write B/s
  2 // interface count

/** Bytes per temperature entry */
const TEMP_ENTRY_BYTES = 2

/** Bytes per interface entry (state byte + rx + tx) */
const IFACE_ENTRY_BYTES = 1 + 4 + 4

export interface MonitorSample {
  /** ms since epoch */
  timestamp: number
  /** Aggregate CPU 0-100 */
  cpuPercent: number
  memoryUsedBytes: number
  memoryTotalBytes: number
  diskUsedBytes: number
  diskTotalBytes: number
  /** Bytes/sec */
  diskReadRate: number
  diskWriteRate: number
  /** °C, parallel to the header's temperature series order */
  temperatures: number[]
  /** Parallel to the header's interface series order */
  interfaces: Array<{ up: boolean; rxRate: number; txRate: number }>
}

export interface MonitorStreamHeader {
  series: MonitorSeries[]
  /** Thermal zone names, in record order */
  temperatureZones: string[]
  /** Interface names, in record order */
  interfaces: string[]
}

export type MonitorFrame =
  | { kind: typeof FRAME_KIND_HEADER; header: MonitorStreamHeader }
  | { kind: typeof FRAME_KIND_SAMPLE; sample: MonitorSample }

export function encodeSample(sample: MonitorSample): Uint8Array {
  const size =
    SAMPLE_FIXED_BYTES +
    sample.temperatures.length * TEMP_ENTRY_BYTES +
    sample.interfaces.length * IFACE_ENTRY_BYTES
  const view = new DataView(new ArrayBuffer(size))
  let offset = 0
  view.setUint32(offset, Math.floor(sample.timestamp / 1000), false)
  offset += 4
  view.setUint16(offset, clampUint16(Math.round(sample.cpuPercent * 10)), false)
  offset += 2
  view.setUint32(offset, clampUint32(Math.round(sample.memoryUsedBytes / BYTES_PER_KIB)), false)
  offset += 4
  view.setUint32(offset, clampUint32(Math.round(sample.memoryTotalBytes / BYTES_PER_KIB)), false)
  offset += 4
  view.setUint32(offset, clampUint32(Math.round(sample.diskUsedBytes / BYTES_PER_KIB)), false)
  offset += 4
  view.setUint32(offset, clampUint32(Math.round(sample.diskTotalBytes / BYTES_PER_KIB)), false)
  offset += 4
  view.setUint16(offset, sample.temperatures.length, false)
  offset += 2
  for (const celsius of sample.temperatures) {
    view.setInt16(offset, clampInt16(Math.round(celsius * CENTI_PER_C)), false)
    offset += 2
  }
  view.setUint32(offset, clampUint32(Math.round(sample.diskReadRate)), false)
  offset += 4
  view.setUint32(offset, clampUint32(Math.round(sample.diskWriteRate)), false)
  offset += 4
  view.setUint16(offset, sample.interfaces.length, false)
  offset += 2
  for (const iface of sample.interfaces) {
    view.setUint8(offset, iface.up ? 1 : 0)
    offset += 1
    view.setUint32(offset, clampUint32(Math.round(iface.rxRate)), false)
    offset += 4
    view.setUint32(offset, clampUint32(Math.round(iface.txRate)), false)
    offset += 4
  }
  return new Uint8Array(view.buffer)
}

/** Wrap a payload in a frame: magic, kind, big-endian length, body. */
export function encodeFrame(kind: number, body: Uint8Array): Uint8Array {
  const frame = new Uint8Array(1 + 1 + FRAME_LENGTH_BYTES + body.length)
  frame[0] = FRAME_MAGIC
  frame[1] = kind
  new DataView(frame.buffer).setUint32(2, body.length, false)
  frame.set(body, 2 + FRAME_LENGTH_BYTES)
  return frame
}

export function encodeHeader(header: MonitorStreamHeader): Uint8Array {
  const json = JSON.stringify({
    series: header.series,
    temperatureZones: header.temperatureZones,
    interfaces: header.interfaces
  })
  return encodeFrame(FRAME_KIND_HEADER, new TextEncoder().encode(json))
}

export function encodeSampleFrame(sample: MonitorSample): Uint8Array {
  return encodeFrame(FRAME_KIND_SAMPLE, encodeSample(sample))
}

/**
 * Incremental frame decoder. Feed it raw chunks in any split; it emits whole
 * frames and keeps the remainder buffered.
 */
export class MonitorFrameDecoder {
  private buffer = new Uint8Array(0)

  push(chunk: Uint8Array): MonitorFrame[] {
    if (chunk.length > 0) {
      const merged = new Uint8Array(this.buffer.length + chunk.length)
      merged.set(this.buffer, 0)
      merged.set(chunk, this.buffer.length)
      this.buffer = merged
    }
    const frames: MonitorFrame[] = []
    for (;;) {
      const frame = this.take()
      if (!frame) {
        break
      }
      frames.push(frame)
    }
    return frames
  }

  /** Drop any buffered partial frame (used when a stream restarts). */
  reset(): void {
    this.buffer = new Uint8Array(0)
  }

  private take(): MonitorFrame | null {
    const headerBytes = 2 + FRAME_LENGTH_BYTES
    if (this.buffer.length < headerBytes) {
      return null
    }
    if (this.buffer[0] !== FRAME_MAGIC) {
      // Resync on the next magic byte rather than dropping the whole buffer.
      const next = this.buffer.indexOf(FRAME_MAGIC, 1)
      this.buffer = next < 0 ? new Uint8Array(0) : this.buffer.slice(next)
      return null
    }
    const kind = this.buffer[1]
    const length = new DataView(
      this.buffer.buffer,
      this.buffer.byteOffset + 2,
      FRAME_LENGTH_BYTES
    ).getUint32(0, false)
    const total = headerBytes + length
    if (this.buffer.length < total) {
      return null
    }
    const body = this.buffer.slice(headerBytes, total)
    this.buffer = this.buffer.slice(total)
    if (kind === FRAME_KIND_HEADER) {
      return { kind: FRAME_KIND_HEADER, header: decodeHeader(body) }
    }
    if (kind === FRAME_KIND_SAMPLE) {
      const sample = decodeSample(body)
      return sample ? { kind: FRAME_KIND_SAMPLE, sample } : null
    }
    return null
  }
}

function decodeHeader(body: Uint8Array): MonitorStreamHeader {
  const parsed = JSON.parse(new TextDecoder().decode(body)) as Partial<MonitorStreamHeader>
  return {
    series: Array.isArray(parsed.series) ? parsed.series : [],
    temperatureZones: Array.isArray(parsed.temperatureZones) ? parsed.temperatureZones : [],
    interfaces: Array.isArray(parsed.interfaces) ? parsed.interfaces : []
  }
}

export function decodeSample(body: Uint8Array): MonitorSample | null {
  if (body.length < SAMPLE_FIXED_BYTES) {
    return null
  }
  const view = new DataView(body.buffer, body.byteOffset, body.length)
  let offset = 0
  const epoch = view.getUint32(offset, false)
  offset += 4
  const cpuTenths = view.getUint16(offset, false)
  offset += 2
  const memUsedKib = view.getUint32(offset, false)
  offset += 4
  const memTotalKib = view.getUint32(offset, false)
  offset += 4
  const diskUsedKib = view.getUint32(offset, false)
  offset += 4
  const diskTotalKib = view.getUint32(offset, false)
  offset += 4
  const tempCount = view.getUint16(offset, false)
  offset += 2
  if (body.length < offset + tempCount * TEMP_ENTRY_BYTES + 10) {
    return null
  }
  const temperatures: number[] = []
  for (let i = 0; i < tempCount; i += 1) {
    temperatures.push(view.getInt16(offset, false) / CENTI_PER_C)
    offset += 2
  }
  const diskReadRate = view.getUint32(offset, false)
  offset += 4
  const diskWriteRate = view.getUint32(offset, false)
  offset += 4
  const ifaceCount = view.getUint16(offset, false)
  offset += 2
  if (body.length < offset + ifaceCount * IFACE_ENTRY_BYTES) {
    return null
  }
  const interfaces: MonitorSample['interfaces'] = []
  for (let i = 0; i < ifaceCount; i += 1) {
    const up = view.getUint8(offset) === 1
    offset += 1
    const rxRate = view.getUint32(offset, false)
    offset += 4
    const txRate = view.getUint32(offset, false)
    offset += 4
    interfaces.push({ up, rxRate, txRate })
  }
  return {
    timestamp: epoch * 1000,
    cpuPercent: cpuTenths / 10,
    memoryUsedBytes: memUsedKib * BYTES_PER_KIB,
    memoryTotalBytes: memTotalKib * BYTES_PER_KIB,
    diskUsedBytes: diskUsedKib * BYTES_PER_KIB,
    diskTotalBytes: diskTotalKib * BYTES_PER_KIB,
    diskReadRate,
    diskWriteRate,
    temperatures,
    interfaces
  }
}

function clampUint16(value: number): number {
  return Math.max(0, Math.min(0xffff, value))
}

function clampUint32(value: number): number {
  return Math.max(0, Math.min(0xffffffff, value))
}

function clampInt16(value: number): number {
  return Math.max(-0x8000, Math.min(0x7fff, value))
}
