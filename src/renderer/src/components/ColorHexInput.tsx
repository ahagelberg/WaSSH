import { useEffect, useState } from 'react'

/** 6-digit hex, optional # prefix */
const HEX_RE = /^#?([0-9a-fA-F]{6})$/

interface Props {
  /** Effective color to display (hex); commit targets replace it */
  value: string
  onChange: (hex: string) => void
}

/**
 * Hex text field next to a color swatch. Commits complete 6-digit hex values;
 * partial input is kept while typing and reverted to `value` on blur.
 */
export default function ColorHexInput({ value, onChange }: Props) {
  const [text, setText] = useState(value)

  useEffect(() => {
    setText(value)
  }, [value])

  const handleChange = (raw: string): void => {
    const m = HEX_RE.exec(raw.trim())
    if (m) {
      const hex = `#${m[1].toLowerCase()}`
      setText(hex)
      onChange(hex)
      return
    }
    setText(raw)
  }

  return (
    <input
      type="text"
      className="settings-color-hex"
      spellCheck={false}
      value={text}
      onChange={(e) => handleChange(e.target.value)}
      onBlur={() => setText(value)}
    />
  )
}
