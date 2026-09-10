import { useEffect, useState } from 'react'

const HEX_RE = /^#?([0-9a-fA-F]{6})$/

interface Props {
  value: string
  onChange: (hex: string) => void
  className?: string
}

/** Hex text field that commits complete six-digit colors. */
export default function PluginColorInput({ value, onChange, className }: Props) {
  const [text, setText] = useState(value)

  useEffect(() => {
    setText(value)
  }, [value])

  const handleChange = (raw: string): void => {
    const match = HEX_RE.exec(raw.trim())
    if (!match) {
      setText(raw)
      return
    }
    const hex = `#${match[1].toLowerCase()}`
    setText(hex)
    onChange(hex)
  }

  return (
    <input
      type="text"
      className={className}
      spellCheck={false}
      value={text}
      onChange={(event) => handleChange(event.target.value)}
      onBlur={() => setText(value)}
    />
  )
}
