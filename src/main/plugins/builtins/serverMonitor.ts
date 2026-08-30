import type { PluginMainModule } from '../PluginHost'
import type {
  ServerMonitorNetIface,
  ServerMonitorProcess,
  ServerMonitorSnapshot
} from '../../../shared/plugins'
import {
  SERVER_MONITOR_DEFAULT_INTERVAL_MS,
  SERVER_MONITOR_MIN_INTERVAL_MS,
  SERVER_MONITOR_TOP_PROCESS_COUNT
} from '../../../shared/plugins'

/**
 * Machine-readable remote sample (Linux). CPU % and net rates need consecutive samples.
 * Process list is CPU-sorted; count matches SERVER_MONITOR_TOP_PROCESS_COUNT (+ header).
 */
const MONITOR_COMMAND = [
  "echo '===META==='",
  '(hostname 2>/dev/null || uname -n 2>/dev/null || echo unknown)',
  "(cat /proc/uptime 2>/dev/null | awk '{print $1}' || echo 0)",
  '(cat /proc/loadavg 2>/dev/null || echo 0 0 0 0/0 0)',
  '(uname -srm 2>/dev/null || echo unknown)',
  '(nproc 2>/dev/null || grep -c ^processor /proc/cpuinfo 2>/dev/null || echo 0)',
  "echo '===CPU==='",
  "(grep -E '^cpu ' /proc/stat 2>/dev/null || echo cpu 0 0 0 0 0 0 0 0)",
  "echo '===MEM==='",
  "grep -E '^(MemTotal|MemAvailable|MemFree|Buffers|Cached|SwapTotal|SwapFree):' /proc/meminfo 2>/dev/null || true",
  "echo '===DISK==='",
  '(df -B1 / 2>/dev/null | tail -n 1 || echo)',
  "echo '===NET==='",
  '(cat /proc/net/dev 2>/dev/null || true)',
  "echo '===PROCS==='",
  `(ps -eo pid=,user:16=,pcpu=,pmem=,rss=,comm= --sort=-pcpu 2>/dev/null | head -n ${SERVER_MONITOR_TOP_PROCESS_COUNT} || true)`
].join('; ')

/** Max wait for one exec sample (ms) */
const EXEC_WAIT_MS = 10000

/** Milliseconds per second (rate math) */
const MS_PER_SEC = 1000

/** Bytes in one kibibyte (ps rss is KiB) */
const BYTES_PER_KIB = 1024

interface CpuJiffies {
  idle: number
  total: number
}

interface NetCounters {
  at: number
  ifaces: Record<string, { rx: number; tx: number }>
}

function parseInterval(settings: Record<string, unknown>): number {
  const n = Number(settings.intervalMs)
  if (!Number.isFinite(n)) {
    return SERVER_MONITOR_DEFAULT_INTERVAL_MS
  }
  return Math.max(SERVER_MONITOR_MIN_INTERVAL_MS, Math.floor(n))
}

function section(raw: string, name: string): string {
  const re = new RegExp(`===${name}===\\s*([\\s\\S]*?)(?====|$)`)
  const m = re.exec(raw)
  return m ? m[1].trim() : ''
}

function parseCpuLine(line: string): CpuJiffies | null {
  const parts = line.trim().split(/\s+/)
  if (parts[0] !== 'cpu' || parts.length < 5) {
    return null
  }
  const nums = parts.slice(1).map((x) => Number(x) || 0)
  const idle = (nums[3] || 0) + (nums[4] || 0)
  const total = nums.reduce((a, b) => a + b, 0)
  return { idle, total }
}

function memKb(block: string, key: string): number {
  const re = new RegExp(`^${key}:\\s*(\\d+)`, 'm')
  const m = re.exec(block)
  return m ? Number(m[1]) * BYTES_PER_KIB : 0
}

function parseDisk(line: string): { total: number; used: number } {
  const parts = line.trim().split(/\s+/)
  // Filesystem 1B-blocks Used Available Use% Mounted
  if (parts.length < 4) {
    return { total: 0, used: 0 }
  }
  return {
    total: Number(parts[1]) || 0,
    used: Number(parts[2]) || 0
  }
}

function parseLoadProcs(field: string): { running: number; total: number } {
  const parts = field.split('/')
  return {
    running: Number(parts[0]) || 0,
    total: Number(parts[1]) || 0
  }
}

function parseNetDev(block: string): Record<string, { rx: number; tx: number }> {
  const ifaces: Record<string, { rx: number; tx: number }> = {}
  for (const line of block.split(/\n/)) {
    const trimmed = line.trim()
    const colon = trimmed.indexOf(':')
    if (colon < 0) {
      continue
    }
    const name = trimmed.slice(0, colon).trim()
    if (!name || name === 'face' || name === 'Inter-') {
      continue
    }
    const nums = trimmed
      .slice(colon + 1)
      .trim()
      .split(/\s+/)
      .map((x) => Number(x) || 0)
    // rx bytes, packets, errs, drop, fifo, frame, compressed, multicast,
    // tx bytes, ...
    if (nums.length < 9) {
      continue
    }
    ifaces[name] = { rx: nums[0], tx: nums[8] }
  }
  return ifaces
}

function netRates(
  current: Record<string, { rx: number; tx: number }>,
  prev: NetCounters | null,
  now: number
): ServerMonitorNetIface[] {
  const elapsedSec =
    prev && now > prev.at ? (now - prev.at) / MS_PER_SEC : 0
  const names = Object.keys(current).sort((a, b) => a.localeCompare(b))
  return names.map((name) => {
    const cur = current[name]
    const old = prev?.ifaces[name]
    let rxRate: number | null = null
    let txRate: number | null = null
    if (old && elapsedSec > 0) {
      rxRate = Math.max(0, (cur.rx - old.rx) / elapsedSec)
      txRate = Math.max(0, (cur.tx - old.tx) / elapsedSec)
    }
    return {
      name,
      rxBytes: cur.rx,
      txBytes: cur.tx,
      rxRate,
      txRate
    }
  })
}

function parseProcesses(block: string): ServerMonitorProcess[] {
  const rows: ServerMonitorProcess[] = []
  for (const line of block.split(/\n/)) {
    const trimmed = line.trim()
    if (!trimmed) {
      continue
    }
    // pid user pcpu pmem rss comm  — user may contain spaces if padded; use width-aware split
    const m = /^(\d+)\s+(\S+)\s+(\d+[.,]?\d*)\s+(\d+[.,]?\d*)\s+(\d+)\s+(.+)$/.exec(
      trimmed
    )
    if (!m) {
      continue
    }
    rows.push({
      pid: Number(m[1]) || 0,
      user: m[2],
      cpuPercent: Number(String(m[3]).replace(',', '.')) || 0,
      memPercent: Number(String(m[4]).replace(',', '.')) || 0,
      rssBytes: (Number(m[5]) || 0) * BYTES_PER_KIB,
      command: m[6].trim()
    })
    if (rows.length >= SERVER_MONITOR_TOP_PROCESS_COUNT) {
      break
    }
  }
  return rows
}

function emptySnapshot(partial: Partial<ServerMonitorSnapshot> = {}): ServerMonitorSnapshot {
  return {
    updatedAt: Date.now(),
    hostname: '',
    uptimeSec: 0,
    load1: 0,
    load5: 0,
    load15: 0,
    kernel: '',
    cpuCount: 0,
    procsRunning: 0,
    procsTotal: 0,
    cpuPercent: null,
    memTotalBytes: 0,
    memUsedBytes: 0,
    memAvailableBytes: 0,
    swapTotalBytes: 0,
    swapUsedBytes: 0,
    diskTotalBytes: 0,
    diskUsedBytes: 0,
    processes: [],
    network: [],
    ...partial
  }
}

function parseSample(
  raw: string,
  prevCpu: CpuJiffies | null,
  prevNet: NetCounters | null
): {
  snapshot: ServerMonitorSnapshot
  cpu: CpuJiffies | null
  net: NetCounters
} {
  const now = Date.now()
  const meta = section(raw, 'META').split(/\n/).map((l) => l.trim()).filter(Boolean)
  const hostname = meta[0] || 'unknown'
  const uptimeSec = Number(meta[1]) || 0
  const loadParts = (meta[2] || '0 0 0 0/0 0').split(/\s+/)
  const load1 = Number(loadParts[0]) || 0
  const load5 = Number(loadParts[1]) || 0
  const load15 = Number(loadParts[2]) || 0
  const procs = parseLoadProcs(loadParts[3] || '0/0')
  const kernel = meta[3] || ''
  const cpuCount = Number(meta[4]) || 0

  const cpuLine = section(raw, 'CPU').split(/\n/)[0] || ''
  const cpu = parseCpuLine(cpuLine)
  let cpuPercent: number | null = null
  if (cpu && prevCpu && cpu.total > prevCpu.total) {
    const idleDelta = cpu.idle - prevCpu.idle
    const totalDelta = cpu.total - prevCpu.total
    cpuPercent = Math.max(0, Math.min(100, (1 - idleDelta / totalDelta) * 100))
  }

  const memBlock = section(raw, 'MEM')
  const memTotal = memKb(memBlock, 'MemTotal')
  const memAvailable = memKb(memBlock, 'MemAvailable')
  const memFree = memKb(memBlock, 'MemFree')
  const buffers = memKb(memBlock, 'Buffers')
  const cached = memKb(memBlock, 'Cached')
  const available = memAvailable || memFree + buffers + cached
  const memUsed = Math.max(0, memTotal - available)
  const swapTotal = memKb(memBlock, 'SwapTotal')
  const swapFree = memKb(memBlock, 'SwapFree')
  const swapUsed = Math.max(0, swapTotal - swapFree)

  const disk = parseDisk(section(raw, 'DISK').split(/\n/)[0] || '')
  const netCounters = parseNetDev(section(raw, 'NET'))
  const network = netRates(netCounters, prevNet, now)
  const processes = parseProcesses(section(raw, 'PROCS'))

  return {
    cpu,
    net: { at: now, ifaces: netCounters },
    snapshot: emptySnapshot({
      updatedAt: now,
      hostname,
      uptimeSec,
      load1,
      load5,
      load15,
      kernel,
      cpuCount,
      procsRunning: procs.running,
      procsTotal: procs.total,
      cpuPercent,
      memTotalBytes: memTotal,
      memUsedBytes: memUsed,
      memAvailableBytes: available,
      swapTotalBytes: swapTotal,
      swapUsedBytes: swapUsed,
      diskTotalBytes: disk.total,
      diskUsedBytes: disk.used,
      processes,
      network
    })
  }
}

export const serverMonitorMain: PluginMainModule = {
  async onActivate(ctx) {
    let timer: ReturnType<typeof setInterval> | null = null
    let busy = false
    let stopped = false
    let prevCpu: CpuJiffies | null = null
    let prevNet: NetCounters | null = null

    const poll = async (): Promise<void> => {
      if (busy || stopped) {
        return
      }
      busy = true
      try {
        const connectionId = await ctx.openSideConnection({
          kind: 'ssh-exec',
          command: MONITOR_COMMAND
        })
        let buf = ''
        const offData = ctx.onSideData(connectionId, (chunk) => {
          buf += chunk
        })
        await new Promise<void>((resolve) => {
          const finish = (): void => resolve()
          const offClose = ctx.onSideClosed(connectionId, () => {
            offClose()
            finish()
          })
          setTimeout(() => {
            offClose()
            ctx.closeSideConnection(connectionId)
            finish()
          }, EXEC_WAIT_MS)
        })
        offData()
        const parsed = parseSample(buf, prevCpu, prevNet)
        if (parsed.cpu) {
          prevCpu = parsed.cpu
        }
        prevNet = parsed.net
        ctx.sendToRenderer({
          type: 'stats',
          snapshot: parsed.snapshot
        })
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        ctx.sendToRenderer({
          type: 'stats',
          snapshot: emptySnapshot({ error: message })
        })
      } finally {
        busy = false
      }
    }

    void poll()
    const interval = parseInterval(ctx.getSettings())
    timer = setInterval(() => {
      void poll()
    }, interval)

    ctx.onDeactivateCleanup(() => {
      stopped = true
      if (timer) {
        clearInterval(timer)
        timer = null
      }
    })
  }
}
