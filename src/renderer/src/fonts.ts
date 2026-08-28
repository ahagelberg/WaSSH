import { BUNDLED_FONT_FAMILIES } from '@shared/types'

const MONO_PROBE_SIZE_PX = 16
const MONO_PROBE_NARROW = 'iii'
const MONO_PROBE_WIDE = 'WWW'

const GENERIC_FONT_FAMILIES = new Set([
  'cursive',
  'emoji',
  'fangsong',
  'fantasy',
  'math',
  'monospace',
  'sans-serif',
  'serif',
  'system-ui',
  'ui-monospace',
  'ui-sans-serif',
  'ui-serif'
])

function familyIsMonospace(family: string): boolean {
  const canvas = document.createElement('canvas')
  const ctx = canvas.getContext('2d')
  if (!ctx) {
    return false
  }
  ctx.font = `${MONO_PROBE_SIZE_PX}px "${family.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
  return ctx.measureText(MONO_PROBE_NARROW).width === ctx.measureText(MONO_PROBE_WIDE).width
}

async function installedFontFamilies(): Promise<string[] | null> {
  const query = window.queryLocalFonts
  if (typeof query !== 'function') {
    return null
  }
  try {
    const fonts = await query()
    const families = new Set<string>()
    for (const font of fonts) {
      const family = font.family?.trim()
      if (!family || GENERIC_FONT_FAMILIES.has(family.toLowerCase())) {
        continue
      }
      families.add(family)
    }
    return Array.from(families)
  } catch {
    return null
  }
}

export async function listMonospaceFontFamilies(): Promise<string[]> {
  const bundled: string[] = Array.from(BUNDLED_FONT_FAMILIES)
  const installed = await installedFontFamilies()
  if (!installed) {
    return bundled
  }
  const names = new Set<string>(bundled)
  for (const family of installed) {
    if (familyIsMonospace(family)) {
      names.add(family)
    }
  }
  return Array.from(names).sort((a, b) => a.localeCompare(b))
}

export function fontSelectOptions(available: string[], current: string): string[] {
  const names = new Set<string>(available)
  if (current) {
    names.add(current)
  }
  return Array.from(names).sort((a, b) => a.localeCompare(b))
}
