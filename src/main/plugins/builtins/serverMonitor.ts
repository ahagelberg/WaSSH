import type { PluginMainContext, PluginMainModule } from '../PluginHost'
import type {
  ServerMonitorNetIface,
  ServerMonitorProcess,
  ServerMonitorProcessSignal,
  ServerMonitorProcessSort,
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
  SERVER_MONITOR_PROCESS_SORT_DEFAULT,
  SERVER_MONITOR_PROCESS_SORT_DESC_DEFAULT,
  SERVER_MONITOR_PROCESS_SORT_KEYS,
  SERVER_MONITOR_TEMP_MILLI_PER_C,
  SERVER_MONITOR_TOP_PROCESS_COUNT
} from '../../../shared/plugins'

/** `ps --sort=` field for each UI column */
const PS_SORT_FIELD: Record<ServerMonitorProcessSort, string> = {
  pid: 'pid',
  user: 'user',
  state: 'state',
  nice: 'nice',
  threads: 'nlwp',
  cpu: 'pcpu',
  mem: 'pmem',
  command: 'args'
}

/** Max wait for one exec sample (ms) */
const EXEC_WAIT_MS = 10000

/** Process rows a baseline list must have before short samples are held back */
const PROC_LIST_STABLE_MIN = 10

/** Consecutive short samples before a shorter process list is accepted */
const PROC_LIST_STALE_LIMIT = 3

/** Milliseconds per second (rate math) */
const MS_PER_SEC = 1000

/** Whole-disk names in diskstats (skip partitions to avoid double-count) */
const WHOLE_DISK_RE = /^(sd[a-z]+|vd[a-z]+|xvd[a-z]+|hd[a-z]+|nvme\d+n\d+|mmcblk\d+)$/

/** Marker prefix for kill exit status in execCapture output */
const KILL_EXIT_MARKER = '__EC:'

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

/** OS families the monitor can sample (Linux stays the primary path) */
type MonitorFamily = 'linux' | 'macos' | 'freebsd'

interface MonitorSessionState {
  stopped: boolean
  busy: boolean
  processSort: ServerMonitorProcessSort
  processSortDesc: boolean
  prevCpu: CpuJiffies | null
  prevCores: CpuJiffies[]
  prevNet: NetCounters | null
  prevDiskIo: DiskIoCounters | null
  timer: ReturnType<typeof setInterval> | null
  intervalMs: number
  poll: () => Promise<void>
  /** OS family detected from `uname`; selects the sample command + parser */
  family: MonitorFamily
  /** Last full process list, kept when a sample's list collapses (transient ps cut) */
  lastProcesses: ServerMonitorProcess[]
  /** Consecutive collapsed-list samples */
  shortSamples: number
}

interface RendererMessage {
  type: 'setProcessSort' | 'signalProcess' | 'refresh'
  sort?: ServerMonitorProcessSort
  descending?: boolean
  pid?: number
  signal?: ServerMonitorProcessSignal
}

const sessionStates = new Map<string, MonitorSessionState>()

function instanceKey(ctx: PluginMainContext): string {
  return `${ctx.tabId}:${ctx.pluginId}`
}

function parseInterval(settings: Record<string, unknown>): number {
  const n = Number(settings.intervalMs)
  if (!Number.isFinite(n)) {
    return SERVER_MONITOR_DEFAULT_INTERVAL_MS
  }
  return Math.max(SERVER_MONITOR_MIN_INTERVAL_MS, Math.floor(n))
}

/** Map UI sort to `ps --sort=` key (leading - = descending). */
function psSortKey(sort: ServerMonitorProcessSort, descending: boolean): string {
  const field = PS_SORT_FIELD[sort]
  return descending ? `-${field}` : field
}

/**
 * Machine-readable remote sample (Linux). CPU %, disk IO, and net rates need consecutive samples.
 * Process list uses remote `ps --sort` for the active column so the sample is a true top-N.
 */
function buildMonitorCommand(sort: ServerMonitorProcessSort, descending: boolean): string {
  const procSort = psSortKey(sort, descending)
  return [
    "echo '===META==='",
    '(hostname 2>/dev/null || uname -n 2>/dev/null || echo unknown)',
    "(cat /proc/uptime 2>/dev/null | awk '{print $1}' || echo 0)",
    '(cat /proc/loadavg 2>/dev/null || echo 0 0 0 0/0 0)',
    '(uname -srm 2>/dev/null || echo unknown)',
    '(nproc 2>/dev/null || grep -c ^processor /proc/cpuinfo 2>/dev/null || echo 0)',
    "echo '===IP==='",
    "(out=$(ip -4 -o addr show scope global 2>/dev/null | awk '{print $4}' | cut -d/ -f1); if [ -n \"$out\" ]; then echo \"$out\"; else hostname -I 2>/dev/null; fi)",
    "echo '===OS==='",
    "(cat /etc/os-release /usr/lib/os-release /etc/lsb-release /etc/redhat-release /etc/system-release /etc/arch-release /etc/SuSE-release 2>/dev/null || true)",
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
    "echo '===IPMAP==='",
    "(out=$(ip -o addr show scope global 2>/dev/null | awk '{print $2, $4}' | sed 's#/.*##'); if [ -z \"$out\" ]; then out=$(ifconfig -a 2>/dev/null | awk '{ if ($0 ~ /^[A-Za-z0-9.]+:/) { iface = $1; sub(/:/, \"\", iface) } if (iface != \"\" && $1 == \"inet\") { a = $2; sub(/^addr:/, \"\", a); if (a != \"127.0.0.1\") print iface, a } }'); fi; if [ -n \"$out\" ]; then printf '%s\\n' \"$out\"; fi)",
    "echo '===NETSPEED==='",
    'for i in /sys/class/net/*; do [ -d "$i" ] || continue; n=$(basename "$i"); [ "$n" = lo ] && continue; s=$(cat "$i/speed" 2>/dev/null || echo -1); echo "$n $s"; done 2>/dev/null || true',
    "echo '===TEMP==='",
    'for z in /sys/class/thermal/thermal_zone*; do [ -r "$z/temp" ] || continue; t=$(cat "$z/type" 2>/dev/null || echo zone); v=$(cat "$z/temp" 2>/dev/null || continue); echo "$t $v"; done 2>/dev/null || true',
    "echo '===PROCS==='",
    `(ps -eo pid=,user:16=,state=,ni=,nlwp=,pcpu=,pmem=,rss=,args= --sort=${procSort} 2>/dev/null | head -n ${SERVER_MONITOR_TOP_PROCESS_COUNT} || true)`
  ].join('; ')
}

/**
 * BSD/macOS sample command. Shares Linux section markers + shapes where
 * possible (META/OS/CPU/MEM/DISK/PROCS), so parsers can mostly be reused.
 */
function buildUnixCommand(family: MonitorFamily, sort: ServerMonitorProcessSort): string {
  const isMac = family === 'macos'
  const parts: string[] = [
    // hostname / uptime / loadavg / kernel / cpu count
    "echo '===META==='",
    '(hostname 2>/dev/null || uname -n 2>/dev/null || echo unknown)',
    '(b=$(sysctl -n kern.boottime 2>/dev/null | sed -n "s/.*sec = \\([0-9][0-9]*\\).*/\\1/p"); if [ -n "$b" ]; then echo $(( $(date +%s 2>/dev/null || echo 0) - b )); else echo 0; fi)',
    'echo "$(sysctl -n vm.loadavg 2>/dev/null | tr -d "{}" || echo 0 0 0) 0/0"',
    '(uname -srm 2>/dev/null || echo unknown)',
    '(sysctl -n hw.ncpu 2>/dev/null || echo 1)',
    "echo '===IP==='",
    "(ifconfig -a 2>/dev/null | awk '$1 == \"inet\" && $2 != \"127.0.0.1\" { print $2 }')",
    // OS / distro
    "echo '===OS==='",
    isMac
      ? '(echo "PRETTY_NAME=\\"$(sw_vers -productName 2>/dev/null) $(sw_vers -productVersion 2>/dev/null)\\"")'
      : '(cat /etc/os-release /usr/lib/os-release 2>/dev/null || true)',
    '(echo "$(uname -sr 2>/dev/null)")',
    // CPU: macOS reports a ready-made percent; BSD gives tick counters for deltas
    "echo '===CPU==='",
    isMac
      ? "(top -l 1 -n 0 2>/dev/null | awk '/CPU usage:/' || true)"
      : "(sysctl -n kern.cp_time 2>/dev/null | awk '{ print \"cpu\", $1, $2, $3, $5, $4 }' || true)",
    // Memory in Linux-shaped MemTotal/… kB lines
    "echo '===MEM==='",
    isMac
      ? '(p=$(sysctl -n hw.pagesize 2>/dev/null || echo 4096); t=$(sysctl -n hw.memsize 2>/dev/null || echo 0); f=$(vm_stat 2>/dev/null | awk \'/Pages free/{print $3}\' | tr -d "."); i=$(vm_stat 2>/dev/null | awk \'/Pages inactive/{print $3}\' | tr -d "."); s=$(vm_stat 2>/dev/null | awk \'/Pages speculative/{print $3}\' | tr -d "."); f=${f:-0}; i=${i:-0}; s=${s:-0}; echo "MemTotal: $((t / 1024)) kB"; echo "MemAvailable: $(((f + i + s) * p / 1024)) kB"; echo "MemFree: $((f * p / 1024)) kB")'
      : '(p=$(sysctl -n hw.pagesize 2>/dev/null || echo 4096); t=$(sysctl -n hw.physmem 2>/dev/null || echo 0); f=$(sysctl -n vm.stats.vm.v_free_count 2>/dev/null || echo 0); i=$(sysctl -n vm.stats.vm.v_inactive_count 2>/dev/null || echo 0); c=$(sysctl -n vm.stats.vm.v_cache_count 2>/dev/null || echo 0); f=${f:-0}; i=${i:-0}; c=${c:-0}; echo "MemTotal: $((t / 1024)) kB"; echo "MemAvailable: $(((f + i + c) * p / 1024)) kB"; echo "MemFree: $((f * p / 1024)) kB")',
    isMac
      ? '(sysctl vm.swapusage 2>/dev/null | awk \'{ t=0; u=0; for (i = 1; i <= NF; i++) { if ($i == "total") { gsub(/[A-Za-z]/, "", $(i+2)); t = $(i+2) * 1024 } if ($i == "free") { gsub(/[A-Za-z]/, "", $(i+2)); u = $(i+2) * 1024 } } } END { if (t > 0) { print "SwapTotal: " t " kB"; print "SwapFree: " u " kB" } }\' || true)'
      : '(swapinfo -k 2>/dev/null | awk \'NR > 1 { s += $2; a += $4 } END { if (s > 0) { print "SwapTotal: " s " kB"; print "SwapFree: " a " kB" } }\' || true)',
    // Root disk usage (KB blocks on BSD/macOS)
    "echo '===DISK==='",
    '(df -k / 2>/dev/null | tail -n 1 || true)',
    // Per-interface RX/TX byte counters (link rows only)
    "echo '===NET==='",
    '(netstat -ib 2>/dev/null | awk \'NR == 1 { for (i = 1; i <= NF; i++) { if ($i == "Ibytes") cI = i; if ($i == "Obytes") cO = i } } NR > 1 { for (i = 1; i <= NF; i++) { if (index($i, "<Link") == 1) { print $1, $cI, $cO; break } } }\' || true)',
    // Interface → IPv4 address map (used for the network list hover tooltip)
    "echo '===IPMAP==='",
    "(ifconfig -a 2>/dev/null | awk '{ if ($0 ~ /^[A-Za-z0-9.]+:/) { iface = $1; sub(/:/, \"\", iface) } if (iface != \"\" && $1 == \"inet\" && $2 != \"127.0.0.1\" && iface != \"lo0\") print iface, $2 }' || true)"
  ]
  if (!isMac) {
    parts.push(
      "echo '===TEMP==='",
      '(i=0; n=$(sysctl -n hw.ncpu 2>/dev/null || echo 1); while [ "$i" -lt "$n" ]; do t=$(sysctl -n dev.cpu.$i.temperature 2>/dev/null); if [ -n "$t" ]; then echo "cpu$i $t"; fi; i=$((i + 1)); done || true)'
    )
  }
  parts.push(
    "echo '===PROCS==='",
    buildBsdProcCommand(isMac, sort === 'mem')
  )
  return parts.join('; ')
}

/**
 * BSD/macOS process list. BSD `ps -o` keywords differ across flavors (and some
 * reject unknown ones entirely), so try the rich set first and fall back to a
 * minimal set that only uses keywords found on every BSD-derived `ps`.
 */
function buildBsdProcCommand(isMac: boolean, byMem: boolean): string {
  const psField = isMac ? 'command' : 'args'
  const psState = isMac ? 'state' : 'stat'
  const orderFlag = byMem ? '-m' : '-r'
  return `(rows=$(ps -A ${orderFlag} -o pid=,user=,${psState}=,nice=,%cpu=,%mem=,rss=,${psField}= 2>/dev/null); if [ -n "$rows" ]; then printf '%s\n' "$rows" | head -n ${SERVER_MONITOR_TOP_PROCESS_COUNT}; else ps -A ${orderFlag} -o pid=,user=,%cpu=,%mem=,rss=,${psField}= 2>/dev/null | awk '{ cmd=$6; for (j = 7; j <= NF; j++) cmd = cmd " " $j; print $1, $2, "?", 0, $3, $4, $5, cmd }' | head -n ${SERVER_MONITOR_TOP_PROCESS_COUNT}; fi)`
}

/**
 * Wrap a command so it runs under a POSIX `/bin/sh`, whatever the remote login
 * shell is (e.g. FreeBSD defaults to csh).
 */
function shWrap(payload: string): string {
  return `/bin/sh -c '${payload.replace(/'/g, `'\\''`)}'`
}

/** Pick the sample command for the detected OS family. */
function buildSampleCommand(
  family: MonitorFamily,
  sort: ServerMonitorProcessSort,
  descending: boolean
): string {
  const payload =
    family === 'linux'
      ? buildMonitorCommand(sort, descending)
      : buildUnixCommand(family, sort)
  return family === 'linux' ? payload : shWrap(payload)
}

/** Detect the OS family from a sample's META kernel line (`uname -srm`). */
function detectFamilyFromSample(raw: string): MonitorFamily {
  const meta = section(raw, 'META')
    .split(/\n/)
    .map((l) => l.trim())
    .filter(Boolean)
  const first = (meta[3] || '').split(/\s+/)[0] || ''
  if (first === 'Darwin') {
    return 'macos'
  }
  if (first === 'FreeBSD' || first === 'OpenBSD' || first === 'NetBSD' || first === 'DragonFly') {
    return 'freebsd'
  }
  return 'linux'
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

/** Unique, plausible v4/v6 address tokens reported by the sample command */
function parseIps(block: string): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const raw of block.split(/[\s,]+/)) {
    const ip = raw.trim()
    if (!ip || ip === '127.0.0.1' || ip === '::1' || seen.has(ip)) {
      continue
    }
    if (!/^[0-9a-fA-F:.]+$/.test(ip)) {
      continue
    }
    seen.add(ip)
    out.push(ip)
  }
  return out
}

/** Strip surrounding quotes from an os-release style value */
function unquoteField(value: string): string {
  const v = value.trim()
  if (v.length >= 2) {
    const first = v[0]
    const last = v[v.length - 1]
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      return v.slice(1, -1).trim()
    }
  }
  return v
}

/** Best-effort distro name (PRETTY_NAME preferred) from os-release style files */
function parseDistroName(block: string): string {
  let name = ''
  let version = ''
  let pretty = ''
  for (const raw of block.split(/\n/)) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) {
      continue
    }
    const eq = line.indexOf('=')
    if (eq <= 0) {
      if (!pretty) {
        pretty = line
      }
      continue
    }
    const key = line.slice(0, eq).trim()
    const value = unquoteField(line.slice(eq + 1))
    if (!value) {
      continue
    }
    if (key === 'PRETTY_NAME') {
      return value
    }
    if (key === 'DISTRIB_DESCRIPTION' && !pretty) {
      pretty = value
    }
    if (key === 'NAME' && !name) {
      name = value
    }
    if (key === 'VERSION' && !version) {
      version = value
    }
  }
  if (pretty) {
    return pretty
  }
  if (name) {
    return version && !name.includes(version) ? `${name} ${version}` : name
  }
  return ''
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

function parseIfaceIps(block: string): Record<string, string[]> {
  const out: Record<string, string[]> = {}
  for (const raw of block.split(/\n/)) {
    const line = raw.trim()
    const parts = line.split(/\s+/)
    if (parts.length < 2) {
      continue
    }
    const [name, ip] = parts
    if (!name || !/^[0-9a-fA-F:.]+$/.test(ip)) {
      continue
    }
    (out[name] ??= []).push(ip)
  }
  return out
}

function netRates(
  current: Record<string, { rx: number; tx: number }>,
  speeds: Record<string, number>,
  prev: NetCounters | null,
  now: number,
  ifaceIps: Record<string, string[]> = {}
): ServerMonitorNetIface[] {
  const elapsedSec = prev && now > prev.at ? (now - prev.at) / MS_PER_SEC : 0
  const names = Object.keys(current).sort((a, b) => a.localeCompare(b))
  return names.map((name) => {
    const cur = current[name]
    const old = prev?.ifaces[name]
    const speedBits = speeds[name]
    return {
      name,
      ips: ifaceIps[name] || [],
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
    // Linux: "<type> <millidegrees>"
    const milli = /^(\S+)\s+(-?\d+)$/.exec(trimmed)
    if (milli) {
      const value = Number(milli[2])
      if (Number.isFinite(value)) {
        rows.push({
          name: milli[1],
          celsius: value / SERVER_MONITOR_TEMP_MILLI_PER_C
        })
      }
      continue
    }
    // BSD sysctl: "<name> 45.0C"
    const degC = /^(\S+)\s+(-?[\d.]+)C$/.exec(trimmed)
    if (degC) {
      const value = Number(degC[2])
      if (Number.isFinite(value)) {
        rows.push({ name: degC[1], celsius: value })
      }
    }
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

/** netstat -ib rows (`name rxBytes txBytes`) for BSD/macOS */
function parseBsdNetworkCounters(block: string): Record<string, { rx: number; tx: number }> {
  const ifaces: Record<string, { rx: number; tx: number }> = {}
  for (const line of block.split(/\n/)) {
    const parts = line.trim().split(/\s+/)
    if (parts.length < 3 || parts[0].startsWith('lo')) {
      continue
    }
    const rx = Number(parts[1])
    const tx = Number(parts[2])
    if (!Number.isFinite(rx) || !Number.isFinite(tx)) {
      continue
    }
    ifaces[parts[0]] = { rx, tx }
  }
  return ifaces
}

/** Single-shot CPU percent from `top -l 1` (macOS has no /proc/stat). */
function parseMacCpuPercent(block: string): number | null {
  const m = /([\d.]+)%\s+idle/.exec(block)
  if (!m) {
    return null
  }
  const idle = Number(m[1])
  if (!Number.isFinite(idle)) {
    return null
  }
  return Math.max(0, Math.min(100, 100 - idle))
}

/** BSD `ps -A -r -o …` rows (thread count not available on all BSD flavors). */
function parseBsdProcesses(block: string): ServerMonitorProcess[] {
  const rows: ServerMonitorProcess[] = []
  for (const line of block.split(/\n/)) {
    const trimmed = line.trim()
    if (!trimmed) {
      continue
    }
    const m =
      /^(\d+)\s+(\S+)\s+(\S+)\s+(\S+)\s+(\d+[.,]?\d*)\s+(\d+[.,]?\d*)\s+(\d+)\s+(.*)$/.exec(
        trimmed
      )
    if (!m) {
      continue
    }
    const nice = Number(m[4]) || 0
    rows.push({
      pid: Number(m[1]) || 0,
      user: m[2],
      state: m[3],
      nice,
      threads: 0,
      cpuPercent: Number(String(m[5]).replace(',', '.')) || 0,
      memPercent: Number(String(m[6]).replace(',', '.')) || 0,
      rssBytes: (Number(m[7]) || 0) * BYTES_PER_KIB,
      command: m[8].trim()
    })
    if (rows.length >= SERVER_MONITOR_TOP_PROCESS_COUNT) {
      break
    }
  }
  return rows
}

/**
 * Assemble a snapshot for macOS/FreeBSD samples. Data not available without
 * root (macOS temps, per-core CPU, disk IO) degrades to empty/null.
 */
function parseUnixSample(
  raw: string,
  family: MonitorFamily,
  prevCpu: CpuJiffies | null,
  prevNet: NetCounters | null
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
  const distro = parseDistroName(section(raw, 'OS'))
  const kernel = meta[3] || ''
  const cpuCount = Number(meta[4]) || 0
  const ips = parseIps(section(raw, 'IP'))

  let cpu: CpuJiffies | null = null
  let cpuPercent: number | null = null
  const cores: CpuJiffies[] = []
  if (family === 'macos') {
    cpuPercent = parseMacCpuPercent(section(raw, 'CPU'))
  } else {
    cpu = parseCpuBlock(section(raw, 'CPU')).aggregate ?? null
    if (cpu && prevCpu) {
      cpuPercent = cpuPercentFromDelta(cpu, prevCpu)
    }
  }

  const memBlock = section(raw, 'MEM')
  const memTotal = memKb(memBlock, 'MemTotal')
  const memAvailable = memKb(memBlock, 'MemAvailable')
  const memFree = memKb(memBlock, 'MemFree')
  const available = memAvailable || memFree
  const memUsed = Math.max(0, memTotal - available)
  const swapTotal = memKb(memBlock, 'SwapTotal')
  const swapFree = memKb(memBlock, 'SwapFree')
  const swapUsed = Math.max(0, swapTotal - swapFree)

  // BSD/macOS `df -k` reports 1024-byte blocks.
  const disk = parseDisk(section(raw, 'DISK').split(/\n/)[0] || '')
  const diskTotalBytes = disk.total * BYTES_PER_KIB
  const diskUsedBytes = disk.used * BYTES_PER_KIB

  const netCounters = parseBsdNetworkCounters(section(raw, 'NET'))
  const network = netRates(netCounters, {}, prevNet, now, parseIfaceIps(section(raw, 'IPMAP')))
  const temperatures = parseTemperatures(section(raw, 'TEMP'))
  const processes = parseBsdProcesses(section(raw, 'PROCS'))

  return {
    cpu,
    cores,
    net: { at: now, ifaces: netCounters },
    diskIo: { at: now, readBytes: 0, writeBytes: 0 },
    snapshot: emptySnapshot({
      updatedAt: now,
      hostname,
      uptimeSec,
      load1,
      load5,
      load15,
      distro,
      kernel,
      cpuCount,
      procsRunning: procs.running,
      procsTotal: procs.total,
      cpuPercent,
      cpuCores: [] as Array<number | null>,
      memTotalBytes: memTotal,
      memUsedBytes: memUsed,
      memAvailableBytes: available,
      memFreeBytes: memFree,
      memBuffersBytes: 0,
      memCachedBytes: 0,
      swapTotalBytes: swapTotal,
      swapUsedBytes: swapUsed,
      diskTotalBytes,
      diskUsedBytes,
      diskReadBytes: 0,
      diskWriteBytes: 0,
      diskReadRate: null,
      diskWriteRate: null,
      ips,
      temperatures,
      processes,
      network
    })
  }
}

function emptySnapshot(partial: Partial<ServerMonitorSnapshot> = {}): ServerMonitorSnapshot {
  return {
    updatedAt: Date.now(),
    hostname: '',
    ips: [],
    uptimeSec: 0,
    load1: 0,
    load5: 0,
    load15: 0,
    distro: '',
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
  const distro = parseDistroName(section(raw, 'OS'))
  const kernel = meta[3] || ''
  const cpuCount = Number(meta[4]) || 0
  const ips = parseIps(section(raw, 'IP'))

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
  const network = netRates(netCounters, netSpeeds, prevNet, now, parseIfaceIps(section(raw, 'IPMAP')))
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
      distro,
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
      ips,
      temperatures,
      processes,
      network
    })
  }
}

function isValidPid(pid: unknown): pid is number {
  return typeof pid === 'number' && Number.isInteger(pid) && pid > 0
}

function isProcessSort(value: unknown): value is ServerMonitorProcessSort {
  return (
    typeof value === 'string' &&
    (SERVER_MONITOR_PROCESS_SORT_KEYS as string[]).includes(value)
  )
}

function isProcessSignal(value: unknown): value is ServerMonitorProcessSignal {
  return value === 'TERM' || value === 'KILL'
}

function isRendererMessage(payload: unknown): payload is RendererMessage {
  if (!payload || typeof payload !== 'object') {
    return false
  }
  const type = (payload as { type?: unknown }).type
  return type === 'setProcessSort' || type === 'signalProcess' || type === 'refresh'
}

async function signalRemoteProcess(
  ctx: PluginMainContext,
  pid: number,
  signal: ServerMonitorProcessSignal
): Promise<{ ok: boolean; error?: string }> {
  const sig = signal === 'KILL' ? 'KILL' : 'TERM'
  try {
    const out = await ctx.execCapture(
      `kill -s ${sig} ${pid} 2>&1; printf '\\n${KILL_EXIT_MARKER}%s\\n' $?`
    )
    const marker = `${KILL_EXIT_MARKER}`
    const idx = out.lastIndexOf(marker)
    const codeRaw = idx >= 0 ? out.slice(idx + marker.length).trim() : ''
    const code = Number(codeRaw)
    if (code === 0) {
      return { ok: true }
    }
    const stderr = (idx >= 0 ? out.slice(0, idx) : out).trim()
    if (stderr) {
      return { ok: false, error: stderr }
    }
    if (code === 1) {
      return { ok: false, error: 'Permission denied or no such process' }
    }
    return { ok: false, error: `kill failed (exit ${Number.isFinite(code) ? code : '?'})` }
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) }
  }
}

function armTimer(ctx: PluginMainContext, state: MonitorSessionState): void {
  if (state.timer) {
    clearInterval(state.timer)
    state.timer = null
  }
  state.timer = setInterval(() => {
    const nextInterval = parseInterval(ctx.getSettings())
    if (nextInterval !== state.intervalMs) {
      state.intervalMs = nextInterval
      armTimer(ctx, state)
      return
    }
    void state.poll()
  }, state.intervalMs)
}

export const serverMonitorMain: PluginMainModule = {
  async onActivate(ctx) {
    const state: MonitorSessionState = {
      stopped: false,
      busy: false,
      processSort: SERVER_MONITOR_PROCESS_SORT_DEFAULT,
      processSortDesc: SERVER_MONITOR_PROCESS_SORT_DESC_DEFAULT,
      family: 'linux',
      prevCpu: null,
      prevCores: [],
      prevNet: null,
      prevDiskIo: null,
      lastProcesses: [],
      shortSamples: 0,
      timer: null,
      intervalMs: parseInterval(ctx.getSettings()),
      poll: async () => undefined
    }

    state.poll = async (): Promise<void> => {
      if (state.busy || state.stopped) {
        return
      }
      state.busy = true
      try {
        const connectionId = await ctx.openSideConnection({
          kind: 'ssh-exec',
          command: buildSampleCommand(state.family, state.processSort, state.processSortDesc)
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

        // The first sample may have run the Linux command against a BSD/macOS
        // host; switch family and clear cross-family deltas for the next poll.
        const family = detectFamilyFromSample(buf)
        if (family !== state.family) {
          state.family = family
          state.prevCpu = null
          state.prevCores = []
          state.prevNet = null
          state.prevDiskIo = null
        }

        const parsed =
          family === 'linux'
            ? parseSample(buf, state.prevCpu, state.prevCores, state.prevNet, state.prevDiskIo)
            : parseUnixSample(buf, family, state.prevCpu, state.prevNet)
        if (parsed.cpu) {
          state.prevCpu = parsed.cpu
        }
        state.prevCores = parsed.cores
        state.prevNet = parsed.net
        state.prevDiskIo = parsed.diskIo

        // A sample that comes back with far fewer process rows than the last full
        // list is usually a cut/partial ps output; hold the stable list briefly so
        // the table does not jump between sizes every poll.
        const lastProcs = state.lastProcesses
        const sampledProcs = parsed.snapshot.processes
        let procs = sampledProcs
        if (
          lastProcs.length >= PROC_LIST_STABLE_MIN &&
          sampledProcs.length * 2 < lastProcs.length
        ) {
          state.shortSamples += 1
          if (state.shortSamples <= PROC_LIST_STALE_LIMIT) {
            procs = lastProcs
          } else {
            state.shortSamples = 0
          }
        } else {
          state.shortSamples = 0
        }
        state.lastProcesses = procs
        parsed.snapshot.processes = procs

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
        state.busy = false
      }
    }

    sessionStates.set(instanceKey(ctx), state)
    ctx.onDeactivateCleanup(() => {
      state.stopped = true
      if (state.timer) {
        clearInterval(state.timer)
        state.timer = null
      }
      sessionStates.delete(instanceKey(ctx))
    })

    void state.poll()
    armTimer(ctx, state)
  },

  async onMessage(ctx, payload) {
    if (!isRendererMessage(payload)) {
      return undefined
    }
    const state = sessionStates.get(instanceKey(ctx))
    if (!state || state.stopped) {
      return undefined
    }

    if (payload.type === 'refresh') {
      void state.poll()
      return { ok: true }
    }

    if (payload.type === 'setProcessSort') {
      if (!isProcessSort(payload.sort)) {
        return { ok: false, error: 'Invalid sort' }
      }
      const descending =
        typeof payload.descending === 'boolean'
          ? payload.descending
          : SERVER_MONITOR_PROCESS_SORT_DESC_DEFAULT
      state.processSort = payload.sort
      state.processSortDesc = descending
      void state.poll()
      return { ok: true }
    }

    if (payload.type === 'signalProcess') {
      if (!isValidPid(payload.pid) || !isProcessSignal(payload.signal)) {
        return { ok: false, error: 'Invalid pid or signal' }
      }
      const result = await signalRemoteProcess(ctx, payload.pid, payload.signal)
      if (result.ok) {
        void state.poll()
      }
      return result
    }

    return undefined
  }
}
