import { useMemo, useState } from 'react'
import type { AuthMethod, ConnectionParams, HostProfile } from '@shared/types'
import { DEFAULT_SSH_PORT } from '@shared/types'
import { hostToConnection } from '@shared/connection'
import SettingsDialog, { type SettingsSection } from './SettingsDialog'

export type HostSessionMode = 'editHost' | 'editOpenSession'

interface Props {
  mode: HostSessionMode
  connected: boolean
  hosts: HostProfile[]
  initial: ConnectionParams | HostProfile
  onSaveHost: (host: HostProfile, password: string, passphrase: string) => void
  onSaveSession: (connection: ConnectionParams) => void
  onClose: () => void
  pickPrivateKey: () => Promise<string | null>
}

function toConnection(initial: ConnectionParams | HostProfile): ConnectionParams {
  if ('ephemeralPassword' in initial) {
    return { ...initial, proxyHostId: initial.proxyHostId || '' }
  }
  return hostToConnection(initial)
}

export default function HostSessionSettingsDialog({
  mode,
  connected,
  hosts,
  initial,
  onSaveHost,
  onSaveSession,
  onClose,
  pickPrivateKey
}: Props) {
  const [form, setForm] = useState<ConnectionParams>(() => toConnection(initial))
  const [password, setPassword] = useState('')
  const [passphrase, setPassphrase] = useState('')
  const identityLocked = mode === 'editOpenSession' && connected
  const editingHostId =
    mode === 'editHost' && 'id' in initial ? initial.id : form.hostId || ''
  const proxyOptions = hosts.filter((h) => h.id !== editingHostId)

  const patch = (partial: Partial<ConnectionParams>): void => {
    setForm((prev) => ({ ...prev, ...partial }))
  }

  const sections: SettingsSection[] = useMemo(() => {
    const connectionRows = (
      <>
        {mode === 'editHost' ? (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Name</strong>
              <span>Display name in the sidebar.</span>
            </div>
            <input
              type="text"
              value={form.name}
              onChange={(e) => patch({ name: e.target.value })}
            />
          </div>
        ) : null}
        <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
          <div className="settings-row-label">
            <strong>Host</strong>
            <span>Hostname or IP.</span>
          </div>
          <input
            type="text"
            value={form.host}
            onChange={(e) => patch({ host: e.target.value })}
            readOnly={identityLocked}
          />
        </div>
        <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
          <div className="settings-row-label">
            <strong>Port</strong>
            <span>SSH port.</span>
          </div>
          <input
            type="number"
            value={form.port}
            onChange={(e) => patch({ port: Number(e.target.value) || DEFAULT_SSH_PORT })}
            readOnly={identityLocked}
          />
        </div>
        <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
          <div className="settings-row-label">
            <strong>Username</strong>
            <span>Leave empty to prompt in the terminal.</span>
          </div>
          <input
            type="text"
            value={form.username}
            onChange={(e) => patch({ username: e.target.value })}
            readOnly={identityLocked}
          />
        </div>
        <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
          <div className="settings-row-label">
            <strong>Auth method</strong>
            <span>Preferred authentication.</span>
          </div>
          <select
            value={form.authMethod}
            onChange={(e) => patch({ authMethod: e.target.value as AuthMethod })}
            disabled={identityLocked}
          >
            <option value="password">Password</option>
            <option value="privateKey">Private key</option>
            <option value="none">Prompt / none stored</option>
          </select>
        </div>
        {!identityLocked ? (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Password</strong>
              <span>
                {mode === 'editHost'
                  ? 'Stored in host settings (vaulted). Empty = prompt in terminal.'
                  : 'Session-local only; does not update the saved host.'}
              </span>
            </div>
            <input
              type="password"
              value={password}
              placeholder={form.passwordVaultId ? '(stored)' : ''}
              onChange={(e) => setPassword(e.target.value)}
            />
          </div>
        ) : null}
        <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
          <div className="settings-row-label">
            <strong>Private key path</strong>
            <span>Path to OpenSSH private key file.</span>
          </div>
          <div className="stack">
            <input
              type="text"
              value={form.privateKeyPath}
              onChange={(e) => patch({ privateKeyPath: e.target.value })}
              readOnly={identityLocked}
            />
            {!identityLocked ? (
              <button
                type="button"
                onClick={() => {
                  void pickPrivateKey().then((p) => {
                    if (p) {
                      patch({ privateKeyPath: p, authMethod: 'privateKey' })
                    }
                  })
                }}
              >
                Browse…
              </button>
            ) : null}
          </div>
        </div>
        {!identityLocked ? (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Key passphrase</strong>
              <span>
                {mode === 'editHost' ? 'Vaulted with the host profile.' : 'Session-local only.'}
              </span>
            </div>
            <input
              type="password"
              value={passphrase}
              placeholder={form.passphraseVaultId ? '(stored)' : ''}
              onChange={(e) => setPassphrase(e.target.value)}
            />
          </div>
        ) : null}
        <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
          <div className="settings-row-label">
            <strong>Proxy / jump host</strong>
            <span>
              Connect through another saved host first (for closed networks / bastion access).
            </span>
          </div>
          <select
            value={form.proxyHostId}
            onChange={(e) => patch({ proxyHostId: e.target.value })}
            disabled={identityLocked}
          >
            <option value="">None (direct)</option>
            {proxyOptions.map((h) => (
              <option key={h.id} value={h.id}>
                {h.name || `${h.username}@${h.host}`}
              </option>
            ))}
          </select>
        </div>
      </>
    )

    const list: SettingsSection[] = [
      {
        id: 'connection',
        title: 'Connection',
        content: connectionRows
      }
    ]

    if (mode === 'editOpenSession') {
      list.push({
        id: 'session',
        title: 'Session',
        content: (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Tab title</strong>
              <span>Local display name for this tab (does not rename the saved host).</span>
            </div>
            <input
              type="text"
              value={form.name}
              onChange={(e) => patch({ name: e.target.value })}
            />
          </div>
        )
      })
    }

    return list
  }, [form, identityLocked, mode, password, passphrase, pickPrivateKey, proxyOptions])

  const handleSave = (): void => {
    if (mode === 'editHost') {
      const id =
        form.hostId ||
        ('id' in initial ? initial.id : crypto.randomUUID())
      const host: HostProfile = {
        id,
        name: form.name || `${form.username}@${form.host}`,
        host: form.host,
        port: form.port,
        username: form.username,
        passwordVaultId: form.passwordVaultId || `pwd-${id}`,
        privateKeyPath: form.privateKeyPath,
        passphraseVaultId: form.passphraseVaultId || (passphrase ? `pp-${id}` : ''),
        authMethod: form.authMethod,
        proxyHostId: form.proxyHostId || ''
      }
      onSaveHost(host, password, passphrase)
      onClose()
      return
    }

    onSaveSession({
      ...form,
      ephemeralPassword: password || form.ephemeralPassword,
      ephemeralPassphrase: passphrase || form.ephemeralPassphrase
    })
    onClose()
  }

  return (
    <SettingsDialog
      title={mode === 'editHost' ? 'Host settings' : 'Session settings'}
      sections={sections}
      onClose={onClose}
      footer={
        <>
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="primary" onClick={handleSave}>
            Save
          </button>
        </>
      }
    />
  )
}
