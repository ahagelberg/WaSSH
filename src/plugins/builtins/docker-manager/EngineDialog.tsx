import { type ReactElement } from 'react'
import { PluginButton } from '@plugin-api/renderer'
import { formatTimestamp } from './viewUtils'
import type { DockerEngineStatus } from './protocol'

interface Props {
  status: DockerEngineStatus | null
  busy: boolean
  lines: string[]
  updatesAt: number
  staleCount: number
  checkedCount: number
  updateErrors: Array<{ image: string; error: string }>
  onUpdateEngine: () => void
  onCheckUpdates: () => void
  onClose: () => void
}

/**
 * Engine overview: version details, update state, and the privileged engine
 * upgrade action. Reuses the install log stream for upgrade output.
 */
export default function EngineDialog({
  status,
  busy,
  lines,
  updatesAt,
  staleCount,
  checkedCount,
  updateErrors,
  onUpdateEngine,
  onCheckUpdates,
  onClose
}: Props): ReactElement {
  return (
    <div className="plugin-ui-modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className="plugin-ui-modal docker-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Docker engine"
        onClick={(event) => event.stopPropagation()}
      >
        <h2 className="plugin-ui-heading">Docker engine</h2>

        <dl className="docker-kv">
          <Row label="Status" value={status?.state ?? 'unknown'} />
          <Row label="Version" value={status?.version ?? '—'} mono />
          <Row label="API version" value={status?.apiVersion ?? '—'} mono />
          <Row label="Platform" value={status ? `${status.os ?? '—'} / ${status.arch ?? '—'}` : '—'} mono />
          <Row label="Socket" value={status?.socketPath ?? '—'} mono />
          <Row label="Containers" value={containerSummary(status)} />
          <Row label="Images" value={status?.images === undefined ? '—' : String(status.images)} />
          <Row
            label="Updates checked"
            value={updatesAt ? formatTimestamp(new Date(updatesAt).toISOString()) : 'not checked yet'}
          />
          <Row
            label="Out of date"
            value={
              staleCount > 0
                ? `${staleCount} container${staleCount === 1 ? '' : 's'}`
                : checkedCount > 0
                  ? 'none'
                  : '—'
            }
          />
        </dl>

        {updateErrors.length > 0 ? (
          <details className="docker-raw">
            <summary>{updateErrors.length} image(s) could not be checked</summary>
            <pre className="docker-kv-pre">
              {updateErrors.map((entry) => `${entry.image}: ${entry.error}`).join('\n')}
            </pre>
          </details>
        ) : null}

        <p className="plugin-ui-hint">
          Updating the engine runs the package manager for the detected distribution through sudo. The daemon
          restarts, so running containers may be briefly interrupted.
        </p>

        {lines.length > 0 ? <pre className="docker-install-log">{lines.slice(-40).join('\n')}</pre> : null}

        <div className="plugin-ui-actions docker-modal-actions">
          <PluginButton compact onClick={onCheckUpdates} disabled={busy}>
            Check container updates
          </PluginButton>
          <div className="plugin-ui-spacer" />
          <PluginButton compact onClick={onClose}>
            Close
          </PluginButton>
          <PluginButton variant="primary" compact disabled={busy} onClick={onUpdateEngine}>
            {busy ? 'Updating…' : 'Update engine'}
          </PluginButton>
        </div>
      </div>
    </div>
  )
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }): ReactElement {
  return (
    <div className="docker-kv-row">
      <dt className="plugin-ui-muted">{label}</dt>
      <dd className={mono ? 'docker-mono' : undefined}>{value}</dd>
    </div>
  )
}

function containerSummary(status: DockerEngineStatus | null): string {
  if (!status || status.containersRunning === undefined) {
    return '—'
  }
  const parts = [`${status.containersRunning} running`]
  if (status.containersPaused) {
    parts.push(`${status.containersPaused} paused`)
  }
  if (status.containersStopped) {
    parts.push(`${status.containersStopped} stopped`)
  }
  return parts.join(', ')
}
