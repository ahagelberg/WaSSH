import { useEffect, useMemo, useState } from 'react'
import {
  BELL_MODE_INVERT_LINE,
  BELL_MODE_INVERT_WINDOW,
  BELL_MODE_SYSTEM,
  BUNDLED_FONT_FAMILIES,
  CURSOR_STYLE_BAR,
  CURSOR_STYLE_BLOCK,
  CURSOR_STYLE_UNDERLINE,
  DEFAULT_CURSOR_BLINK,
  DEFAULT_CURSOR_STYLE,
  DEFAULT_FONT_SIZE_PX,
  DEFAULT_SSH_PORT,
  FONT_SIZE_MAX_PX,
  FONT_SIZE_MIN_PX,
  type AuthMethod,
  type BellMode,
  type ConnectionParams,
  type CursorStyle,
  type HostProfile
} from '@shared/types'
import { sessionStyleFrom, hostToConnection } from '@shared/connection'
import SettingsDialog, { type SettingsSection } from './SettingsDialog'
import { fontSelectOptions, listMonospaceFontFamilies } from '../fonts'

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
    return {
      ...initial,
      proxyHostId: initial.proxyHostId || '',
      ...sessionStyleFrom(initial)
    }
  }
  return hostToConnection(initial)
}

/** CSS variable used as the tab custom-color starting sample */
const TAB_COLOR_THEME_VAR = '--accent'
/** CSS variable for themed terminal background */
const TERM_BG_THEME_VAR = '--bg-term'
/** CSS variable for themed terminal text */
const TERM_FG_THEME_VAR = '--text'
/** Hex digits per RGB channel */
const HEX_CHANNEL_DIGITS = 2
const HEX_COLOR_RE = /^#[0-9a-fA-F]{6}$/

function themeVarHex(cssVar: string): string {
  const raw = getComputedStyle(document.documentElement).getPropertyValue(cssVar).trim()
  if (HEX_COLOR_RE.test(raw)) {
    return raw
  }
  const rgb = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/.exec(raw)
  if (!rgb) {
    return raw
  }
  return `#${[rgb[1], rgb[2], rgb[3]]
    .map((n) => Number(n).toString(16).padStart(HEX_CHANNEL_DIGITS, '0'))
    .join('')}`
}

function ColorRow({
  label,
  hint,
  value,
  themeVar,
  onChange
}: {
  label: string
  hint: string
  value: string
  themeVar: string
  onChange: (value: string) => void
}) {
  const useTheme = !value
  const pickerValue = useTheme ? themeVarHex(themeVar) : value
  return (
    <div className="settings-row">
      <div className="settings-row-label">
        <strong>{label}</strong>
        <span>{hint}</span>
      </div>
      <div className="settings-color-field">
        <label className="settings-theme-default">
          <input
            type="checkbox"
            checked={useTheme}
            onChange={(e) =>
              onChange(e.target.checked ? '' : themeVarHex(themeVar))
            }
          />
          Theme default
        </label>
        {HEX_COLOR_RE.test(pickerValue) ? (
          <input
            type="color"
            value={pickerValue}
            disabled={useTheme}
            onChange={(e) => onChange(e.target.value)}
          />
        ) : null}
      </div>
    </div>
  )
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
  const [fontFamilies, setFontFamilies] = useState<string[]>(Array.from(BUNDLED_FONT_FAMILIES))
  const identityLocked = mode === 'editOpenSession' && connected
  const editingHostId =
    mode === 'editHost' && 'id' in initial ? initial.id : form.hostId || ''
  const proxyOptions = hosts.filter((h) => h.id !== editingHostId)

  const patch = (partial: Partial<ConnectionParams>): void => {
    setForm((prev) => ({ ...prev, ...partial }))
  }

  useEffect(() => {
    void listMonospaceFontFamilies().then(setFontFamilies)
  }, [])

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

    const fontHint =
      mode === 'editHost'
        ? 'Used for new sessions from this host. Open tabs keep their own copy.'
        : 'Applies only to this tab. Does not change the saved host.'

    const appearanceRows = (
      <>
        <ColorRow
          label="Tab color"
          hint="Leave as theme default for no accent. A custom color does not follow Dark/Light."
          value={form.tabColor}
          themeVar={TAB_COLOR_THEME_VAR}
          onChange={(tabColor) => patch({ tabColor })}
        />
        <ColorRow
          label="Terminal background"
          hint="Theme default follows the app theme. A custom color stays fixed."
          value={form.termBackground}
          themeVar={TERM_BG_THEME_VAR}
          onChange={(termBackground) => patch({ termBackground })}
        />
        <ColorRow
          label="Terminal text"
          hint="Theme default follows the app theme. A custom color stays fixed."
          value={form.termForeground}
          themeVar={TERM_FG_THEME_VAR}
          onChange={(termForeground) => patch({ termForeground })}
        />
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>Font family</strong>
            <span>{fontHint}</span>
          </div>
          <select
            value={form.fontFamily}
            onChange={(e) => patch({ fontFamily: e.target.value })}
          >
            {fontSelectOptions(fontFamilies, form.fontFamily).map((family) => (
              <option key={family} value={family}>
                {family}
              </option>
            ))}
          </select>
        </div>
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>Font size</strong>
            <span>{fontHint}</span>
          </div>
          <input
            type="number"
            min={FONT_SIZE_MIN_PX}
            max={FONT_SIZE_MAX_PX}
            value={form.fontSizePx}
            onChange={(e) =>
              patch({ fontSizePx: Number(e.target.value) || DEFAULT_FONT_SIZE_PX })
            }
          />
        </div>
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>Bell</strong>
            <span>{fontHint}</span>
          </div>
          <select
            value={form.bellMode}
            onChange={(e) => patch({ bellMode: e.target.value as BellMode })}
          >
            <option value={BELL_MODE_SYSTEM}>Default system sound</option>
            <option value={BELL_MODE_INVERT_WINDOW}>
              Blink whole terminal (invert background)
            </option>
            <option value={BELL_MODE_INVERT_LINE}>Blink current line</option>
          </select>
        </div>
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>Cursor</strong>
            <span>{fontHint}</span>
          </div>
          <select
            value={form.cursorStyle || DEFAULT_CURSOR_STYLE}
            onChange={(e) => patch({ cursorStyle: e.target.value as CursorStyle })}
          >
            <option value={CURSOR_STYLE_BLOCK}>Block</option>
            <option value={CURSOR_STYLE_UNDERLINE}>Underline</option>
            <option value={CURSOR_STYLE_BAR}>Vertical line</option>
          </select>
        </div>
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>Cursor blink</strong>
            <span>{fontHint}</span>
          </div>
          <input
            type="checkbox"
            checked={form.cursorBlink ?? DEFAULT_CURSOR_BLINK}
            onChange={(e) => patch({ cursorBlink: e.target.checked })}
          />
        </div>
      </>
    )

    const list: SettingsSection[] = [
      {
        id: 'connection',
        title: 'Connection',
        content: connectionRows
      },
      {
        id: 'appearance',
        title: 'Appearance',
        content: appearanceRows
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
  }, [form, fontFamilies, identityLocked, mode, password, passphrase, pickPrivateKey, proxyOptions])

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
        proxyHostId: form.proxyHostId || '',
        ...sessionStyleFrom(form)
      }
      onSaveHost(host, password, passphrase)
      onClose()
      return
    }

    onSaveSession({
      ...form,
      ...sessionStyleFrom(form),
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
