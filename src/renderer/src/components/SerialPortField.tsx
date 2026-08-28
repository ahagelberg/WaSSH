import { useCallback, useEffect, useRef, useState } from 'react'
import type { SerialPortInfo } from '@shared/types'

const KEY_ARROW_DOWN = 'ArrowDown'
const KEY_ARROW_UP = 'ArrowUp'
const KEY_ENTER = 'Enter'
const KEY_ESCAPE = 'Escape'

interface Props {
  id: string
  listId: string
  value: string
  disabled?: boolean
  placeholder?: string
  onChange: (value: string) => void
  onSubmit?: () => void
}

export default function SerialPortField({
  id,
  listId,
  value,
  disabled,
  placeholder,
  onChange,
  onSubmit
}: Props) {
  const [ports, setPorts] = useState<SerialPortInfo[]>([])
  const [open, setOpen] = useState(false)
  const [activeIndex, setActiveIndex] = useState(0)
  const [menuBox, setMenuBox] = useState<{ left: number; top: number; width: number } | null>(
    null
  )
  const rootRef = useRef<HTMLDivElement>(null)

  const refresh = useCallback(() => {
    const list = window.wassh.listSerialPorts
    if (typeof list !== 'function') {
      return
    }
    void list()
      .then((next) => {
        setPorts(next)
        setActiveIndex((i) => {
          if (next.length === 0) {
            return 0
          }
          return Math.min(i, next.length - 1)
        })
      })
      .catch(() => {
        setPorts([])
      })
  }, [])

  useEffect(() => {
    refresh()
  }, [refresh])

  useEffect(() => {
    if (!open) {
      return
    }
    refresh()
    const close = (ev: Event): void => {
      const target = ev.target
      if (target instanceof Node && rootRef.current?.contains(target)) {
        return
      }
      setOpen(false)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('scroll', close, true)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('scroll', close, true)
    }
  }, [open, refresh])

  const selectPort = (path: string): void => {
    onChange(path)
    setOpen(false)
  }

  const moveActive = (delta: number): void => {
    if (ports.length === 0) {
      return
    }
    setActiveIndex((i) => (i + delta + ports.length) % ports.length)
  }

  const showList = (): void => {
    if (disabled) {
      return
    }
    const rect = rootRef.current?.getBoundingClientRect()
    if (rect) {
      setMenuBox({ left: rect.left, top: rect.bottom, width: rect.width })
    }
    setOpen(true)
    setActiveIndex(0)
    refresh()
  }

  return (
    <div className={`serial-port-field${open ? ' open' : ''}`} ref={rootRef}>
      <input
        id={id}
        role="combobox"
        aria-expanded={open}
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={open && ports[activeIndex] ? `${listId}-${activeIndex}` : undefined}
        value={value}
        disabled={disabled}
        readOnly={disabled}
        placeholder={placeholder ?? 'COM3 or /dev/ttyUSB0'}
        onChange={(e) => onChange(e.target.value)}
        onFocus={refresh}
        onClick={() => {
          showList()
        }}
        onKeyDown={(e) => {
          if (e.key === KEY_ARROW_DOWN) {
            e.preventDefault()
            if (!open) {
              showList()
              return
            }
            moveActive(1)
            return
          }
          if (e.key === KEY_ARROW_UP) {
            e.preventDefault()
            if (!open) {
              showList()
              return
            }
            moveActive(-1)
            return
          }
          if (e.key === KEY_ESCAPE) {
            setOpen(false)
            return
          }
          if (e.key === KEY_ENTER) {
            if (open && ports[activeIndex]) {
              e.preventDefault()
              selectPort(ports[activeIndex].path)
              return
            }
            if (onSubmit) {
              onSubmit()
            }
          }
        }}
      />
      <button
        type="button"
        className="icon-btn serial-port-combo-toggle"
        disabled={disabled}
        aria-label="Show serial ports"
        aria-expanded={open}
        aria-controls={listId}
        tabIndex={-1}
        onClick={() => {
          showList()
        }}
      >
        ▾
      </button>
      {open ? (
        <ul
          id={listId}
          role="listbox"
          className="serial-port-combo-menu"
          style={
            menuBox
              ? { left: menuBox.left, top: menuBox.top, width: menuBox.width }
              : undefined
          }
        >
          {ports.length === 0 ? (
            <li className="serial-port-combo-empty">No ports detected — type a port name</li>
          ) : (
            ports.map((p, i) => (
              <li
                key={p.path}
                id={`${listId}-${i}`}
                role="option"
                aria-selected={p.path === value}
                className={`serial-port-combo-option${i === activeIndex ? ' active' : ''}${p.path === value ? ' selected' : ''}`}
                onMouseDown={(e) => {
                  e.preventDefault()
                  selectPort(p.path)
                }}
                onMouseEnter={() => setActiveIndex(i)}
              >
                <span className="serial-port-combo-path">{p.path}</span>
                {p.detail ? (
                  <span className="serial-port-combo-detail">{p.detail}</span>
                ) : null}
              </li>
            ))
          )}
        </ul>
      ) : null}
    </div>
  )
}
