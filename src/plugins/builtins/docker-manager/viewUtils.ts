import type {
  DockerContainerState,
  DockerContainerView,
  DockerNetworkView,
  DockerPortView
} from './protocol'

const KIB = 1024

/** Human-readable byte size (1.5 GiB, 12 MB, ...). */
export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes <= 0) {
    return '0 B'
  }
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB']
  let value = bytes
  let unit = 0
  while (value >= KIB && unit < units.length - 1) {
    value /= KIB
    unit += 1
  }
  const rounded = unit === 0 ? Math.round(value) : Number(value.toFixed(digits))
  return `${rounded} ${units[unit]}`
}

/** Byte rate per second. */
export function formatRate(bytesPerSecond: number): string {
  return `${formatBytes(bytesPerSecond)}/s`
}

/** Short elapsed-time label ("3d 4h", "12m", "just now"). */
export function formatElapsed(timestampMs: number, now = Date.now()): string {
  if (!timestampMs) {
    return '—'
  }
  const seconds = Math.max(0, Math.floor((now - timestampMs) / 1000))
  if (seconds < 5) {
    return 'just now'
  }
  if (seconds < 60) {
    return `${seconds}s`
  }
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) {
    return `${minutes}m`
  }
  const hours = Math.floor(minutes / 60)
  if (hours < 24) {
    return `${hours}h ${minutes % 60}m`
  }
  const days = Math.floor(hours / 24)
  if (days < 30) {
    return `${days}d ${hours % 24}h`
  }
  const months = Math.floor(days / 30)
  if (months < 12) {
    return `${months}mo`
  }
  return `${Math.floor(months / 12)}y`
}

/** Format an RFC3339 timestamp from the daemon. */
export function formatTimestamp(value: string | undefined): string {
  if (!value) {
    return '—'
  }
  const parsed = Date.parse(value)
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return value
  }
  return new Date(parsed).toLocaleString()
}

/** Format a milliseconds-since-epoch value. */
export function formatEpochMs(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) {
    return '—'
  }
  return new Date(ms).toLocaleString()
}

/** Badge text for a container state. */
export function stateLabel(state: DockerContainerState): string {
  switch (state) {
    case 'running':
      return 'Running'
    case 'paused':
      return 'Paused'
    case 'restarting':
      return 'Restarting'
    case 'exited':
      return 'Exited'
    case 'created':
      return 'Created'
    case 'removing':
      return 'Removing'
    case 'dead':
      return 'Dead'
    default:
      return 'Unknown'
  }
}

/** Compact port list ("8080→80/tcp, 443/tcp"). */
export function formatPorts(ports: DockerPortView[]): string {
  if (ports.length === 0) {
    return '—'
  }
  return ports
    .slice(0, 4)
    .map((port) =>
      port.hostPort !== null
        ? `${port.hostPort}→${port.containerPort}/${port.protocol}`
        : `${port.containerPort}/${port.protocol}`
    )
    .join(', ')
    .concat(ports.length > 4 ? ` +${ports.length - 4}` : '')
}

/** Container name with the fallback used when the daemon reports none. */
export function containerLabel(container: DockerContainerView): string {
  return container.name || container.shortId || '(unnamed)'
}

export function networkLabel(network: DockerNetworkView): string {
  return network.name || network.shortId || '(unnamed)'
}

/** Order containers so running ones come first, then by name. */
export function sortContainers(containers: DockerContainerView[]): DockerContainerView[] {
  const rank = (container: DockerContainerView): number => {
    switch (container.state) {
      case 'running':
        return 0
      case 'paused':
        return 1
      case 'restarting':
        return 2
      default:
        return 3
    }
  }
  return [...containers].sort((a, b) => {
    const diff = rank(a) - rank(b)
    if (diff !== 0) {
      return diff
    }
    return containerLabel(a).localeCompare(containerLabel(b))
  })
}

/** Split a log blob into at most `limit` trailing lines. */
export function tailLines(text: string, limit: number): string {
  const lines = text.split('\n')
  if (lines.length <= limit) {
    return text
  }
  return lines.slice(lines.length - limit).join('\n')
}

/** Error message from a JSON.parse attempt, or null when valid. */
export function jsonParseError(text: string): string | null {
  const trimmed = text.trim()
  if (!trimmed) {
    return null
  }
  try {
    const parsed: unknown = JSON.parse(trimmed)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return 'Advanced JSON must be a JSON object'
    }
    return null
  } catch (error) {
    return error instanceof Error ? error.message : 'Invalid JSON'
  }
}

/** Parse a comma/space separated port list into port rows. */
export function parsePortList(value: string): Array<{ host: string; container: string; protocol: string }> {
  const out: Array<{ host: string; container: string; protocol: string }> = []
  for (const raw of value.split(',')) {
    const entry = raw.trim()
    if (!entry) {
      continue
    }
    const [mapping, protocol = 'tcp'] = entry.split('/')
    const [host, container = host] = mapping.split(':')
    out.push({ host: host.trim(), container: container.trim(), protocol: protocol.trim() || 'tcp' })
  }
  return out
}

/** Parse `KEY=value` lines into environment rows. */
export function parseEnvList(value: string): Array<{ key: string; value: string }> {
  const out: Array<{ key: string; value: string }> = []
  for (const raw of value.split('\n')) {
    const entry = raw.trim()
    if (!entry || entry.startsWith('#')) {
      continue
    }
    const index = entry.indexOf('=')
    if (index < 0) {
      out.push({ key: entry, value: '' })
    } else {
      out.push({ key: entry.slice(0, index).trim(), value: entry.slice(index + 1) })
    }
  }
  return out
}

/** One-line summary of a container's image and command. */
export function containerSubtitle(container: DockerContainerView): string {
  return container.image || container.command || '—'
}
