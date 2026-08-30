import type { ISearchOptions } from '@xterm/addon-search'

/** Controller each TerminalView registers for the active-tab find bar */
export interface TerminalSearchController {
  /** Search backwards; pass fromEnd to start at the bottom of the buffer */
  findPrevious: (term: string, options: TerminalSearchRunOptions) => boolean
  findNext: (term: string, options: TerminalSearchRunOptions) => boolean
  clear: () => void
}

export interface TerminalSearchRunOptions {
  caseSensitive: boolean
  /** Clear selection/decorations so search starts at buffer end */
  fromEnd: boolean
}

export function toXtermSearchOptions(
  caseSensitive: boolean
): ISearchOptions {
  return {
    caseSensitive,
    decorations: {
      matchBackground: SEARCH_MATCH_BACKGROUND,
      matchOverviewRuler: SEARCH_MATCH_RULER,
      activeMatchBackground: SEARCH_ACTIVE_BACKGROUND,
      activeMatchColorOverviewRuler: SEARCH_ACTIVE_RULER
    }
  }
}

/** Inactive match highlight (#RRGGBB) */
const SEARCH_MATCH_BACKGROUND = '#3d5a80'

/** Overview-ruler tick for inactive matches */
const SEARCH_MATCH_RULER = '#3d8bfd'

/** Active match highlight */
const SEARCH_ACTIVE_BACKGROUND = '#f0b429'

/** Overview-ruler tick for the active match */
const SEARCH_ACTIVE_RULER = '#f0b429'
