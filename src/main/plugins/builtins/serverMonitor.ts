import type { PluginMainModule } from '../PluginHost'
import type {
  ServerMonitorNetIface,
  ServerMonitorProcess,
  ServerMonitorSnapshot,
  ServerMonitorTemp
} from '../../../shared/plugins'
import {
  BYTES_PER_KIB,
  SERVER_MONITOR_DEFAULT_INTERVAL_MS,
  SERVER_MONITOR_DISK_SECTOR_BYTES,
  SERVER_MONITOR_LOOPBACK_IFACE,
  SERVER_MONITOR_MEGABIT_BITS,
  SERVER_MONITOR_MIN_INTERVAL_MS,
  SERVER_MONITOR_TEMP_MILLI_PER_C,
  SERVER_MONITOR_TOP_PROCESS_COUNT
} from '../../../shared/plugins'

/**
 * Machine-readable remote sample (Linux). CPU %, disk IO, and net rates need consecutive samples.
 * Process list is CPU-sorted; count matches SERVER_MONITOR_TOP_PROCESS_COUNT.
 */
const MONITOR_COMMAND = [
  "echo '===META==='",
  '(hostname 2>/dev/null || uname -n 2>/dev/null || echo unknown)',
  "(cat /proc/uptime 2>/dev/null | awk '{print $1}' || echo 0)",
  '(cat /proc/loadavg 2>/dev/null || echo 0 0 0 0/0 0)',
  '(uname -srm 2>/dev/null || echo unknown)',
  '(nproc 2>/dev/null || grep -c ^processor /proc/cpuinfo 2>/dev/null || echo 0)',
  "echo '===CPU==='",
  "(grep -E '^cpu' /proc/stat 2>/dev/null || echo cpu 0 0 0 0 0 0 0 0)",
  "echo '===MEM==='",
  "grep -E '^(MemTotal|MemAvailable|MemFree|Buffers|Cached|SwapTotal|SwapFree):' /proc/meminfo 2>/dev/null || true",
  "echo '===DISK==='",
  '(df -B1 / 2>/dev/null | tail -n 1 || echo)',
  "echo '===DISKIO==='",
  '(cat /proc/diskstats 2>/dev/null || true)',
  "echo '===NET==='",
  '(cat /proc/net/dev 2>/dev/null || true)',
  "echo '===NETSPEED==='",
  'for i in /sys/class/net/*; do [ -d "$i" ] || continue; n=$(basename "$i"); [ "$n" = lo ] && continue; s=$(cat "$i/speed" 2>/dev/null || echo -1); echo "$n $s"; done 2>/dev/null || true',
  "echo '===TEMP==='",
  'for z in /sys/class/thermal/thermal_zone*; do [ -r "$z/temp" ] || continue; t=$(cat "$z/type" 2>/dev/null || echo zone); v=$(cat "$z/temp" 2>/dev/null || continue); echo "$t $v"; done 2>/dev/null || true',
  "echo '===PROCS==='",
  `(ps -eo pid=,user:16=,state=,ni=,nlwp=,pcpu=,pmem=,rss=,args= --sort=-pcpu 2>/dev/null | head -n ${SERVER_MONITOR_TOP_PROCESS_COUNT} || true)`
].join('; ')

/** Max wait for one exec sample (ms) */
const EXEC_WAIT_MS = 10000

/** Milliseconds per second (rate math) */
const MS_PER_SEC = 1000

/** Whole-disk names in diskstats (skip partitions to avoid double-count) */
const WHOLE_DISK_RE = /^(sd[a-z]+|vd[a-z]+|xvd[a-z]+|hd[a-z]+|nvme\d+n\d+|mmcblk\d+)$/

interface CpuJiffies {
  idle: number
  total: number
}

interface NetCounters {
  at: number
  ifaces: Record<string, { rx: number; tx: number }>
}

interface DiskIoCounters {
  at: number
  readBytes: number
  writeBytes: number
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
  if (!/^cpu\d*$/.test(parts[0] || '') || parts.length < 5) {
    return null
  }
  const nums = parts.slice(1).map((x) => Number(x) || 0)
  const idle = (nums[3] || 0) + (nums[4] || 0)
  const total = nums.reduce((a, b) => a + b, 0)
  return { idle, total }
}

/** Aggregate line first, then cpu0..cpuN in order */
function parseCpuBlock(block: string): { aggregate: CpuJiffies | null; cores: CpuJiffies[] } {
  let aggregate: CpuJiffies | null = null
  const cores: CpuJiffies[] = []
  for (const line of block.split(/\n/)) {
    const trimmed = line.trim()
    if (!trimmed) {
      continue
    }
    const label = trimmed.split(/\s+/)[0] || ''
    const jiffies = parseCpuLine(trimmed)
    if (!jiffies) {
      continue
    }
    if (label === 'cpu') {
      aggregate = jiffies
      continue
    }
    cores.push(jiffies)
  }
  return { aggregate, cores }
}

function cpuPercentFromDelta(cur: CpuJiffies, prev: CpuJiffies): number | null {
  if (cur.total <= prev.total) {
    return null
  }
  const idleDelta = cur.idle - prev.idle
  const totalDelta = cur.total - prev.total
  return Math.max(0, Math.min(100, (1 - idleDelta / totalDelta) * 100))
}

function rateFromDelta(cur: number, prev: number | undefined, elapsedSec: number): number | null {
  if (prev == null || elapsedSec <= 0) {
    return null
  }
  return Math.max(0, (cur - prev) / elapsedSec)
}

function memKb(block: string, key: string): number {
  const re = new RegExp(`^${key}:\\s*(\\d+)`, 'm')
  const m = re.exec(block)
  return m ? Number(m[1]) * BYTES_PER_KIB : 0
}

function parseDisk(line: string): { total: number; used: number } {
  const parts = line.trim().split(/\s+/)
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

function parseDiskIo(block: string): { readBytes: number; writeBytes: number } {
  let readSectors = 0
  let writeSectors = 0
  for (const line of block.split(/\n/)) {
    const parts = line.trim().split(/\s+/)
    if (parts.length < 10) {
      continue
    }
    const name = parts[2]
    if (!WHOLE_DISK_RE.test(name)) {
      continue
    }
    readSectors += Number(parts[5]) || 0
    writeSectors += Number(parts[9]) || 0
  }
  return {
    readBytes: readSectors * SERVER_MONITOR_DISK_SECTOR_BYTES,
    writeBytes: writeSectors * SERVER_MONITOR_DISK_SECTOR_BYTES
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
    if (!name || name === 'face' || name === 'Inter-' || name === SERVER_MONITOR_LOOPBACK_IFACE) {
      continue
    }
    const nums = trimmed
      .slice(colon + 1)
      .trim()
      .split(/\s+/)
      .map((x) => Number(x) || 0)
    if (nums.length < 9) {
      continue
    }
    ifaces[name] = { rx: nums[0], tx: nums[8] }
  }
  return ifaces
}

/** sysfs net speed (Mbps) → bits/sec; invalid or down omitted */
function parseNetSpeeds(block: string): Record<string, number> {
  const speeds: Record<string, number> = {}
  for (const line of block.split(/\n/)) {
    const m = /^(\S+)\s+(-?\d+)$/.exec(line.trim())
    if (!m) {
      continue
    }
    const name = m[1]
    if (name === SERVER_MONITOR_LOOPBACK_IFACE) {
      continue
    }
    const mbps = Number(m[2])
    if (!Number.isFinite(mbps) || mbps <= 0) {
      continue
    }
    speeds[name] = mbps * SERVER_MONITOR_MEGABIT_BITS
  }
  return speeds
}

function netRates(
  current: Record<string, { rx: number; tx: number }>,
  speeds: Record<string, number>,
  prev: NetCounters | null,
  now: number
): ServerMonitorNetIface[] {
  const elapsedSec = prev && now > prev.at ? (now - prev.at) / MS_PER_SEC : 0
  const names = Object.keys(current).sort((a, b) => a.localeCompare(b))
  return names.map((name) => {
    const cur = current[name]
    const old = prev?.ifaces[name]
    const speedBits = speeds[name]
    return {
      name,
      rxBytes: cur.rx,
      txBytes: cur.tx,
      rxRate: rateFromDelta(cur.rx, old?.rx, elapsedSec),
      txRate: rateFromDelta(cur.tx, old?.tx, elapsedSec),
      speedBitsPerSec: speedBits != null && speedBits > 0 ? speedBits : null
    }
  })
}

function parseTemperatures(block: string): ServerMonitorTemp[] {
  const rows: ServerMonitorTemp[] = []
  for (const line of block.split(/\n/)) {
    const trimmed = line.trim()
    if (!trimmed) {
      continue
    }
    const m = /^(\S+)\s+(-?\d+)$/.exec(trimmed)
    if (!m) {
      continue
    }
    const milli = Number(m[2])
    if (!Number.isFinite(milli)) {
      continue
    }
    rows.push({
      name: m[1],
      celsius: milli / SERVER_MONITOR_TEMP_MILLI_PER_C
    })
  }
  return rows
}

function parseProcesses(block: string): ServerMonitorProcess[] {
  const rows: ServerMonitorProcess[] = []
  for (const line of block.split(/\n/)) {
    const trimmed = line.trim()
    if (!trimmed) {
      continue
    }
    const m =
      /^(\d+)\s+(\S+)\s+(\S+)\s+(-?\d+)\s+(\d+)\s+(\d+[.,]?\d*)\s+(\d+[.,]?\d*)\s+(\d+)\s+(.*)$/.exec(
        trimmed
      )
    if (!m) {
      continue
    }
    rows.push({
      pid: Number(m[1]) || 0,
      user: m[2],
      state: m[3],
      nice: Number(m[4]) || 0,
      threads: Number(m[5]) || 0,
      cpuPercent: Number(String(m[6]).replace(',', '.')) || 0,
      memPercent: Number(String(m[7]).replace(',', '.')) || 0,
      rssBytes: (Number(m[8]) || 0) * BYTES_PER_KIB,
      command: m[9].trim()
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
    cpuCores: [],
    memTotalBytes: 0,
    memUsedBytes: 0,
    memAvailableBytes: 0,
    memFreeBytes: 0,
    memBuffersBytes: 0,
    memCachedBytes: 0,
    swapTotalBytes: 0,
    swapUsedBytes: 0,
    diskTotalBytes: 0,
    diskUsedBytes: 0,
    diskReadBytes: 0,
    diskWriteBytes: 0,
    diskReadRate: null,
    diskWriteRate: null,
    temperatures: [],
    processes: [],
    network: [],
    ...partial
  }
}

function parseSample(
  raw: string,
  prevCpu: CpuJiffies | null,
  prevCores: CpuJiffies[],
  prevNet: NetCounters | null,
  prevDiskIo: DiskIoCounters | null
): {
  snapshot: ServerMonitorSnapshot
  cpu: CpuJiffies | null
  cores: CpuJiffies[]
  net: NetCounters
  diskIo: DiskIoCounters
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

  const { aggregate: cpu, cores } = parseCpuBlock(section(raw, 'CPU'))
  let cpuPercent: number | null = null
  if (cpu && prevCpu) {
    cpuPercent = cpuPercentFromDelta(cpu, prevCpu)
  }
  const cpuCores: Array<number | null> = cores.map((core, i) => {
    const prev = prevCores[i]
    return prev ? cpuPercentFromDelta(core, prev) : null
  })

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
  const diskIo = parseDiskIo(section(raw, 'DISKIO'))
  const diskElapsed =
    prevDiskIo && now > prevDiskIo.at ? (now - prevDiskIo.at) / MS_PER_SEC : 0
  const diskReadRate = rateFromDelta(diskIo.readBytes, prevDiskIo?.readBytes, diskElapsed)
  const diskWriteRate = rateFromDelta(diskIo.writeBytes, prevDiskIo?.writeBytes, diskElapsed)

  const netCounters = parseNetDev(section(raw, 'NET'))
  const netSpeeds = parseNetSpeeds(section(raw, 'NETSPEED'))
  const network = netRates(netCounters, netSpeeds, prevNet, now)
  const temperatures = parseTemperatures(section(raw, 'TEMP'))
  const processes = parseProcesses(section(raw, 'PROCS'))

  return {
    cpu,
    cores,
    net: { at: now, ifaces: netCounters },
    diskIo: { at: now, readBytes: diskIo.readBytes, writeBytes: diskIo.writeBytes },
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
      cpuCores,
      memTotalBytes: memTotal,
      memUsedBytes: memUsed,
      memAvailableBytes: available,
      memFreeBytes: memFree,
      memBuffersBytes: buffers,
      memCachedBytes: cached,
      swapTotalBytes: swapTotal,
      swapUsedBytes: swapUsed,
      diskTotalBytes: disk.total,
      diskUsedBytes: disk.used,
      diskReadBytes: diskIo.readBytes,
      diskWriteBytes: diskIo.writeBytes,
      diskReadRate,
      diskWriteRate,
      temperatures,
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
    let prevCores: CpuJiffies[] = []
    let prevNet: NetCounters | null = null
    let prevDiskIo: DiskIoCounters | null = null

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
        const parsed = parseSample(buf, prevCpu, prevCores, prevNet, prevDiskIo)
        if (parsed.cpu) {
          prevCpu = parsed.cpu
        }
        prevCores = parsed.cores
        prevNet = parsed.net
        prevDiskIo = parsed.diskIo
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
    let intervalMs = parseInterval(ctx.getSettings())
    const armTimer = (): void => {
      timer = setInterval(() => {
        const nextInterval = parseInterval(ctx.getSettings())
        if (nextInterval !== intervalMs) {
          intervalMs = nextInterval
          if (timer) {
            clearInterval(timer)
            timer = null
          }
          armTimer()
          return
        }
        void poll()
      }, intervalMs)
    }
    armTimer()

    ctx.onDeactivateCleanup(() => {
      stopped = true
      if (timer) {
        clearInterval(timer)
        timer = null
      }
    })
  }
}
