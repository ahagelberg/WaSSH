import { THIRD_PARTY_LICENSES } from '@shared/licenses'
import {
  APP_AUTHOR,
  APP_DESCRIPTION,
  APP_LICENSE,
  APP_NAME,
  APP_VERSION
} from '@shared/version'
import DialogShell from './DialogShell'

interface Props {
  onClose: () => void
}

export default function AboutDialog({ onClose }: Props) {
  return (
    <DialogShell
      titleId="about-title"
      title={`About ${APP_NAME}`}
      onClose={onClose}
      baseClass="about-dialog"
      footer={
        <button type="button" className="primary" onClick={onClose}>
          Close
        </button>
      }
    >
      <div className="about-dialog-body">
        <div className="about-app">
          <div className="about-app-name">{APP_NAME}</div>
          <div className="about-app-version">Version {APP_VERSION}</div>
          <p className="about-app-desc">{APP_DESCRIPTION}</p>
          <p className="about-app-meta">By {APP_AUTHOR}</p>
          <p className="about-app-meta">License: {APP_LICENSE}</p>
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
    </DialogShell>
  )
}
