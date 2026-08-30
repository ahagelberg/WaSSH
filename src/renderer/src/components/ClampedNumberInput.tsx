import { useEffect, useRef, useState, type ReactElement } from 'react'

interface Props {
  value: number
  min: number
  max: number
  disabled?: boolean
  /** When true, commit floors to an integer */
  integer?: boolean
  onCommit: (value: number) => void
}

function clampNumber(n: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, n))
}

/**
 * Number field that allows empty/partial text while editing; clamps and commits on blur/Enter.
 * Avoids controlled min/max snapping that blocks replacing the value by typing.
 */
export default function ClampedNumberInput({
  value,
  min,
  max,
  disabled,
  integer,
  onCommit
}: Props): ReactElement {
  const [text, setText] = useState(String(value))
  const focusedRef = useRef(false)

  useEffect(() => {
    if (!focusedRef.current) {
      setText(String(value))
    }
  }, [value])

  const commit = (raw: string): void => {
    const parsed = Number(raw)
    if (!Number.isFinite(parsed)) {
      setText(String(value))
      return
    }
    const next = clampNumber(integer ? Math.floor(parsed) : parsed, min, max)
    setText(String(next))
    if (next !== value) {
      onCommit(next)
    }
  }

  return (
    <input
      type="number"
      min={min}
      max={max}
      disabled={disabled}
      value={text}
      onFocus={() => {
        focusedRef.current = true
      }}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => {
        focusedRef.current = false
        commit(text)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.currentTarget.blur()
        }
      }}
    />
  )
}
