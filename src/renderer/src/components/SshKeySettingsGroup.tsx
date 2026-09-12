import { useEffect, useState } from 'react'
import type {
  AuthMethod,
  SshKeyAlgorithm,
  SshKeyInfo,
  SshKeyLocalItem
} from '../../../shared/types'

/** Default algorithm for new SSH key generation */
const DEFAULT_KEY_ALGO: SshKeyAlgorithm = 'ed25519'
/** RSA algorithm identifier */
const RSA_KEY_ALGO: SshKeyAlgorithm = 'rsa'
/** Default filename for Ed25519 keys */
const DEFAULT_ED25519_NAME = 'id_ed25519'
/** Default filename for RSA keys */
const DEFAULT_RSA_NAME = 'id_rsa'
/** Reset delay for the "Copied!" button text in ms */
const COPIED_FEEDBACK_MS = 2000

interface Props {
  mode: 'editHost' | 'editOpenSession'
  host: string
  port: number
  username: string
  password: string
  passwordVaultId: string
  privateKeyPath: string
  passphrase: string
  passphraseVaultId: string
  proxyHostId: string
  identityLocked: boolean
  onSelectKey: (path: string) => void
  onPassphraseChange: (passphrase: string) => void
  onAuthMethodChange: (method: AuthMethod) => void
  pickPrivateKey: () => Promise<string | null>
}

type ActionStatus = 'idle' | 'running' | 'success' | 'error'

export default function SshKeySettingsGroup({
  mode,
  host,
  port,
  username,
  password,
  passwordVaultId,
  privateKeyPath,
  passphrase,
  passphraseVaultId,
  proxyHostId,
  identityLocked,
  onSelectKey,
  onPassphraseChange,
  onAuthMethodChange,
  pickPrivateKey
}: Props) {
  const [localKeys, setLocalKeys] = useState<SshKeyLocalItem[]>([])
  const [keyInfo, setKeyInfo] = useState<SshKeyInfo | null>(null)
  const [copied, setCopied] = useState(false)

  // Generate Key state
  const [genAlgo, setGenAlgo] = useState<SshKeyAlgorithm>(DEFAULT_KEY_ALGO)
  const [genFilename, setGenFilename] = useState(DEFAULT_ED25519_NAME)
  const [genPassphrase, setGenPassphrase] = useState('')
  const [genStatus, setGenStatus] = useState<ActionStatus>('idle')
  const [genMessage, setGenMessage] = useState('')

  // Deploy Key state
  const [deployPassword, setDeployPassword] = useState('')
  const [deployStatus, setDeployStatus] = useState<ActionStatus>('idle')
  const [deployMessage, setDeployMessage] = useState('')

  // Retrieve Key state
  const [retrievePassword, setRetrievePassword] = useState('')
  const [retrieveStatus, setRetrieveStatus] = useState<ActionStatus>('idle')
  const [retrieveMessage, setRetrieveMessage] = useState('')

  const refreshLocalKeys = (): void => {
    window.wassh.listLocalSshKeys().then((keys) => {
      setLocalKeys(keys)
    }).catch(() => {
      setLocalKeys([])
    })
  }

  useEffect(() => {
    refreshLocalKeys()
  }, [])

  useEffect(() => {
    if (!privateKeyPath) {
      setKeyInfo(null)
      return
    }
    window.wassh.getSshKeyInfo(privateKeyPath, passphrase).then((info) => {
      setKeyInfo(info)
    }).catch(() => {
      setKeyInfo(null)
    })
  }, [privateKeyPath, passphrase])

  const handleCopyPublic = (): void => {
    if (!keyInfo?.publicKey) {
      return
    }
    navigator.clipboard.writeText(keyInfo.publicKey).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS)
    }).catch(() => {
      // Ignore clipboard write failure
    })
  }

  const handleAlgoChange = (algo: SshKeyAlgorithm): void => {
    setGenAlgo(algo)
    if (algo === RSA_KEY_ALGO && genFilename === DEFAULT_ED25519_NAME) {
      setGenFilename(DEFAULT_RSA_NAME)
    } else if (algo === DEFAULT_KEY_ALGO && genFilename === DEFAULT_RSA_NAME) {
      setGenFilename(DEFAULT_ED25519_NAME)
    }
  }

  const handleGenerate = async (): Promise<void> => {
    setGenStatus('running')
    setGenMessage('Generating key pair…')
    try {
      const result = await window.wassh.generateSshKey({
        type: genAlgo,
        filename: genFilename,
        passphrase: genPassphrase || undefined
      })
      setGenStatus('success')
      setGenMessage(`Created key pair at ${result.privateKeyPath}`)
      onSelectKey(result.privateKeyPath)
      onAuthMethodChange('privateKey')
      if (genPassphrase) {
        onPassphraseChange(genPassphrase)
      }
      refreshLocalKeys()
    } catch (err) {
      setGenStatus('error')
      setGenMessage(err instanceof Error ? err.message : String(err))
    }
  }

  const handleDeploy = async (): Promise<void> => {
    if (!keyInfo?.publicKey) {
      setDeployStatus('error')
      setDeployMessage('No public key available. Select or generate a key first.')
      return
    }
    if (!host) {
      setDeployStatus('error')
      setDeployMessage('Host address is required.')
      return
    }

    setDeployStatus('running')
    setDeployMessage('Connecting to host and adding public key…')
    try {
      const effectivePassword = deployPassword || password
      const result = await window.wassh.deploySshKey({
        host,
        port,
        username: username || 'root',
        password: effectivePassword || undefined,
        passwordVaultId: !effectivePassword && passwordVaultId ? passwordVaultId : undefined,
        proxyHostId: proxyHostId || undefined,
        publicKey: keyInfo.publicKey
      })

      if (result.success) {
        setDeployStatus('success')
        setDeployMessage(result.message)
        onAuthMethodChange('privateKey')
      } else {
        setDeployStatus('error')
        setDeployMessage(result.message)
      }
    } catch (err) {
      setDeployStatus('error')
      setDeployMessage(err instanceof Error ? err.message : String(err))
    }
  }

  const handleRetrieve = async (): Promise<void> => {
    if (!host) {
      setRetrieveStatus('error')
      setRetrieveMessage('Host address is required.')
      return
    }

    setRetrieveStatus('running')
    setRetrieveMessage('Scanning remote host for existing SSH keys…')
    try {
      const effectivePassword = retrievePassword || password
      const result = await window.wassh.retrieveSshKey({
        host,
        port,
        username: username || 'root',
        password: effectivePassword || undefined,
        passwordVaultId: !effectivePassword && passwordVaultId ? passwordVaultId : undefined,
        proxyHostId: proxyHostId || undefined
      })

      if (result.success && result.privateKeyPath) {
        setRetrieveStatus('success')
        setRetrieveMessage(result.message)
        onSelectKey(result.privateKeyPath)
        onAuthMethodChange('privateKey')
        refreshLocalKeys()
      } else {
        setRetrieveStatus('error')
        setRetrieveMessage(result.message)
      }
    } catch (err) {
      setRetrieveStatus('error')
      setRetrieveMessage(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div className="ssh-keys-manager">
      {/* 1. Active Key Selector */}
      <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
        <div className="settings-row-label">
          <strong>Private key path</strong>
          <span>Select from ~/.ssh or browse for an OpenSSH private key file.</span>
        </div>
        <div className="stack">
          {localKeys.length > 0 ? (
            <select
              value={localKeys.some((k) => k.path === privateKeyPath) ? privateKeyPath : ''}
              disabled={identityLocked}
              onChange={(e) => {
                if (e.target.value) {
                  onSelectKey(e.target.value)
                  onAuthMethodChange('privateKey')
                }
              }}
            >
              <option value="">Custom / choose from list…</option>
              {localKeys.map((k) => (
                <option key={k.path} value={k.path}>
                  {k.name} ({k.type})
                </option>
              ))}
            </select>
          ) : null}
          <div style={{ display: 'flex', gap: 6 }}>
            <input
              type="text"
              style={{ flex: 1 }}
              value={privateKeyPath}
              placeholder="e.g. ~/.ssh/id_ed25519"
              onChange={(e) => onSelectKey(e.target.value)}
              readOnly={identityLocked}
            />
            {!identityLocked ? (
              <button
                type="button"
                onClick={() => {
                  void pickPrivateKey().then((p) => {
                    if (p) {
                      onSelectKey(p)
                      onAuthMethodChange('privateKey')
                    }
                  })
                }}
              >
                Browse…
              </button>
            ) : null}
          </div>
        </div>
      </div>

      {/* Key Passphrase */}
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
            placeholder={passphraseVaultId ? '(stored)' : ''}
            onChange={(e) => onPassphraseChange(e.target.value)}
          />
        </div>
      ) : null}

      {/* Public Key Display */}
      {keyInfo?.publicKey ? (
        <div className="settings-row settings-row-block">
          <div className="settings-row-label">
            <strong>Public key</strong>
            <span>Corresponding public key for deployment or sharing.</span>
          </div>
          <div className="ssh-key-pub-box">
            <pre className="ssh-key-pub-text">{keyInfo.publicKey}</pre>
            <button
              type="button"
              className="ssh-key-copy-btn"
              onClick={handleCopyPublic}
            >
              {copied ? 'Copied!' : 'Copy public key'}
            </button>
          </div>
        </div>
      ) : null}

      {/* 2. Password-less Login / Deploy Key */}
      <div className="settings-row settings-row-block">
        <div className="settings-row-label">
          <strong>Password-less login setup</strong>
          <span>
            Deploy the selected public key to {username || 'root'}@{host || 'host'} to enable
            instant, password-less SSH authentication.
          </span>
        </div>
        <div className="ssh-key-action-card">
          {!password && !passwordVaultId ? (
            <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 8 }}>
              <input
                type="password"
                placeholder="Enter host password for setup"
                value={deployPassword}
                onChange={(e) => setDeployPassword(e.target.value)}
                style={{ flex: 1 }}
              />
            </div>
          ) : null}
          <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            <button
              type="button"
              disabled={deployStatus === 'running' || !privateKeyPath || !host}
              onClick={() => void handleDeploy()}
            >
              {deployStatus === 'running' ? 'Deploying…' : 'Deploy key to host'}
            </button>
            {deployMessage ? (
              <span className={`ssh-key-status ${deployStatus}`}>{deployMessage}</span>
            ) : null}
          </div>
        </div>
      </div>

      {/* 3. Generate Key */}
      <div className="settings-row settings-row-block">
        <div className="settings-row-label">
          <strong>Generate new key pair</strong>
          <span>Create a secure SSH key pair locally in your ~/.ssh directory.</span>
        </div>
        <div className="ssh-key-action-card">
          <div className="ssh-key-gen-fields">
            <label className="ssh-key-gen-field">
              <span>Type</span>
              <select
                value={genAlgo}
                onChange={(e) => handleAlgoChange(e.target.value as SshKeyAlgorithm)}
              >
                <option value={DEFAULT_KEY_ALGO}>Ed25519 (Recommended)</option>
                <option value={RSA_KEY_ALGO}>RSA (4096-bit)</option>
              </select>
            </label>
            <label className="ssh-key-gen-field">
              <span>Filename</span>
              <input
                type="text"
                value={genFilename}
                onChange={(e) => setGenFilename(e.target.value)}
                placeholder="id_ed25519"
              />
            </label>
            <label className="ssh-key-gen-field">
              <span>Passphrase (optional)</span>
              <input
                type="password"
                value={genPassphrase}
                onChange={(e) => setGenPassphrase(e.target.value)}
                placeholder="Empty for no passphrase"
              />
            </label>
          </div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginTop: 8 }}>
            <button
              type="button"
              disabled={genStatus === 'running' || !genFilename.trim()}
              onClick={() => void handleGenerate()}
            >
              {genStatus === 'running' ? 'Generating…' : 'Generate key pair'}
            </button>
            {genMessage ? (
              <span className={`ssh-key-status ${genStatus}`}>{genMessage}</span>
            ) : null}
          </div>
        </div>
      </div>

      {/* 4. Retrieve Key from Host */}
      <div className="settings-row settings-row-block">
        <div className="settings-row-label">
          <strong>Retrieve key from host</strong>
          <span>
            Download an existing SSH key pair from {username || 'root'}@{host || 'host'} to use
            locally.
          </span>
        </div>
        <div className="ssh-key-action-card">
          {!password && !passwordVaultId ? (
            <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 8 }}>
              <input
                type="password"
                placeholder="Enter host password to authenticate"
                value={retrievePassword}
                onChange={(e) => setRetrievePassword(e.target.value)}
                style={{ flex: 1 }}
              />
            </div>
          ) : null}
          <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            <button
              type="button"
              disabled={retrieveStatus === 'running' || !host}
              onClick={() => void handleRetrieve()}
            >
              {retrieveStatus === 'running' ? 'Retrieving…' : 'Retrieve key from host'}
            </button>
            {retrieveMessage ? (
              <span className={`ssh-key-status ${retrieveStatus}`}>{retrieveMessage}</span>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  )
}
