import type { ReactNode } from 'react'
import { useEffect, useRef, useState } from 'react'

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

/** Intersection ratio to consider a section active */
const SECTION_VISIBLE_RATIO = 0.35

export default function SettingsDialog({ title, sections, onClose, footer }: Props) {
  const [activeId, setActiveId] = useState(sections[0]?.id ?? '')
  const contentRef = useRef<HTMLDivElement>(null)
  const sectionRefs = useRef<Record<string, HTMLElement | null>>({})

  useEffect(() => {
    const root = contentRef.current
    if (!root) {
      return
    }
    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((e) => e.isIntersecting)
          .sort((a, b) => b.intersectionRatio - a.intersectionRatio)
        if (visible[0]?.target.id) {
          setActiveId(visible[0].target.id)
        }
      },
      { root, threshold: [SECTION_VISIBLE_RATIO, 0.5, 0.75] }
    )
    for (const section of sections) {
      const el = sectionRefs.current[section.id]
      if (el) {
        observer.observe(el)
      }
    }
    return () => observer.disconnect()
  }, [sections])

  const scrollTo = (id: string): void => {
    sectionRefs.current[id]?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    setActiveId(id)
  }

  return (
    <div className="settings-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="settings-dialog" role="dialog" aria-modal="true">
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
