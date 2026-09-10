import type { SftpEntry } from './protocol'

const HEX_BYTES_PER_ROW = 16

export function joinPath(parent: string, name: string): string {
  if (parent === '/' || parent === '') {
    return `/${name}`
  }
  return `${parent.replace(/\/+$/, '')}/${name}`
}

export function parentPath(path: string): string | null {
  if (path === '/' || path === '') {
    return null
  }
  const trimmed = path.replace(/\/+$/, '')
  const idx = trimmed.lastIndexOf('/')
  if (idx <= 0) {
    return '/'
  }
  return trimmed.slice(0, idx)
}

export function baseName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

export function formatBytes(n: number): string {
  if (!n || n <= 0) {
    return '—'
  }
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = n
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${units[i]}`
}

export function formatDate(ms: number): string {
  if (!ms) {
    return '—'
  }
  const d = new Date(ms)
  const pad = (x: number): string => String(x).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) {
    out[i] = bin.charCodeAt(i)
  }
  return out
}

export function hexDump(bytes: Uint8Array): string {
  const lines: string[] = []
  for (let off = 0; off < bytes.length; off += HEX_BYTES_PER_ROW) {
    const hex: string[] = []
    const ascii: string[] = []
    const end = Math.min(off + HEX_BYTES_PER_ROW, bytes.length)
    for (let i = off; i < end; i++) {
      const b = bytes[i]
      hex.push(b.toString(16).padStart(2, '0'))
      ascii.push(b >= 0x20 && b <= 0x7e ? String.fromCharCode(b) : '.')
    }
    while (hex.length < HEX_BYTES_PER_ROW) {
      hex.push('  ')
    }
    const half = HEX_BYTES_PER_ROW / 2
    const hexText = `${hex.slice(0, half).join(' ')}  ${hex.slice(half).join(' ')}`
    const addr = off.toString(16).padStart(8, '0')
    lines.push(`${addr}  ${hexText}  ${ascii.join('').padEnd(HEX_BYTES_PER_ROW)}`)
  }
  return lines.join('\n')
}

export function octalMode(mode: number): string {
  return (mode & 0o777).toString(8).padStart(3, '0')
}

export function typeIcon(entry: SftpEntry): string {
  if (entry.type === 'directory') return '📁'
  if (entry.type === 'symlink') return '🔗'
  if (entry.type === 'file') return '📄'
  return '❓'
}
