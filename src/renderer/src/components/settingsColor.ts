/** CSS variable used as the tab custom-color starting sample */
export const TAB_COLOR_THEME_VAR = '--accent'
/** CSS variable for themed terminal background */
export const TERM_BG_THEME_VAR = '--bg-term'
/** Default terminal text color: fixed, not theme-dependent */
export const TERM_FG_THEME_VAR = '--term-fg-default'

/** Hex digits per RGB channel */
const HEX_CHANNEL_DIGITS = 2
export const HEX_COLOR_RE = /^#[0-9a-fA-F]{6}$/

/** Resolve a CSS custom property to a six-digit hex, converting `rgb()`/`rgba()` when needed. */
export function themeVarHex(cssVar: string): string {
  const raw = getComputedStyle(document.documentElement).getPropertyValue(cssVar).trim()
  if (HEX_COLOR_RE.test(raw)) {
    return raw
  }
  const rgb = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/.exec(raw)
  if (!rgb) {
    return raw
  }
  return `#${[rgb[1], rgb[2], rgb[3]]
    .map((n) => Number(n).toString(16).padStart(HEX_CHANNEL_DIGITS, '0'))
    .join('')}`
}
