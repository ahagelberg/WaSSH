// Validates the remote daemon: python syntax plus wire-format conformance.
// Reads src/plugins/builtins/server-monitor/wassh-service.py directly and runs
// its `--selftest` frames through the shared decoder.
// Run: npx tsx scripts/check-daemon.ts
import { execFileSync } from 'node:child_process'
import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  FRAME_KIND_HEADER,
  FRAME_KIND_SAMPLE,
  MonitorFrameDecoder
} from '../src/shared/monitorFrames'

const DAEMON_PATH = 'src/plugins/builtins/server-monitor/wassh-service.py'

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) {
    throw new Error(`FAIL: ${message}`)
  }
  console.log(`ok: ${message}`)
}

/** python3 / python / py -3, whichever exists on this machine. */
function findPython(): { command: string; prefix: string[] } | null {
  for (const [command, prefix] of [
    ['python3', []],
    ['python', []],
    ['py', ['-3']]
  ] as ReadonlyArray<readonly [string, readonly string[]]>) {
    try {
      execFileSync(command, [...prefix, '--version'], { stdio: 'pipe' })
      return { command, prefix: [...prefix] }
    } catch {
      /* try the next candidate */
    }
  }
  return null
}

const python = findPython()
if (!python) {
  console.log('daemon check: skipped (no python3/python/py on PATH)')
  process.exit(0)
}

// --- syntax ---------------------------------------------------------------
const cacheDir = mkdtempSync(join(tmpdir(), 'wassh-daemon-'))
execFileSync(
  python.command,
  [
    ...python.prefix,
    '-c',
    'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)',
    DAEMON_PATH,
    join(cacheDir, 'wassh-service.pyc')
  ],
  { stdio: 'pipe' }
)
console.log('ok: daemon syntax compiles')

// --- wire format ----------------------------------------------------------
const output = execFileSync(python.command, [...python.prefix, DAEMON_PATH, '--selftest'], {
  stdio: ['ignore', 'pipe', 'pipe']
})
const frames = new MonitorFrameDecoder().push(new Uint8Array(output))
assert(frames.length === 2, `selftest emits a header frame and a sample frame (got ${frames.length})`)

const headerFrame = frames[0]
assert(headerFrame.kind === FRAME_KIND_HEADER, 'first frame is a header frame')
const sampleFrame = frames[1]
assert(sampleFrame.kind === FRAME_KIND_SAMPLE, 'second frame is a sample frame')

const header = headerFrame.header
assert(header.interfaces.length === 1 && header.interfaces[0] === 'eth0', 'header carries the interface list')
assert(header.temperatureZones.length === 1 && header.temperatureZones[0] === 'zone0', 'header carries the thermal zone list')
assert(header.series.some((series) => series.id === 'net:eth0:rx'), 'header declares net series')
assert(header.series.some((series) => series.id === 'temp:zone0'), 'header declares temp series')

const sample = sampleFrame.sample
assert(sample.timestamp === 1_700_000_000_000, `epoch seconds decode to ms (got ${sample.timestamp})`)
assert(Math.abs(sample.cpuPercent - 42.5) < 1e-9, `cpu tenths decode (got ${sample.cpuPercent})`)
assert(sample.memoryUsedBytes === 1024 * 1024 && sample.memoryTotalBytes === 2048 * 1024, 'memory KiB decodes to bytes')
assert(sample.diskUsedBytes === 512 * 1024 && sample.diskTotalBytes === 4096 * 1024, 'disk KiB decodes to bytes')
assert(
  sample.temperatures.length === 2 && sample.temperatures[0] === 47.5 && sample.temperatures[1] === -10.25,
  'signed centi-degree temperatures decode'
)
assert(sample.diskReadRate === 1234 && sample.diskWriteRate === 5678, 'disk rates decode')
assert(
  sample.interfaces.length === 1 &&
    sample.interfaces[0].up &&
    sample.interfaces[0].rxRate === 100 &&
    sample.interfaces[0].txRate === 200,
  'interface entry decodes'
)
