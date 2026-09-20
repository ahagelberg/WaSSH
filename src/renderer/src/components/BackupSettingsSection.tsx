import { useState, type ReactElement } from 'react'
import { APP_VERSION } from '@shared/version'
import type { BackupImportResult } from '@shared/backup'

/** Outcome banner shown after an export or restore attempt */
interface BackupStatus {
  kind: 'ok' | 'error' | 'warn'
  message: string
}

/** Format a manifest timestamp for display */
function formatCreatedAt(ms: number): string {
  try {
    return new Date(ms).toLocaleString()
  } catch {
    return String(ms)
  }
}

/**
 * Settings → Backup: export every config file to one zip, or restore from a
 * previously exported zip. Restoring replaces all config and reloads the app.
 */
export default function BackupSettingsSection(): ReactElement {
  const [status, setStatus] = useState<BackupStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [pending, setPending] = useState<{ path: string; info: BackupImportResult } | null>(null)

  const runExport = async (): Promise<void> => {
    setBusy(true)
    setStatus(null)
    try {
      const result = await window.wassh.exportBackup()
      if (result.cancelled) {
        return
      }
      if (!result.ok) {
        setStatus({ kind: 'error', message: result.error ?? 'Export failed' })
        return
      }
      setStatus({
        kind: 'ok',
        message: `Exported ${result.entryCount ?? 0} config file${
          result.entryCount === 1 ? '' : 's'
        } to ${result.path}`
      })
    } finally {
      setBusy(false)
    }
  }

  const runPick = async (): Promise<void> => {
    setBusy(true)
    setStatus(null)
    setPending(null)
    try {
      const path = await window.wassh.pickBackupFile()
      if (!path) {
        return
      }
      const info = await window.wassh.inspectBackup(path)
      if (!info.ok) {
        setStatus({ kind: 'error', message: info.error ?? 'Invalid backup archive' })
        return
      }
      setPending({ path, info })
    } finally {
      setBusy(false)
    }
  }

  const runRestore = async (): Promise<void> => {
    if (!pending) {
      return
    }
    setBusy(true)
    setStatus(null)
    try {
      const result = await window.wassh.restoreBackup(pending.path)
      if (!result.ok) {
        setStatus({ kind: 'error', message: result.error ?? 'Restore failed' })
        return
      }
      // A successful restore reloads the window, so this banner is brief.
      setStatus({
        kind: 'ok',
        message: `Restored ${result.restored?.length ?? 0} config files — reloading…`
      })
      setPending(null)
    } finally {
      setBusy(false)
    }
  }

  const manifest = pending?.info.manifest

  return (
    <>
      <div className="settings-row">
        <div className="settings-row-label">
          <strong>Export settings</strong>
          <span>
            Write all settings, hosts, session tabs, known hosts, plugin data, and the saved
            credentials vault to a single zip archive.
          </span>
        </div>
        <button type="button" onClick={() => void runExport()} disabled={busy}>
          Export…
        </button>
      </div>

      <div className="settings-row">
        <div className="settings-row-label">
          <strong>Restore settings</strong>
          <span>
            Replace all current configuration from a backup zip. This cannot be undone, and the app
            reloads afterwards.
          </span>
        </div>
        <button type="button" onClick={() => void runPick()} disabled={busy}>
          Choose file…
        </button>
      </div>

      {pending && manifest ? (
        <div className="settings-row settings-backup-confirm">
          <div className="settings-row-label">
            <strong>Confirm restore</strong>
            <span>
              Created {formatCreatedAt(manifest.createdAt)} with WaSSH {manifest.appVersion} ·{' '}
              {manifest.entries.length} file{manifest.entries.length === 1 ? '' : 's'} · format v
              {manifest.formatVersion}
            </span>
            {pending.info.versionMismatch ? (
              <span className="settings-backup-warn">
                This backup was made with a different WaSSH version. Restoring may not be fully
                compatible.
              </span>
            ) : null}
          </div>
          <div className="settings-backup-actions">
            <button type="button" onClick={() => setPending(null)} disabled={busy}>
              Cancel
            </button>
            <button
              type="button"
              className="danger"
              onClick={() => void runRestore()}
              disabled={busy}
            >
              Restore and reload
            </button>
          </div>
        </div>
      ) : null}

      {status ? (
        <div
          className={`settings-backup-status settings-backup-status-${status.kind}`}
          role="status"
        >
          {status.message}
        </div>
      ) : null}

      <p className="plugin-settings-external-note">
        Backups are written in plain (unencrypted) JSON inside the zip. The credentials vault is
        included as-is, so it can only be decrypted on the same OS user account that created it.
        Current app version: {APP_VERSION}.
      </p>
    </>
  )
}
