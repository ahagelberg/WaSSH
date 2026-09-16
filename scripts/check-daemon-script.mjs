// Extracts the remote daemon shell payload and syntax-checks it with `sh -n`.
// Run: node scripts/check-daemon-script.mjs
import { readFileSync, writeFileSync, mkdtempSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const source = readFileSync('src/plugins/builtins/server-monitor/remoteService.ts', 'utf8')
const match = /export const REMOTE_SERVICE_PAYLOAD = `([\s\S]*?)`\n/.exec(source)
if (!match) {
  console.error('Could not find REMOTE_SERVICE_PAYLOAD')
  process.exit(1)
}

// Resolve the template placeholders the build would substitute.
const constants = {}
for (const [, name, value] of source.matchAll(
  /^const ([A-Z0-9_]+) = (.+)$/gm
)) {
  constants[name] = value.replace(/^['"]|['"]$/g, '')
}
constants.REMOTE_SERVICE_VERSION = '3'
constants.REMOTE_STATE_PATH = '/var/lib/wassh-service'

const script = match[1].replace(/\$\{([A-Z0-9_]+)\}/g, (whole, name) =>
  name in constants ? constants[name] : whole
)

const dir = mkdtempSync(join(tmpdir(), 'wassh-daemon-'))
const path = join(dir, 'wassh-service')
writeFileSync(path, script)

try {
  execFileSync('sh', ['-n', path], { stdio: 'pipe' })
  console.log('daemon script: syntax OK')
} catch (error) {
  console.error('daemon script: syntax error')
  console.error(error.stderr?.toString() || error.message)
  process.exit(1)
}
