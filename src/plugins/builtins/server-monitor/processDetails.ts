/**
 * Pure parsers for the process-details dialog. Kept separate from `main.ts` so
 * they can be exercised without the plugin host.
 */

/** Parse a `/proc/PID/status`-style `Key:\tValue` block */
export function parseKeyValueBlock(block: string): Map<string, string> {
  const out = new Map<string, string>()
  for (const line of block.split(/\n/)) {
    const idx = line.indexOf(':')
    if (idx <= 0) {
      continue
    }
    out.set(line.slice(0, idx).trim(), line.slice(idx + 1).trim())
  }
  return out
}

/** Parent/owner/start columns from `ps -o ppid=,user=,lstart=,etime=` */
export interface ParentInfo {
  ppid: string
  user: string
  started: string
  elapsed: string
}

export function parseParentLine(line: string): ParentInfo {
  const trimmed = line.trim()
  if (!trimmed) {
    return { ppid: '', user: '', started: '', elapsed: '' }
  }
  // ppid and user are single tokens; lstart is 5 (`Wed Sep 17 10:11:12 2025`)
  // and etime is the trailing token.
  const parts = trimmed.split(/\s+/)
  if (parts.length < 7) {
    return { ppid: parts[0] ?? '', user: parts[1] ?? '', started: '', elapsed: '' }
  }
  return {
    ppid: parts[0],
    user: parts[1],
    started: parts.slice(2, parts.length - 1).join(' '),
    elapsed: parts[parts.length - 1]
  }
}

/** `limits` rows shown in the details dialog */
const WANTED_LIMITS = new Set([
  'Max open files',
  'Max processes',
  'Max locked memory',
  'Max address space',
  'Max file size',
  'Max stack size'
])

/** Extract the interesting `limits` rows as `label → value` pairs */
export function parseLimits(block: string): Array<[string, string]> {
  const out: Array<[string, string]> = []
  for (const line of block.split(/\n/)) {
    const idx = line.indexOf('  ')
    if (idx <= 0) {
      continue
    }
    const label = line.slice(0, idx).trim()
    if (!WANTED_LIMITS.has(label)) {
      continue
    }
    // "soft  hard  units" → keep the soft value and units only.
    const parts = line.slice(idx).trim().split(/\s+/)
    out.push([label, parts.slice(0, 1).concat(parts.slice(2)).join(' ')])
  }
  return out
}
