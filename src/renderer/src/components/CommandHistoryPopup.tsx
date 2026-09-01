import { useEffect, useMemo, useRef, useState } from 'react'

interface Props {
  commands: string[]
  initialFilter: string
  onClose: () => void
  onSelect: (command: string) => void
}

export default function CommandHistoryPopup({ commands, initialFilter, onClose, onSelect }: Props) {
  const [query, setQuery] = useState(initialFilter)
  const [selectedIndex, setSelectedIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  useEffect(() => {
    setSelectedIndex(0)
  }, [query])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return commands
    return commands.filter((cmd) => cmd.toLowerCase().includes(q))
  }, [commands, query])

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault()
      onClose()
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1))
      return
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      setSelectedIndex((i) => Math.max(i - 1, 0))
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      const cmd = filtered[selectedIndex]
      if (cmd) {
        onSelect(cmd)
      }
    }
  }

  return (
    <div className="command-history-popup" onKeyDown={onKeyDown}>
      <input
        ref={inputRef}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder="Search command history"
        className="command-history-input"
      />
      <div className="command-history-list">
        {filtered.map((cmd, idx) => (
          <div
            key={cmd}
            className={`command-history-item${idx === selectedIndex ? ' selected' : ''}`}
            onMouseEnter={() => setSelectedIndex(idx)}
            onClick={() => onSelect(cmd)}
          >
            {cmd}
          </div>
        ))}
        {filtered.length === 0 ? (
          <div className="command-history-empty">No matching commands</div>
        ) : null}
      </div>
    </div>
  )
}
