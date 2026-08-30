import { useEffect, useRef } from 'react'

/** Keyboard key that dismisses the find bar */
const DISMISS_KEY = 'Escape'

/** Find previous (backwards) */
const FIND_PREV_KEY = 'Enter'

/** Find previous accelerator key (also F3) */
const FIND_PREV_FKEY = 'F3'

interface Props {
  query: string
  caseSensitive: boolean
  /** Bumps to re-focus / select the input (e.g. Ctrl+F while open) */
  focusNonce: number
  /** null = idle / empty query; true/false = last search result */
  found: boolean | null
  onQueryChange: (query: string) => void
  onCaseSensitiveChange: (value: boolean) => void
  onFindPrevious: () => void
  onFindNext: () => void
  onClose: () => void
}

export default function TerminalSearchBar({
  query,
  caseSensitive,
  focusNonce,
  found,
  onQueryChange,
  onCaseSensitiveChange,
  onFindPrevious,
  onFindNext,
  onClose
}: Props) {
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const el = inputRef.current
    if (!el) {
      return
    }
    el.focus()
    el.select()
  }, [focusNonce])

  const statusText =
    query.length === 0 ? null : found === false ? 'Not found' : found === true ? 'Found' : null

  return (
    <div
      className="terminal-search-bar"
      role="search"
      onKeyDown={(e) => {
        if (e.key === DISMISS_KEY) {
          e.preventDefault()
          e.stopPropagation()
          onClose()
          return
        }
        if (e.key === FIND_PREV_FKEY) {
          e.preventDefault()
          e.stopPropagation()
          if (e.shiftKey) {
            onFindNext()
          } else {
            onFindPrevious()
          }
          return
        }
        if (e.key === FIND_PREV_KEY) {
          e.preventDefault()
          e.stopPropagation()
          if (e.shiftKey) {
            onFindNext()
          } else {
            onFindPrevious()
          }
        }
      }}
    >
      <label className="terminal-search-label">
        Find
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          placeholder="Search buffer…"
          aria-label="Search terminal buffer"
        />
      </label>
      <label className="terminal-search-case">
        <input
          type="checkbox"
          checked={caseSensitive}
          onChange={(e) => onCaseSensitiveChange(e.target.checked)}
        />
        Match case
      </label>
      <button type="button" onClick={onFindPrevious} title="Find previous (Enter)">
        Previous
      </button>
      <button type="button" onClick={onFindNext} title="Find next (Shift+Enter)">
        Next
      </button>
      {statusText ? (
        <span
          className={`terminal-search-status${found === false ? ' is-miss' : ''}`}
          aria-live="polite"
        >
          {statusText}
        </span>
      ) : null}
      <button type="button" className="terminal-search-close" onClick={onClose} aria-label="Close">
        ✕
      </button>
    </div>
  )
}
