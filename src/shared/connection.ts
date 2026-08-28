import {
  BELL_MODE_INVERT_LINE,
  BELL_MODE_INVERT_WINDOW,
  BELL_MODE_SYSTEM,
  BUNDLED_FONT_FAMILIES,
  CURSOR_STYLE_BAR,
  CURSOR_STYLE_BLOCK,
  CURSOR_STYLE_UNDERLINE,
  DEFAULT_BELL_MODE,
  DEFAULT_CURSOR_BLINK,
  DEFAULT_CURSOR_STYLE,
  DEFAULT_FONT_FAMILY,
  DEFAULT_FONT_SIZE_PX,
  DEFAULT_SCROLLBACK_LINES,
  FONT_SIZE_MAX_PX,
  FONT_SIZE_MIN_PX,
  SCROLLBACK_LINES_MAX,
  SCROLLBACK_LINES_MIN,
  THEME_COLOR_UNSET,
  type BellMode,
  type ConnectionParams,
  type CursorStyle,
  type HostProfile
} from './types'

export interface SessionStyle {
  tabColor: string
  termBackground: string
  termForeground: string
  fontSizePx: number
  fontFamily: string
  bellMode: BellMode
  cursorStyle: CursorStyle
  cursorBlink: boolean
  scrollbackLines: number
}

/** Hex values treated as theme default (empty) */
const UNSET_TAB_COLOR_HEX = '#3d8bfd'
const UNSET_TERM_BACKGROUND_HEX = '#0c0d0f'
const UNSET_TERM_FOREGROUND_HEX = '#e8eaed'

function colorOrTheme(value: string | undefined, unsetHex: string): string {
  if (!value || value.trim().toLowerCase() === unsetHex) {
    return THEME_COLOR_UNSET
  }
  return value
}

function bellModeOrDefault(value: string | undefined): BellMode {
  if (
    value === BELL_MODE_SYSTEM ||
    value === BELL_MODE_INVERT_WINDOW ||
    value === BELL_MODE_INVERT_LINE
  ) {
    return value
  }
  return DEFAULT_BELL_MODE
}

function cursorStyleOrDefault(value: string | undefined): CursorStyle {
  if (
    value === CURSOR_STYLE_BLOCK ||
    value === CURSOR_STYLE_UNDERLINE ||
    value === CURSOR_STYLE_BAR
  ) {
    return value
  }
  return DEFAULT_CURSOR_STYLE
}

function fontFamilyOrDefault(value: string | undefined): string {
  const first = value?.split(',')[0]?.trim().replace(/^["']|["']$/g, '')
  return first || DEFAULT_FONT_FAMILY
}

function quoteFontFamily(name: string): string {
  if (name === 'monospace') {
    return name
  }
  return `"${name.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
}

export function terminalFontStack(family: string): string {
  const names = [fontFamilyOrDefault(family), ...BUNDLED_FONT_FAMILIES, 'monospace']
  const unique: string[] = []
  for (const name of names) {
    if (!unique.includes(name)) {
      unique.push(name)
    }
  }
  return unique.map(quoteFontFamily).join(', ')
}

function fontSizeOrDefault(value: number | undefined): number {
  if (typeof value !== 'number' || Number.isNaN(value)) {
    return DEFAULT_FONT_SIZE_PX
  }
  if (value < FONT_SIZE_MIN_PX) {
    return FONT_SIZE_MIN_PX
  }
  if (value > FONT_SIZE_MAX_PX) {
    return FONT_SIZE_MAX_PX
  }
  return value
}

function scrollbackLinesOrDefault(value: number | undefined): number {
  if (typeof value !== 'number' || Number.isNaN(value)) {
    return DEFAULT_SCROLLBACK_LINES
  }
  if (value < SCROLLBACK_LINES_MIN) {
    return SCROLLBACK_LINES_MIN
  }
  if (value > SCROLLBACK_LINES_MAX) {
    return SCROLLBACK_LINES_MAX
  }
  return Math.floor(value)
}

export function sessionStyleFrom(
  src: Partial<SessionStyle> | null | undefined
): SessionStyle {
  return {
    tabColor: colorOrTheme(src?.tabColor, UNSET_TAB_COLOR_HEX),
    termBackground: colorOrTheme(src?.termBackground, UNSET_TERM_BACKGROUND_HEX),
    termForeground: colorOrTheme(src?.termForeground, UNSET_TERM_FOREGROUND_HEX),
    fontSizePx: fontSizeOrDefault(src?.fontSizePx),
    fontFamily: fontFamilyOrDefault(src?.fontFamily),
    bellMode: bellModeOrDefault(src?.bellMode),
    cursorStyle: cursorStyleOrDefault(src?.cursorStyle),
    cursorBlink: typeof src?.cursorBlink === 'boolean' ? src.cursorBlink : DEFAULT_CURSOR_BLINK,
    scrollbackLines: scrollbackLinesOrDefault(src?.scrollbackLines)
  }
}

export function hostDisplayName(host: Pick<HostProfile, 'name' | 'host'>): string {
  const trimmed = (host.name ?? '').trim()
  if (trimmed) {
    return trimmed
  }
  return host.host ?? ''
}

export function hostToConnection(host: HostProfile): ConnectionParams {
  return {
    hostId: host.id,
    name: host.name,
    host: host.host,
    port: host.port,
    username: host.username,
    passwordVaultId: host.passwordVaultId,
    privateKeyPath: host.privateKeyPath,
    passphraseVaultId: host.passphraseVaultId,
    authMethod: host.authMethod,
    proxyHostId: host.proxyHostId || '',
    ...sessionStyleFrom(host),
    ephemeralPassword: '',
    ephemeralPassphrase: ''
  }
}

/** Outermost proxy first, target last */
export function resolveProxyChain(
  target: ConnectionParams,
  hosts: HostProfile[]
): ConnectionParams[] {
  const byId = new Map(hosts.map((h) => [h.id, h]))
  const hops: ConnectionParams[] = []
  const seen = new Set<string>()
  let proxyId = target.proxyHostId
  while (proxyId) {
    if (seen.has(proxyId)) {
      throw new Error('Circular proxy host reference')
    }
    seen.add(proxyId)
    const profile = byId.get(proxyId)
    if (!profile) {
      throw new Error(`Proxy host not found (${proxyId})`)
    }
    hops.unshift(hostToConnection(profile))
    proxyId = profile.proxyHostId || ''
  }
  return [...hops, target]
}

export function proxyLabel(chain: ConnectionParams[]): string | null {
  if (chain.length < 2) {
    return null
  }
  return chain
    .slice(0, -1)
    .map((h) => h.name || h.host)
    .join(' → ')
}
