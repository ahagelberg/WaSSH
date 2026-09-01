import type { CSSProperties } from 'react'

interface SessionStyleCss {
  tabColor?: string
  termBackground?: string
  termForeground?: string
}

function sessionStyleCss(style: SessionStyleCss): CSSProperties | undefined {
  const css: Record<string, string> = {}
  if (style.tabColor) {
    css['--tab-accent-color'] = style.tabColor
  }
  if (style.termBackground) {
    css['--term-bg-color'] = style.termBackground
  }
  if (style.termForeground) {
    css['--term-fg-color'] = style.termForeground
  }
  if (Object.keys(css).length === 0) {
    return undefined
  }
  return css as CSSProperties
}

/** Bridge resolved tab accent hex into CSS via --tab-accent-color */
export function sessionAccentStyle(tabColor: string): CSSProperties | undefined {
  return sessionStyleCss({ tabColor })
}

/** Bridge resolved terminal colors into CSS via --term-bg-color / --term-fg-color */
export function sessionTerminalStyle(
  termBackground: string,
  termForeground: string
): CSSProperties | undefined {
  return sessionStyleCss({ termBackground, termForeground })
}
