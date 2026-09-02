import type { ReactNode } from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'

export interface SettingsSection {
  id: string
  title: string
  content: ReactNode
}

interface Props {
  title: string
  sections: SettingsSection[]
  onClose: () => void
  footer?: ReactNode
}

/** Offset from the scroll viewport top before a section counts as current; keep in sync with .settings-section scroll-margin-top */
const SECTION_ACTIVE_OFFSET_PX = 16

/** Keyboard key that dismisses the dialog like Cancel */
const DIALOG_DISMISS_KEY = 'Escape'

export default function SettingsDialog({ title, sections, onClose, footer }: Props) {
  const [activeId, setActiveId] = useState(sections[0]?.id ?? '')
  const contentRef = useRef<HTMLDivElement>(null)
  const dialogRef = useRef<HTMLDivElement>(null)
  const sectionRefs = useRef<Record<string, HTMLElement | null>>({})
  const onCloseRef = useRef(onClose)
  onCloseRef.current = onClose

  useEffect(() => {
    // Pull focus out of the terminal / other surfaces so ESC targets this dialog.
    dialogRef.current?.focus({ preventScroll: true })

    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key !== DIALOG_DISMISS_KEY) {
        return
      }
      e.preventDefault()
      e.stopImmediatePropagation()
      onCloseRef.current()
    }
    // Capture phase: fire before any focused element / inner handler can
    // swallow Escape, so the dialog always cancels on ESC (same as Cancel).
    window.addEventListener('keydown', onKeyDown, true)
    return () => window.removeEventListener('keydown', onKeyDown, true)
  }, [])

  const updateActiveSection = useCallback((): void => {
    const root = contentRef.current
    if (!root) {
      return
    }
    // At the bottom the last section is current even when it cannot reach the top.
    if (root.scrollTop + root.clientHeight >= root.scrollHeight - 1) {
      const last = sections[sections.length - 1]
      if (last) {
        setActiveId(last.id)
      }
      return
    }
    // The current section is the last one whose top has scrolled to the top of
    // the viewport (sections are laid out top to bottom in document order).
    const rootTop = root.getBoundingClientRect().top
    let current: string | null = null
    for (const s of sections) {
      const el = sectionRefs.current[s.id]
      if (!el) {
        continue
      }
      if (el.getBoundingClientRect().top <= rootTop + SECTION_ACTIVE_OFFSET_PX) {
        current = s.id
      } else {
        break
      }
    }
    setActiveId(current ?? sections[0]?.id ?? '')
  }, [sections])

  useEffect(() => {
    const root = contentRef.current
    if (!root) {
      return
    }
    updateActiveSection()
    root.addEventListener('scroll', updateActiveSection, { passive: true })
    // Settle after smooth programmatic scrolling finishes (final resting position).
    root.addEventListener('scrollend', updateActiveSection)
    return () => {
      root.removeEventListener('scroll', updateActiveSection)
      root.removeEventListener('scrollend', updateActiveSection)
    }
  }, [updateActiveSection])

  const scrollTo = (id: string): void => {
    sectionRefs.current[id]?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    setActiveId(id)
  }

  return (
    <div className="settings-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        ref={dialogRef}
        className="settings-dialog"
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
      >
        <div className="settings-dialog-header">
          <h2>{title}</h2>
          <button type="button" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        <div className="settings-dialog-body">
          <nav className="settings-nav">
            {sections.map((s) => (
              <button
                key={s.id}
                type="button"
                className={activeId === s.id ? 'active' : ''}
                onClick={() => scrollTo(s.id)}
              >
                {s.title}
              </button>
            ))}
          </nav>
          <div className="settings-content" ref={contentRef}>
            {sections.map((s) => (
              <section
                key={s.id}
                id={s.id}
                className="settings-section"
                ref={(el) => {
                  sectionRefs.current[s.id] = el
                }}
              >
                <h3>{s.title}</h3>
                {s.content}
              </section>
            ))}
          </div>
        </div>
        {footer ? <div className="settings-footer">{footer}</div> : null}
      </div>
    </div>
  )
}
