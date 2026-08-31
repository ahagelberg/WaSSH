import { useRef, useState, type KeyboardEvent } from 'react'

interface Props {
  value: string[]
  onChange: (tags: string[]) => void
  id?: string
  placeholder?: string
  disabled?: boolean
}

/**
 * Standard tag input: type a word and press Enter or comma to turn it into a
 * tag blob; each blob has an ✕ to remove it. Backspace on an empty field
 * removes the last tag, and blurring commits the pending text.
 */
export default function TagInput({
  value,
  onChange,
  id,
  placeholder,
  disabled
}: Props) {
  const [input, setInput] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  const commit = (): void => {
    const raw = input.trim()
    setInput('')
    if (!raw || disabled) {
      return
    }
    const exists = value.some((tag) => tag.toLowerCase() === raw.toLowerCase())
    if (exists) {
      return
    }
    onChange([...value, raw])
  }

  const removeTag = (tag: string): void => {
    if (disabled) {
      return
    }
    onChange(value.filter((t) => t !== tag))
  }

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>): void => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault()
      commit()
      return
    }
    if (e.key === 'Backspace' && !input && value.length > 0) {
      onChange(value.slice(0, -1))
    }
  }

  return (
    <div className="tag-input" onClick={() => inputRef.current?.focus()}>
      {value.map((tag) => (
        <span key={tag} className="tag-blob">
          {tag}
          <button
            type="button"
            className="tag-remove"
            aria-label={`Remove tag ${tag}`}
            disabled={disabled}
            onClick={(e) => {
              e.stopPropagation()
              removeTag(tag)
            }}
          >
            ✕
          </button>
        </span>
      ))}
      <input
        ref={inputRef}
        id={id}
        type="text"
        className="tag-input-field"
        value={input}
        disabled={disabled}
        placeholder={placeholder}
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={commit}
      />
    </div>
  )
}
