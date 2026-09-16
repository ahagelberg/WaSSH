import { useEffect, type ReactNode, type Ref } from 'react'

interface Props {
  titleId: string
  title: ReactNode
  onClose: () => void
  /** Base dialog class; defaults to `settings-dialog` */
  baseClass?: string
  /** Extra class appended to the base class */
  className?: string
  dialogRef?: Ref<HTMLDivElement>
  /** Make the dialog element focusable (for programmatic focus) */
  focusable?: boolean
  /** Close when clicking the backdrop */
  closeOnBackdrop?: boolean
  /** Close on Escape */
  closeOnEscape?: boolean
  children: ReactNode
  footer?: ReactNode
}

const DIALOG_DISMISS_KEY = 'Escape'

/** Shared overlay + header (title + close button) shell for modal dialogs. */
export default function DialogShell({
  titleId,
  title,
  onClose,
  baseClass = 'settings-dialog',
  className,
  dialogRef,
  focusable = false,
  closeOnBackdrop = true,
  closeOnEscape = true,
  children,
  footer
}: Props) {
  useEffect(() => {
    if (!closeOnEscape) {
      return
    }
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key !== DIALOG_DISMISS_KEY) {
        return
      }
      e.preventDefault()
      onClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [closeOnEscape, onClose])

  return (
    <div
      className="settings-overlay"
      onMouseDown={
        closeOnBackdrop ? (e) => e.target === e.currentTarget && onClose() : undefined
      }
    >
      <div
        ref={dialogRef}
        className={className ? `${baseClass} ${className}` : baseClass}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={focusable ? -1 : undefined}
      >
        <div className="settings-dialog-header">
          <h2 id={titleId}>{title}</h2>
          <button type="button" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        {children}
        {footer ? <div className="settings-footer">{footer}</div> : null}
      </div>
    </div>
  )
}
