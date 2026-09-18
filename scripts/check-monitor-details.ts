// Smoke-tests the process-details parsers: /proc/PID/status, limits, and the
// `ps -o ppid=,user=,lstart=,etime=` parent line.
// Run: npx tsx scripts/check-monitor-details.ts
import {
  parseKeyValueBlock,
  parseLimits,
  parseParentLine
} from '../src/plugins/builtins/server-monitor/processDetails'

function assert(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(`FAIL: ${message}`)
  }
  console.log(`ok: ${message}`)
}

// --- /proc/PID/status -----------------------------------------------------
const status = parseKeyValueBlock(
  [
    'Name:\tbash',
    'State:\tS (sleeping)',
    'Tgid:\t1234',
    'Pid:\t1234',
    'PPid:\t1',
    'Uid:\t1000\t1000\t1000\t1000',
    'Gid:\t1000\t1000\t1000\t1000',
    'Threads:\t1',
    'VmPeak:\t  123456 kB',
    'VmSize:\t  120000 kB',
    'VmRSS:\t    4321 kB',
    'voluntary_ctxt_switches:\t42',
    'nonvoluntary_ctxt_switches:\t3'
  ].join('\n')
)
assert(status.get('State') === 'S (sleeping)', 'status State parsed')
assert(status.get('Threads') === '1', 'status Threads parsed')
assert(status.get('VmRSS') === '4321 kB', 'status VmRSS keeps its units')
assert(status.get('voluntary_ctxt_switches') === '42', 'status ctx switches parsed')
assert(status.get('Uid') === '1000\t1000\t1000\t1000', 'status Uid keeps all four values')

// --- ps parent line -------------------------------------------------------
const parent = parseParentLine('1 root Wed Sep 17 10:11:12 2025 01:23:45')
assert(parent.ppid === '1', `parent ppid parsed (got ${parent.ppid})`)
assert(parent.user === 'root', `parent user parsed (got ${parent.user})`)
assert(
  parent.started === 'Wed Sep 17 10:11:12 2025',
  `parent start time parsed (got ${JSON.stringify(parent.started)})`
)
assert(parent.elapsed === '01:23:45', `parent elapsed parsed (got ${parent.elapsed})`)
assert(parseParentLine('').ppid === '', 'empty parent line is safe')
assert(parseParentLine('   ').user === '', 'blank parent line is safe')

// --- /proc/PID/limits -----------------------------------------------------
const limits = parseLimits(
  [
    'Limit                     Soft Limit           Hard Limit           Units',
    'Max cpu time              unlimited            unlimited            seconds',
    'Max open files            1024                 1048576              files',
    'Max processes             63000                63000                processes',
    'Max address space         unlimited            unlimited            bytes',
    'Max stack size            8388608              unlimited            bytes'
  ].join('\n')
)
const byLabel = new Map(limits)
assert(
  byLabel.get('Max open files') === '1024 files',
  `limits soft value (got ${byLabel.get('Max open files')})`
)
assert(byLabel.get('Max processes') === '63000 processes', 'limits processes parsed')
assert(byLabel.get('Max stack size') === '8388608 bytes', 'limits stack parsed')
assert(!byLabel.has('Max cpu time'), 'unwanted limits are dropped')

console.log('all process-details checks passed')
