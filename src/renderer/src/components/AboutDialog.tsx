import { useEffect } from 'react'
import { THIRD_PARTY_LICENSES } from '@shared/licenses'
import {
  APP_AUTHOR,
  APP_DESCRIPTION,
  APP_LICENSE,
  APP_NAME,
  APP_VERSION
} from '@shared/version'

interface Props {
  onClose: () => void
}

/** Keyboard key that dismisses the dialog */
const DIALOG_DISMISS_KEY = 'Escape'

export default function AboutDialog({ onClose }: Props) {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key !== DIALOG_DISMISS_KEY) {
        return
      }
      e.preventDefault()
      onClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  return (
    <div className="settings-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="about-dialog" role="dialog" aria-modal="true" aria-labelledby="about-title">
        <div className="settings-dialog-header">
          <h2 id="about-title">About {APP_NAME}</h2>
          <button type="button" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        <div className="about-dialog-body">
          <div className="about-app">
            <div className="about-app-name">{APP_NAME}</div>
            <div className="about-app-version">Version {APP_VERSION}</div>
            <p className="about-app-desc">{APP_DESCRIPTION}</p>
            <p className="about-app-meta">
              License: {APP_LICENSE}
              {APP_AUTHOR ? ` · ${APP_AUTHOR}` : null}
            </p>
          </div>
          <section className="about-licenses">
            <h3>Third-party licenses</h3>
            <p className="about-licenses-intro">
              WaSSH includes the following open-source libraries:
            </p>
            <ul className="about-license-list">
              {THIRD_PARTY_LICENSES.map((lib) => (
                <li key={lib.name}>
                  <span className="about-license-name">{lib.name}</span>
                  <span className="about-license-spdx">{lib.license}</span>
                  {lib.note ? <span className="about-license-note">{lib.note}</span> : null}
                </li>
              ))}
            </ul>
          </section>
        </div>
        <div className="settings-footer">
          <button type="button" className="primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  )
}
