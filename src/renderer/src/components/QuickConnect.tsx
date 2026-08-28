import { useState } from 'react'
import { DEFAULT_SSH_PORT, type ConnectionParams } from '@shared/types'
import { sessionStyleFrom } from '@shared/connection'

interface Props {
  onConnect: (connection: ConnectionParams) => void
}

export default function QuickConnect({ onConnect }: Props) {
  const [host, setHost] = useState('')
  const [port, setPort] = useState(String(DEFAULT_SSH_PORT))
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')

  const submit = (): void => {
    const trimmed = host.trim()
    if (!trimmed) {
      return
    }
    onConnect({
      hostId: null,
      name: username ? `${username}@${trimmed}` : trimmed,
      host: trimmed,
      port: Number(port) || DEFAULT_SSH_PORT,
      username: username.trim(),
      passwordVaultId: '',
      privateKeyPath: '',
      passphraseVaultId: '',
      authMethod: password ? 'password' : 'none',
      proxyHostId: '',
      ...sessionStyleFrom(null),
      ephemeralPassword: password,
      ephemeralPassphrase: ''
    })
  }

  return (
    <div className="quick-connect">
      <h3>Quick connect</h3>
      <div className="field-row">
        <label htmlFor="qc-host">Host</label>
        <div className="quick-connect-row">
          <input
            id="qc-host"
            value={host}
            onChange={(e) => setHost(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && submit()}
            placeholder="hostname"
          />
          <input
            aria-label="Port"
            value={port}
            onChange={(e) => setPort(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && submit()}
          />
        </div>
      </div>
      <div className="field-row">
        <label htmlFor="qc-user">Username</label>
        <input
          id="qc-user"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && submit()}
        />
      </div>
      <div className="field-row">
        <label htmlFor="qc-pass">Password</label>
        <input
          id="qc-pass"
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && submit()}
          placeholder="optional"
        />
      </div>
      <button type="button" className="primary" onClick={submit}>
        Connect
      </button>
    </div>
  )
}
