// Smoke-tests the monitor ladder rollups and the binary frame codec.
// Run: npx tsx scripts/check-monitor-ladder.ts
import { MonitorLadder } from '../src/shared/monitorLadder'
import {
  MonitorFrameDecoder,
  encodeHeader,
  encodeSampleFrame,
  type MonitorSample
} from '../src/shared/monitorFrames'
import { SERIES_CPU, type MonitorSeries } from '../src/shared/monitorSeries'

const series: MonitorSeries[] = [
  { id: SERIES_CPU, label: 'CPU', unit: '%', kind: 'gauge', max: 100 }
]

function assert(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(`FAIL: ${message}`)
  }
  console.log(`ok: ${message}`)
}

// --- ladder rollups ------------------------------------------------------
const ladder = new MonitorLadder(series)
const start = 1_700_000_000_000
// 600 samples at 1 Hz: value = index % 100 so peaks land in known buckets.
for (let i = 0; i < 600; i += 1) {
  ladder.push(start + i * 1000, { [SERIES_CPU]: i % 100 })
}

const fast = ladder.buckets(1, start, start + 600_000)
assert(fast !== null, 'fast tier returned')
const fastPoints = fast!.series[0].buckets
assert(fastPoints.length === 300, `fast tier retains 300 buckets (got ${fastPoints.length})`)
assert(
  fastPoints[fastPoints.length - 1].timestamp === start + 599_000,
  'fast tier holds the newest bucket'
)

const medium = ladder.buckets(15, start, start + 600_000)
const mediumPoints = medium!.series[0].buckets
assert(mediumPoints.length === 40, `medium tier covers 600 s in 40 buckets (got ${mediumPoints.length})`)

// Buckets align to wall-clock boundaries, so the first 15 s bucket holds
// samples 10..24 -> values 10..24, i.e. min 10 / max 24 / avg 17.
const first = mediumPoints[0]
assert(first.min === 10 && first.max === 24, `medium bucket min/max preserved (${first.min}/${first.max})`)
assert(Math.abs(first.avg - 17) < 0.001, `medium bucket avg is 17 (got ${first.avg})`)

const slow = ladder.buckets(60, start, start + 600_000)!.series[0].buckets
assert(slow.length === 10, `slow tier covers 600 s in 10 buckets (got ${slow.length})`)

// --- frame codec ---------------------------------------------------------
const header = {
  series,
  temperatureZones: ['x86_pkg_temp', 'acpitz'],
  interfaces: ['eth0']
}
const decoder = new MonitorFrameDecoder()
const sample: MonitorSample = {
  timestamp: start,
  cpuPercent: 42.5,
  memoryUsedBytes: 8 * 1024 ** 3,
  memoryTotalBytes: 16 * 1024 ** 3,
  diskUsedBytes: 100 * 1024 ** 3,
  diskTotalBytes: 500 * 1024 ** 3,
  diskReadRate: 123456,
  diskWriteRate: 654321,
  temperatures: [47.5, -10.25],
  interfaces: [{ up: true, rxRate: 1000, txRate: 2000 }]
}

const bytes = new Uint8Array([...encodeHeader(header), ...encodeSampleFrame(sample)])
// Split mid-frame to exercise the incremental decoder.
const frames = [...decoder.push(bytes.slice(0, 11)), ...decoder.push(bytes.slice(11))]
assert(frames.length === 2, `decoder emitted 2 frames (got ${frames.length})`)

const decodedHeader = frames[0]
assert(decodedHeader.kind === 1, 'first frame is the header')
if (decodedHeader.kind === 1) {
  assert(decodedHeader.header.interfaces[0] === 'eth0', 'header carries interface names')
  assert(decodedHeader.header.temperatureZones.length === 2, 'header carries thermal zones')
}

const decodedSample = frames[1]
assert(decodedSample.kind === 2, 'second frame is a sample')
if (decodedSample.kind === 2) {
  const s = decodedSample.sample
  assert(s.cpuPercent === 42.5, `cpu round-trips (${s.cpuPercent})`)
  assert(s.memoryUsedBytes === 8 * 1024 ** 3, `memory round-trips (${s.memoryUsedBytes})`)
  assert(s.diskReadRate === 123456, `disk read rate round-trips (${s.diskReadRate})`)
  assert(s.temperatures[0] === 47.5, `temperature round-trips (${s.temperatures[0]})`)
  assert(s.temperatures[1] === -10.25, `negative temperature round-trips (${s.temperatures[1]})`)
  assert(s.interfaces[0].up && s.interfaces[0].txRate === 2000, 'interface round-trips')
}

// --- empty tails ---------------------------------------------------------
const emptyDecoder = new MonitorFrameDecoder()
const sparse: MonitorSample = { ...sample, temperatures: [], interfaces: [] }
const sparseFrames = emptyDecoder.push(encodeSampleFrame(sparse))
assert(
  sparseFrames.length === 1 &&
    sparseFrames[0].kind === 2 &&
    sparseFrames[0].sample.temperatures.length === 0,
  'zero-length tails decode'
)

// --- interface state flapping -------------------------------------------
// An up -> down -> up flip inside one bucket must keep min 0 and max 1 so the
// renderer can mark the bucket as flapping instead of losing the transition.
const stateLadder = new MonitorLadder([
  { id: 'iface:eth0:state', label: 'eth0 state', unit: '', kind: 'state', max: 1 }
])
const stateStart = 1_700_000_000_000
for (const value of [1, 1, 0, 1, 1]) {
  stateLadder.push(stateStart, { 'iface:eth0:state': value })
}
const stateBucket = stateLadder.buckets(1, stateStart, stateStart)!.series[0].buckets[0]
assert(stateBucket.min === 0 && stateBucket.max === 1, 'flapping bucket keeps min 0 / max 1')
assert(Math.abs(stateBucket.avg - 0.8) < 0.001, `flapping bucket avg is 0.8 (got ${stateBucket.avg})`)

console.log('\nall checks passed')
