import type { DockerStatsResponse } from './apiTypes'
import type { DockerLogChunk, DockerPullProgress, DockerStatsView } from './protocol'

/**
 * Docker's multiplexed stream framing (used for non-TTY container output and
 * attach/exec streams): one byte of stream id, three padding bytes, then a
 * big-endian uint32 payload length.
 */
const FRAME_HEADER_BYTES = 8
const STREAM_STDIN = 0
const STREAM_STDOUT = 1
const STREAM_STDERR = 2

/**
 * Reassembles Docker's multiplexed log/attach stream into text chunks. TTY
 * containers are not framed, so their output passes through untouched.
 */
export class DockerLogDemuxer {
  private buffer: Buffer = Buffer.alloc(0)

  constructor(private readonly tty: boolean) {}

  push(chunk: Buffer): DockerLogChunk[] {
    if (this.tty) {
      return [{ stream: 'raw', text: chunk.toString('utf8') }]
    }

    const merged = this.buffer.length > 0 ? Buffer.concat([this.buffer, chunk]) : chunk
    const chunks: DockerLogChunk[] = []
    let offset = 0

    while (merged.length - offset >= FRAME_HEADER_BYTES) {
      const streamId = merged[offset]
      const size = merged.readUInt32BE(offset + 4)
      if (merged.length - offset - FRAME_HEADER_BYTES < size) {
        break
      }
      const payload = merged.subarray(offset + FRAME_HEADER_BYTES, offset + FRAME_HEADER_BYTES + size)
      offset += FRAME_HEADER_BYTES + size
      if (payload.length > 0) {
        chunks.push({
          stream: streamId === STREAM_STDERR ? 'stderr' : streamId === STREAM_STDIN ? 'raw' : 'stdout',
          text: payload.toString('utf8')
        })
      }
    }

    this.buffer = offset >= merged.length ? Buffer.alloc(0) : Buffer.from(merged.subarray(offset))
    return chunks
  }

  /** Emit anything left in the buffer when the stream ends mid-frame. */
  flush(): DockerLogChunk[] {
    if (this.buffer.length === 0) {
      return []
    }
    const text = this.buffer.toString('utf8')
    this.buffer = Buffer.alloc(0)
    return text ? [{ stream: 'raw', text }] : []
  }
}

/** Splits a byte stream into newline-delimited JSON strings. */
export class JsonLineSplitter {
  private buffer = ''

  push(chunk: Buffer): string[] {
    this.buffer += chunk.toString('utf8')
    const lines = this.buffer.split('\n')
    this.buffer = lines.pop() ?? ''
    const out: string[] = []
    for (const line of lines) {
      const trimmed = line.trim()
      if (trimmed) {
        out.push(trimmed)
      }
    }
    return out
  }

  flush(): string[] {
    const trimmed = this.buffer.trim()
    this.buffer = ''
    return trimmed ? [trimmed] : []
  }
}

export interface DockerPullEvent {
  image: string
  status: string
  current: number | null
  total: number | null
  error?: string
  done: boolean
}

/** Interpret one line of `POST /images/create` progress output. */
export function parsePullLine(line: string, image: string): DockerPullEvent | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(line)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== 'object') {
    return null
  }
  const record = parsed as Record<string, unknown>
  const errorDetail = record.errorDetail
  const errorMessage =
    (errorDetail && typeof errorDetail === 'object' && typeof (errorDetail as { message?: unknown }).message === 'string'
      ? ((errorDetail as { message: string }).message)
      : undefined) ?? (typeof record.error === 'string' ? record.error : undefined)

  const progressDetail = record.progressDetail
  const current =
    progressDetail && typeof progressDetail === 'object' && typeof (progressDetail as { current?: unknown }).current === 'number'
      ? ((progressDetail as { current: number }).current)
      : null
  const total =
    progressDetail && typeof progressDetail === 'object' && typeof (progressDetail as { total?: unknown }).total === 'number'
      ? ((progressDetail as { total: number }).total)
      : null

  return {
    image,
    status: typeof record.status === 'string' ? record.status : '',
    current,
    total,
    error: errorMessage,
    done: false
  }
}

function sumBlkio(op: string, stats: DockerStatsResponse): number {
  const entries = stats.blkio_stats?.io_service_bytes_recursive
  if (!Array.isArray(entries)) {
    return 0
  }
  let total = 0
  for (const entry of entries) {
    if (entry && typeof entry.op === 'string' && entry.op.toLowerCase() === op) {
      total += typeof entry.value === 'number' ? entry.value : 0
    }
  }
  return total
}

function sumNetwork(
  stats: DockerStatsResponse,
  pick: (entry: { rx_bytes?: number; tx_bytes?: number }) => number | undefined
): number {
  const networks = stats.networks
  if (!networks) {
    return 0
  }
  let total = 0
  for (const entry of Object.values(networks)) {
    if (entry) {
      total += pick(entry) ?? 0
    }
  }
  return total
}

/**
 * Convert a raw `/containers/{id}/stats` payload into the normalized view
 * model. `stream=false` responses carry both the current and previous CPU
 * counters, so the CPU percentage is computed within the single sample.
 */
export function normalizeStats(raw: DockerStatsResponse): DockerStatsView {
  const cpu = raw.cpu_stats
  const precpu = raw.precpu_stats
  const cpuDelta = (cpu?.cpu_usage?.total_usage ?? 0) - (precpu?.cpu_usage?.total_usage ?? 0)
  const systemDelta = (cpu?.system_cpu_usage ?? 0) - (precpu?.system_cpu_usage ?? 0)
  const onlineCpus = cpu?.online_cpus ?? cpu?.cpu_usage?.percpu_usage?.length ?? 1
  const cpuPercent = systemDelta > 0 && cpuDelta > 0 ? (cpuDelta / systemDelta) * onlineCpus * 100 : 0

  const memoryUsage = raw.memory_stats?.usage ?? 0
  const memoryLimit = raw.memory_stats?.limit ?? 0

  return {
    timestamp: Date.parse(raw.read ?? '') || Date.now(),
    cpuPercent: Math.max(0, Math.round(cpuPercent * 100) / 100),
    memoryUsed: memoryUsage,
    memoryLimit,
    memoryPercent: memoryLimit > 0 ? Math.round((memoryUsage / memoryLimit) * 10000) / 100 : 0,
    pids: raw.pids_stats?.current ?? 0,
    netRx: sumNetwork(raw, (entry) => entry.rx_bytes),
    netTx: sumNetwork(raw, (entry) => entry.tx_bytes),
    blockRead: sumBlkio('read', raw),
    blockWrite: sumBlkio('write', raw)
  }
}

/** Empty stats sample used before the first real reading arrives. */
export function emptyStats(): DockerStatsView {
  return {
    timestamp: Date.now(),
    cpuPercent: 0,
    memoryUsed: 0,
    memoryLimit: 0,
    memoryPercent: 0,
    pids: 0,
    netRx: 0,
    netTx: 0,
    blockRead: 0,
    blockWrite: 0
  }
}

/** Completed pull event used when the pull stream closes cleanly. */
export function donePullEvent(image: string): DockerPullProgress {
  return { image, status: 'Pull complete', current: null, total: null, done: true }
}
