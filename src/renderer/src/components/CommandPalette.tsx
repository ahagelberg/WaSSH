import { useEffect, useMemo, useRef, useState } from 'react'
import type { CommandDefinition } from '../commands'

interface Props {
  commands: CommandDefinition[]
  onClose: () => void
  onExecute: (commandId: string, args: string) => void
}

export default function CommandPalette({ commands, onClose, onExecute }: Props) {
  const [query, setQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(0)
  const [argMode, setArgMode] = useState(false)
  const [selectedCommand, setSelectedCommand] = useState<CommandDefinition | null>(null)
  const [argValue, setArgValue] = useState('')

  const queryRef = useRef<HTMLInputElement>(null)
  const argRef = useRef<HTMLInputElement>(null)

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return commands
    return commands.filter((cmd) => {
      const title = cmd.title.toLowerCase()
      if (title.includes(q)) return true
      let i = 0
      for (const ch of title) {
        if (ch === q[i]) i++
        if (i === q.length) return true
      }
      return false
    })
  }, [commands, query])

  useEffect(() => {
    setSelectedIndex(0)
  }, [query])

  useEffect(() => {
    if (argMode) argRef.current?.focus()
    else queryRef.current?.focus()
  }, [argMode])

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        if (argMode) {
          setArgMode(false)
          setSelectedCommand(null)
          setArgValue('')
        } else {
          onClose()
        }
        return
      }

      if (argMode) return

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
          if (cmd.needsArgument) {
            setSelectedCommand(cmd)
            setArgMode(true)
            setArgValue('')
          } else {
            onExecute(cmd.id, '')
          }
        }
      }
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [filtered, selectedIndex, argMode, onClose, onExecute])

  const submitArg = (e: React.FormEvent) => {
    e.preventDefault()
    if (!selectedCommand) return
    onExecute(selectedCommand.id, argValue)
  }

  const pickCommand = (cmd: CommandDefinition, index: number) => {
    setSelectedIndex(index)
    if (cmd.needsArgument) {
      setSelectedCommand(cmd)
      setArgMode(true)
      setArgValue('')
    } else {
      onExecute(cmd.id, '')
    }
  }

  return (
    <div className="command-palette-overlay" onClick={onClose}>
      <div className="command-palette" onClick={(e) => e.stopPropagation()}>
        {!argMode ? (
          <>
            <div className="command-palette-search">
              <input
                ref={queryRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Type a command…"
                className="command-palette-input"
              />
            </div>
            <div className="command-palette-list">
              {filtered.map((cmd, idx) => (
                <div
                  key={cmd.id}
                  onClick={() => pickCommand(cmd, idx)}
                  onMouseEnter={() => setSelectedIndex(idx)}
                  className={`command-palette-item${idx === selectedIndex ? ' selected' : ''}`}
                >
                  {cmd.title}
                  {cmd.needsArgument ? (
                    <span className="command-palette-requires-arg">…</span>
                  ) : null}
                </div>
              ))}
              {filtered.length === 0 ? (
                <div className="command-palette-empty">No matching commands</div>
              ) : null}
            </div>
          </>
        ) : (
          <form onSubmit={submitArg} className="command-palette-arg-form">
            <span className="command-palette-arg-label">{selectedCommand?.title}</span>
            <input
              ref={argRef}
              value={argValue}
              onChange={(e) => setArgValue(e.target.value)}
              placeholder={selectedCommand?.placeholder ?? 'Enter value…'}
              autoFocus
              className="command-palette-arg-input"
            />
          </form>
        )}
      </div>
    </div>
  )
}
