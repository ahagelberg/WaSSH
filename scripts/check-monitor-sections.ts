// Smoke-tests the monitor sample section splitter: a process whose args
// contain `===` must not truncate the PROCS block.
// Run: npx tsx scripts/check-monitor-sections.ts
import { readFileSync } from 'fs'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

const here = dirname(fileURLToPath(import.meta.url))
const source = readFileSync(join(here, '../src/plugins/builtins/server-monitor/main.ts'), 'utf8')

function assert(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(`FAIL: ${message}`)
  }
  console.log(`ok: ${message}`)
}

// The section helpers are module-private; extract and evaluate them verbatim so
// this script exercises the real implementation rather than a copy.
const sectionNames = /const SECTION_NAMES = \[([\s\S]*?)\] as const/.exec(source)
const delimiterSrc = /const SECTION_DELIMITER = (.+)/.exec(source)
const sectionFn = /function section\(raw: string, name: string\): string \{[\s\S]*?\n\}/.exec(source)
assert(sectionNames !== null, 'SECTION_NAMES found in source')
assert(delimiterSrc !== null, 'SECTION_DELIMITER found in source')
assert(sectionFn !== null, 'section() found in source')

const SECTION_NAMES = eval(`[${sectionNames![1]}]`) as string[]
const SECTION_DELIMITER = eval(delimiterSrc![1]) as string
const section = eval(`(${sectionFn![0].replace(/: string/g, '')})`) as (
  raw: string,
  name: string
) => string

assert(SECTION_NAMES.includes('PROCS'), 'PROCS is a known section')

const sample = [
  '===META===',
  'host',
  '1.0',
  '0 0 0 0/0 0',
  'Linux 6.0 x86_64',
  '8',
  '===PROCS===',
  "1234 root S 0 5 1.2 3.4 12345 bash -c echo '===hello==='",
  '5678 root S 0 5 0.5 1.1 9999 /usr/bin/foo --bar',
  '9012 root S 0 5 0.1 0.2 5555 node server.js',
  '===TEMP===',
  'x 42'
].join('\n')

const procs = section(sample, 'PROCS')
const lines = procs.split('\n').filter(Boolean)
assert(lines.length === 3, `all three process rows kept (got ${lines.length})`)
assert(lines[0].includes("echo '===hello==='"), 'first row keeps its full args')
assert(lines[2].includes('node server.js'), 'last row is present')

const temp = section(sample, 'TEMP')
assert(temp === 'x 42', `section after PROCS still parses (got ${JSON.stringify(temp)})`)

const meta = section(sample, 'META').split('\n')
assert(meta[0] === 'host', 'META first line parses')
assert(meta[3] === 'Linux 6.0 x86_64', 'META keeps all lines')

console.log('all section checks passed')
