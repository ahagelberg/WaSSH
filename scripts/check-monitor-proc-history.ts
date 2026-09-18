// Smoke-tests per-process CPU history windowing: samples pushed at 1 Hz must
// come back bucketed to the panel's tier widths and clipped to the window.
// Run: npx tsx scripts/check-monitor-proc-history.ts
import { MonitorLadder } from '../src/shared/monitorLadder'
import { SERIES_PERCENT_MAX } from '../src/shared/monitorSeries'

const SERIES_ID = 'proc:1234'

function assert(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(`FAIL: ${message}`)
  }
  console.log(`ok: ${message}`)
}

/** Same construction as `recordProcessHistory()` in main.ts */
function makeLadder(): MonitorLadder {
  return new MonitorLadder([
    { id: SERIES_ID, label: 'proc', unit: '%', kind: 'gauge', max: SERIES_PERCENT_MAX }
  ])
}

/** Same read as `processHistoryWindow()` in main.ts */
function window(
  ladder: MonitorLadder,
  toMs: number,
  windowSeconds: number,
  bucketSeconds: number
): Array<{ at: number; cpuPercent: number }> {
  const fromMs = toMs - windowSeconds * 1000
  const snapshot = ladder.buckets(bucketSeconds, fromMs, toMs)
  return (snapshot?.series[0]?.buckets ?? []).map((b) => ({ at: b.timestamp, cpuPercent: b.avg }))
}

// 20 minutes of 1 Hz samples: value = second index % 100.
const start = 1_700_000_000_000
const ladder = makeLadder()
for (let i = 0; i < 1200; i += 1) {
  ladder.push(start + i * 1000, { [SERIES_ID]: i % 100 })
}
const end = start + 1199 * 1000

// 1 m window at the fast tier (1 s buckets) → ~60 points.
const fast = window(ladder, end, 60, 1)
assert(fast.length >= 55 && fast.length <= 61, `1m/1s window has ~60 points (got ${fast.length})`)
assert(
  fast[fast.length - 1].at >= end - 1000,
  'fast window includes the newest bucket'
)

// 10 m window at the medium tier (15 s buckets) → ~40 points, not 600.
const medium = window(ladder, end, 600, 15)
assert(medium.length >= 35 && medium.length <= 41, `10m/15s window has ~40 points (got ${medium.length})`)
assert(
  medium.every((point) => point.at >= end - 600_000),
  'medium window is clipped to the requested range'
)
assert(
  medium.every((point) => point.cpuPercent >= 0 && point.cpuPercent <= 100),
  'bucketed averages stay in range'
)

// A window longer than the retained history still returns what exists.
const wide = window(ladder, end, 7 * 24 * 3600, 600)
assert(wide.length > 0, 'wide window returns the retained buckets')

// An empty ladder yields no points rather than throwing.
assert(window(makeLadder(), end, 600, 15).length === 0, 'empty ladder returns no points')

console.log('all process-history checks passed')
