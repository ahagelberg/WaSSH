import { useEffect, useMemo, useState, type ReactNode } from 'react'
import {
  BELL_MODE_INVERT_LINE,
  BELL_MODE_INVERT_WINDOW,
  BELL_MODE_SYSTEM,
  BUNDLED_FONT_FAMILIES,
  CONNECTION_TYPE_SERIAL,
  CONNECTION_TYPE_SSH,
  CONNECTION_TYPE_TELNET,
  CURSOR_STYLE_BAR,
  CURSOR_STYLE_BLOCK,
  CURSOR_STYLE_UNDERLINE,
  DEFAULT_CURSOR_BLINK,
  DEFAULT_CURSOR_STYLE,
  DEFAULT_FONT_SIZE_PX,
  DEFAULT_SCROLLBACK_LINES,
  FONT_SIZE_MAX_PX,
  FONT_SIZE_MIN_PX,
  RECONNECT_MODE_ALWAYS,
  RECONNECT_MODE_NONE,
  RECONNECT_MODE_ON_FOCUS,
  REMOTE_SESSION_KIND_SCREEN,
  REMOTE_SESSION_KIND_TMUX,
  SCREEN_BUSY_DO_NOT_ATTACH,
  SCREEN_BUSY_FORCE_DETACH,
  SCREEN_BUSY_SHARE,
  DEFAULT_SCREEN_SESSION_NAME,
  SCROLLBACK_LINES_MAX,
  SCROLLBACK_LINES_MIN,
  SERIAL_BAUD_MAX,
  SERIAL_BAUD_MIN,
  SERIAL_BAUD_RATES,
  SERIAL_DATA_BITS_5,
  SERIAL_DATA_BITS_6,
  SERIAL_DATA_BITS_7,
  SERIAL_DATA_BITS_8,
  SERIAL_FLOW_NONE,
  SERIAL_FLOW_RTSCTS,
  SERIAL_FLOW_XONXOFF,
  SERIAL_PARITY_EVEN,
  SERIAL_PARITY_MARK,
  SERIAL_PARITY_NONE,
  SERIAL_PARITY_ODD,
  SERIAL_PARITY_SPACE,
  SERIAL_STOP_BITS_1,
  SERIAL_STOP_BITS_1_5,
  SERIAL_STOP_BITS_2,
  TUNNEL_PORT_MAX,
  TUNNEL_PORT_MIN,
  TUNNEL_TYPE_DYNAMIC,
  TUNNEL_TYPE_LOCAL,
  TUNNEL_TYPE_REMOTE,
  type AuthMethod,
  type BellMode,
  type ConnectionParams,
  type ConnectionType,
  type CursorStyle,
  type HostProfile,
  type ReconnectMode,
  type RemoteSessionKind,
  type ScreenBusyHandling,
  type SerialDataBits,
  type SerialFlowControl,
  type SerialParity,
  type SerialStopBits,
  type SshTunnel,
  type TunnelType
} from '@shared/types'
import {
  connectionTypeOf,
  defaultPortForType,
  hostToConnection,
  isSshConnectionType,
  protocolConfigFrom,
  reconnectModeFrom,
  resolveSessionStyle,
  screenConfigFrom,
  sessionStyleOverridesFrom,
  tunnelConfigFrom,
  type SessionStyle
} from '@shared/connection'
import {
  mergePluginSettings,
  normalizeHostPluginSettings,
  type PluginListItem
} from '@shared/plugins'
import SettingsDialog, { type SettingsSection } from './SettingsDialog'
import ClampedNumberInput from './ClampedNumberInput'
import ColorHexInput from './ColorHexInput'
import { fontSelectOptions, listMonospaceFontFamilies } from '../fonts'
import SerialPortField from './SerialPortField'
import PluginFieldEditor from '../plugins/PluginFieldEditor'
import TagInput from './TagInput'
import TunnelBuilder from './TunnelBuilder'

export type HostSessionMode = 'editHost' | 'editOpenSession'

interface Props {
  mode: HostSessionMode
  connected: boolean
  hosts: HostProfile[]
  initial: ConnectionParams | HostProfile
  /** App-level defaults used when a field is set to “Use default” */
  styleDefaults: SessionStyle
  onSaveHost: (host: HostProfile, password: string, passphrase: string) => void
  onSaveSession: (connection: ConnectionParams) => void
  /** Push the edited host settings to every open session of this host */
  onApplyToSessions?: (host: HostProfile, password: string, passphrase: string) => void
  onClose: () => void
  pickPrivateKey: () => Promise<string | null>
}

function toConnection(initial: ConnectionParams | HostProfile): ConnectionParams {
  if ('ephemeralPassword' in initial) {
    return {
      ...initial,
      proxyHostId: initial.proxyHostId || '',
      ...protocolConfigFrom(initial),
      ...sessionStyleOverridesFrom(initial),
      ...tunnelConfigFrom(initial),
      pluginSettings: normalizeHostPluginSettings(initial.pluginSettings),
      reconnectMode: reconnectModeFrom(initial),
      ...screenConfigFrom(initial)
    }
  }
  return hostToConnection(initial)
}

/** CSS variable used as the tab custom-color starting sample */
const TAB_COLOR_THEME_VAR = '--accent'
/** CSS variable for themed terminal background */
const TERM_BG_THEME_VAR = '--bg-term'
/** Default terminal text color: fixed, not theme-dependent */
const TERM_FG_THEME_VAR = '--term-fg-default'
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
  defaultLabel,
  resolvedFallback,
  onChange
}: {
  label: string
  hint: string
  value: string
  themeVar: string
  defaultLabel: string
  /** Hex used when turning off “use default” if theme var is not a hex */
  resolvedFallback: string
  onChange: (value: string) => void
}) {
  const useDefault = !value
  const themeHex = themeVarHex(themeVar)
  const pickerValue = useDefault
    ? HEX_COLOR_RE.test(resolvedFallback)
      ? resolvedFallback
      : themeHex
    : value
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
            checked={useDefault}
            onChange={(e) =>
              onChange(
                e.target.checked
                  ? ''
                  : HEX_COLOR_RE.test(resolvedFallback)
                    ? resolvedFallback
                    : themeHex
              )
            }
          />
          {defaultLabel}
        </label>
        {HEX_COLOR_RE.test(pickerValue) ? (
          <>
            <input
              type="color"
              value={pickerValue}
              onClick={() => {
                // Clicking the swatch starts a custom color: clear “use default”
                // even if the picker is cancelled.
                if (useDefault) {
                  onChange(HEX_COLOR_RE.test(resolvedFallback) ? resolvedFallback : themeHex)
                }
              }}
              onChange={(e) => onChange(e.target.value)}
            />
            <ColorHexInput value={pickerValue} onChange={onChange} />
          </>
        ) : null}
      </div>
    </div>
  )
}

function DefaultableControl({
  label,
  hint,
  useDefault,
  onUseDefaultChange,
  children
}: {
  label: string
  hint: string
  useDefault: boolean
  onUseDefaultChange: (useDefault: boolean) => void
  children: ReactNode
}) {
  return (
    <div className="settings-row">
      <div className="settings-row-label">
        <strong>{label}</strong>
        <span>{hint}</span>
      </div>
      <div className="settings-defaultable-field">
        <label className="settings-theme-default">
          <input
            type="checkbox"
            checked={useDefault}
            onChange={(e) => onUseDefaultChange(e.target.checked)}
          />
          Use default
        </label>
        <div className={useDefault ? 'settings-defaultable-control is-default' : 'settings-defaultable-control'}>
          {children}
        </div>
      </div>
    </div>
  )
}

export default function HostSessionSettingsDialog({
  mode,
  connected,
  hosts,
  initial,
  styleDefaults,
  onSaveHost,
  onSaveSession,
  onApplyToSessions,
  onClose,
  pickPrivateKey
}: Props) {
  const [form, setForm] = useState<ConnectionParams>(() => toConnection(initial))
  const [password, setPassword] = useState('')
  const [passphrase, setPassphrase] = useState('')
  const [tags, setTags] = useState<string[]>(() => {
    if (mode !== 'editHost' || !('tags' in initial)) {
      return []
    }
    return initial.tags ?? []
  })
  const [fontFamilies, setFontFamilies] = useState<string[]>(Array.from(BUNDLED_FONT_FAMILIES))
  const [plugins, setPlugins] = useState<PluginListItem[]>([])
  const [showTunnelBuilder, setShowTunnelBuilder] = useState(false)
  const identityLocked = mode === 'editOpenSession' && connected
  const editingHostId =
    mode === 'editHost' && 'id' in initial ? initial.id : form.hostId || ''
  const connType = connectionTypeOf(form)
  const isSsh = isSshConnectionType(connType)
  const isSerial = connType === CONNECTION_TYPE_SERIAL
  const proxyOptions = hosts.filter(
    (h) => h.id !== editingHostId && isSshConnectionType(connectionTypeOf(h))
  )

  const patch = (partial: Partial<ConnectionParams>): void => {
    setForm((prev) => ({ ...prev, ...partial }))
  }

  const patchPluginHostSetting = (
    pluginId: string,
    key: string,
    value: unknown
  ): void => {
    setForm((prev) => {
      const plugin = plugins.find((p) => p.id === pluginId)
      const current = mergePluginSettings(
        plugin?.contributes.hostSettingsSchema,
        prev.pluginSettings[pluginId]
      )
      return {
        ...prev,
        pluginSettings: {
          ...prev.pluginSettings,
          [pluginId]: { ...current, [key]: value }
        }
      }
    })
  }

  const changeType = (next: ConnectionType): void => {
    const prevType = connectionTypeOf(form)
    const prevDefault = defaultPortForType(prevType)
    const nextDefault = defaultPortForType(next)
    const port =
      next === CONNECTION_TYPE_SERIAL
        ? 0
        : form.port === prevDefault || form.port === 0
          ? nextDefault
          : form.port
    patch({
      connectionType: next,
      port,
      proxyHostId: next === CONNECTION_TYPE_SSH ? form.proxyHostId : ''
    })
  }

  useEffect(() => {
    void listMonospaceFontFamilies().then(setFontFamilies)
  }, [])

  useEffect(() => {
    void window.wassh.listPlugins().then(setPlugins)
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
        {mode === 'editHost' ? (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Tags</strong>
              <span>Labels for this host. Type a tag and press Enter or comma to add it.</span>
            </div>
            <TagInput value={tags} onChange={setTags} placeholder="Add tag…" />
          </div>
        ) : null}
        <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
          <div className="settings-row-label">
            <strong>Type</strong>
            <span>SSH, Telnet, or serial.</span>
          </div>
          <select
            value={connType}
            onChange={(e) => changeType(e.target.value as ConnectionType)}
            disabled={identityLocked}
          >
            <option value={CONNECTION_TYPE_SSH}>SSH</option>
            <option value={CONNECTION_TYPE_TELNET}>Telnet</option>
            <option value={CONNECTION_TYPE_SERIAL}>Serial</option>
          </select>
        </div>
        {isSerial ? (
          <>
            <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
              <div className="settings-row-label">
                <strong>Serial port</strong>
                <span>Detected ports are listed; you can also type any path.</span>
              </div>
              <SerialPortField
                id="host-serial-port"
                listId="host-serial-ports"
                value={form.host}
                disabled={identityLocked}
                onChange={(host) => patch({ host })}
              />
            </div>
            <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
              <div className="settings-row-label">
                <strong>Baud rate</strong>
                <span>Common rates are suggested; any value in range is allowed.</span>
              </div>
              <div className="stack">
                <input
                  type="number"
                  list="host-baud-rates"
                  min={SERIAL_BAUD_MIN}
                  max={SERIAL_BAUD_MAX}
                  value={form.serialBaudRate}
                  readOnly={identityLocked}
                  onChange={(e) =>
                    patch({ serialBaudRate: Number(e.target.value) || form.serialBaudRate })
                  }
                />
                <datalist id="host-baud-rates">
                  {SERIAL_BAUD_RATES.map((rate) => (
                    <option key={rate} value={rate} />
                  ))}
                </datalist>
              </div>
            </div>
            <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
              <div className="settings-row-label">
                <strong>Data bits</strong>
              </div>
              <select
                value={form.serialDataBits}
                disabled={identityLocked}
                onChange={(e) =>
                  patch({ serialDataBits: Number(e.target.value) as SerialDataBits })
                }
              >
                <option value={SERIAL_DATA_BITS_5}>5</option>
                <option value={SERIAL_DATA_BITS_6}>6</option>
                <option value={SERIAL_DATA_BITS_7}>7</option>
                <option value={SERIAL_DATA_BITS_8}>8</option>
              </select>
            </div>
            <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
              <div className="settings-row-label">
                <strong>Parity</strong>
              </div>
              <select
                value={form.serialParity}
                disabled={identityLocked}
                onChange={(e) => patch({ serialParity: e.target.value as SerialParity })}
              >
                <option value={SERIAL_PARITY_NONE}>None</option>
                <option value={SERIAL_PARITY_EVEN}>Even</option>
                <option value={SERIAL_PARITY_ODD}>Odd</option>
                <option value={SERIAL_PARITY_MARK}>Mark</option>
                <option value={SERIAL_PARITY_SPACE}>Space</option>
              </select>
            </div>
            <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
              <div className="settings-row-label">
                <strong>Stop bits</strong>
              </div>
              <select
                value={form.serialStopBits}
                disabled={identityLocked}
                onChange={(e) =>
                  patch({ serialStopBits: Number(e.target.value) as SerialStopBits })
                }
              >
                <option value={SERIAL_STOP_BITS_1}>1</option>
                <option value={SERIAL_STOP_BITS_1_5}>1.5</option>
                <option value={SERIAL_STOP_BITS_2}>2</option>
              </select>
            </div>
            <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
              <div className="settings-row-label">
                <strong>Flow control</strong>
              </div>
              <select
                value={form.serialFlowControl}
                disabled={identityLocked}
                onChange={(e) =>
                  patch({ serialFlowControl: e.target.value as SerialFlowControl })
                }
              >
                <option value={SERIAL_FLOW_NONE}>None</option>
                <option value={SERIAL_FLOW_RTSCTS}>Hardware (RTS/CTS)</option>
                <option value={SERIAL_FLOW_XONXOFF}>Software (XON/XOFF)</option>
              </select>
            </div>
          </>
        ) : (
          <>
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
                <span>{isSsh ? 'SSH port.' : 'Telnet port.'}</span>
              </div>
              <input
                type="number"
                value={form.port}
                onChange={(e) =>
                  patch({
                    port: Number(e.target.value) || defaultPortForType(connType)
                  })
                }
                readOnly={identityLocked}
              />
            </div>
          </>
        )}
        {isSsh ? (
          <>
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
          </>
        ) : null}
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>Reconnect</strong>
            <span>
              After a mid-session drop: none, when the window is focused, or keep retrying with
              backoff (also on focus).
            </span>
          </div>
          <select
            value={form.reconnectMode}
            onChange={(e) => patch({ reconnectMode: e.target.value as ReconnectMode })}
          >
            <option value={RECONNECT_MODE_NONE}>None</option>
            <option value={RECONNECT_MODE_ON_FOCUS}>On focus</option>
            <option value={RECONNECT_MODE_ALWAYS}>Always</option>
          </select>
        </div>
      </>
    )

    const proxyRows = (
      <div className={`settings-row${identityLocked ? ' readonly' : ''}`}>
        <div className="settings-row-label">
          <strong>Jump host</strong>
          <span>
            Connect through another saved SSH host first (for closed networks / bastion access).
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
    )

    const screenRows = (
      <>
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>Open in remote session</strong>
            <span>
              On connect, create or attach a named persistent session (GNU screen or tmux) so work
              survives client disconnects.
            </span>
          </div>
          <input
            type="checkbox"
            checked={form.openInScreen}
            onChange={(e) => {
              const openInScreen = e.target.checked
              patch({
                openInScreen,
                screenSessionName:
                  openInScreen && !form.screenSessionName.trim()
                    ? DEFAULT_SCREEN_SESSION_NAME
                    : form.screenSessionName
              })
            }}
          />
        </div>
        {form.openInScreen ? (
          <>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Session type</strong>
                <span>Multiplexer used for the remote session.</span>
              </div>
              <select
                value={form.remoteSessionKind}
                onChange={(e) =>
                  patch({ remoteSessionKind: e.target.value as RemoteSessionKind })
                }
              >
                <option value={REMOTE_SESSION_KIND_SCREEN}>GNU Screen</option>
                <option value={REMOTE_SESSION_KIND_TMUX}>tmux</option>
              </select>
            </div>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Session name</strong>
                <span>Remote session name used with screen -S / tmux -s and attach.</span>
              </div>
              <input
                value={form.screenSessionName}
                onChange={(e) => patch({ screenSessionName: e.target.value })}
                placeholder={DEFAULT_SCREEN_SESSION_NAME}
              />
            </div>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Session busy handling</strong>
                <span>
                  When the named session already has another display attached: skip attach, share,
                  or force-detach the other display.
                </span>
              </div>
              <select
                value={form.screenBusyHandling}
                onChange={(e) =>
                  patch({ screenBusyHandling: e.target.value as ScreenBusyHandling })
                }
              >
                <option value={SCREEN_BUSY_DO_NOT_ATTACH}>Do not attach</option>
                <option value={SCREEN_BUSY_SHARE}>Share</option>
                <option value={SCREEN_BUSY_FORCE_DETACH}>Force detach</option>
              </select>
            </div>
          </>
        ) : null}
      </>
    )

    const fontHint =
      mode === 'editHost'
        ? 'Used for new sessions from this host. Open tabs keep their own copy. “Use default” follows Options → Host defaults.'
        : 'Applies only to this tab. “Use default” follows Options → Host defaults.'

    const resolved = resolveSessionStyle(form, styleDefaults)
    const colorFallback = (
      key: 'tabColor' | 'termBackground' | 'termForeground',
      themeVar: string
    ): string => {
      const fromDefaults = styleDefaults[key]
      if (fromDefaults) {
        return fromDefaults
      }
      return themeVarHex(themeVar)
    }

    const appearanceRows = (
      <>
        <ColorRow
          label="Tab color"
          hint={fontHint}
          value={form.tabColor}
          themeVar={TAB_COLOR_THEME_VAR}
          defaultLabel="Use default"
          resolvedFallback={colorFallback('tabColor', TAB_COLOR_THEME_VAR)}
          onChange={(tabColor) => patch({ tabColor })}
        />
        <ColorRow
          label="Terminal background"
          hint={fontHint}
          value={form.termBackground}
          themeVar={TERM_BG_THEME_VAR}
          defaultLabel="Use default"
          resolvedFallback={colorFallback('termBackground', TERM_BG_THEME_VAR)}
          onChange={(termBackground) => patch({ termBackground })}
        />
        <ColorRow
          label="Terminal text"
          hint={fontHint}
          value={form.termForeground}
          themeVar={TERM_FG_THEME_VAR}
          defaultLabel="Use default"
          resolvedFallback={colorFallback('termForeground', TERM_FG_THEME_VAR)}
          onChange={(termForeground) => patch({ termForeground })}
        />
        <DefaultableControl
          label="Font family"
          hint={fontHint}
          useDefault={!form.fontFamily}
          onUseDefaultChange={(useDefault) =>
            patch({ fontFamily: useDefault ? '' : resolved.fontFamily })
          }
        >
          <select
            value={form.fontFamily || resolved.fontFamily}
            disabled={!form.fontFamily}
            onChange={(e) => patch({ fontFamily: e.target.value })}
          >
            {fontSelectOptions(fontFamilies, form.fontFamily || resolved.fontFamily).map(
              (family) => (
                <option key={family} value={family}>
                  {family}
                </option>
              )
            )}
          </select>
        </DefaultableControl>
        <DefaultableControl
          label="Font size"
          hint={fontHint}
          useDefault={form.fontSizePx === null}
          onUseDefaultChange={(useDefault) =>
            patch({ fontSizePx: useDefault ? null : resolved.fontSizePx })
          }
        >
          <ClampedNumberInput
            min={FONT_SIZE_MIN_PX}
            max={FONT_SIZE_MAX_PX}
            disabled={form.fontSizePx === null}
            value={form.fontSizePx ?? resolved.fontSizePx}
            integer
            onCommit={(fontSizePx) => patch({ fontSizePx })}
          />
        </DefaultableControl>
        <DefaultableControl
          label="Scrollback lines"
          hint={`Primary-buffer history (PuTTY-like). ${fontHint}`}
          useDefault={form.scrollbackLines === null}
          onUseDefaultChange={(useDefault) =>
            patch({ scrollbackLines: useDefault ? null : resolved.scrollbackLines })
          }
        >
          <ClampedNumberInput
            min={SCROLLBACK_LINES_MIN}
            max={SCROLLBACK_LINES_MAX}
            disabled={form.scrollbackLines === null}
            value={form.scrollbackLines ?? resolved.scrollbackLines}
            integer
            onCommit={(scrollbackLines) => patch({ scrollbackLines })}
          />
        </DefaultableControl>
        <DefaultableControl
          label="Bell"
          hint={fontHint}
          useDefault={form.bellMode === null}
          onUseDefaultChange={(useDefault) =>
            patch({ bellMode: useDefault ? null : resolved.bellMode })
          }
        >
          <select
            value={form.bellMode ?? resolved.bellMode}
            disabled={form.bellMode === null}
            onChange={(e) => patch({ bellMode: e.target.value as BellMode })}
          >
            <option value={BELL_MODE_SYSTEM}>Default system sound</option>
            <option value={BELL_MODE_INVERT_WINDOW}>
              Blink whole terminal
            </option>
            <option value={BELL_MODE_INVERT_LINE}>Blink current line</option>
          </select>
        </DefaultableControl>
        <DefaultableControl
          label="Cursor"
          hint={fontHint}
          useDefault={form.cursorStyle === null}
          onUseDefaultChange={(useDefault) =>
            patch({ cursorStyle: useDefault ? null : resolved.cursorStyle })
          }
        >
          <select
            value={form.cursorStyle ?? resolved.cursorStyle}
            disabled={form.cursorStyle === null}
            onChange={(e) => patch({ cursorStyle: e.target.value as CursorStyle })}
          >
            <option value={CURSOR_STYLE_BLOCK}>Block</option>
            <option value={CURSOR_STYLE_UNDERLINE}>Underline</option>
            <option value={CURSOR_STYLE_BAR}>Vertical line</option>
          </select>
        </DefaultableControl>
        <DefaultableControl
          label="Cursor blink"
          hint={fontHint}
          useDefault={form.cursorBlink === null}
          onUseDefaultChange={(useDefault) =>
            patch({ cursorBlink: useDefault ? null : resolved.cursorBlink })
          }
        >
          <input
            type="checkbox"
            disabled={form.cursorBlink === null}
            checked={form.cursorBlink ?? resolved.cursorBlink}
            onChange={(e) => patch({ cursorBlink: e.target.checked })}
          />
        </DefaultableControl>
      </>
    )

    const tunnelHint =
      mode === 'editHost'
        ? 'Applied when a new session connects from this host.'
        : 'Applied on the next connect or reconnect for this tab.'

    const sshEndpoint = isSsh
      ? `${form.username ? `${form.username}@` : ''}${form.host || 'host'}:${form.port}`
      : ''

    const updateTunnel = (id: string, partial: Partial<SshTunnel>): void => {
      patch({
        tunnels: form.tunnels.map((t) => (t.id === id ? { ...t, ...partial } : t))
      })
    }

    const removeTunnel = (id: string): void => {
      patch({ tunnels: form.tunnels.filter((t) => t.id !== id) })
    }

    const tunnelRows = (
      <>
        <div className="settings-row">
          <div className="settings-row-label">
            <strong>X11 forwarding</strong>
            <span>
              Forward remote X11 clients to the local display (DISPLAY / port 6000+N). {tunnelHint}
            </span>
          </div>
          <input
            type="checkbox"
            checked={form.x11Forwarding}
            onChange={(e) => patch({ x11Forwarding: e.target.checked })}
          />
        </div>
        <div className="settings-row settings-row-block">
          <div className="settings-row-label">
            <strong>Tunnels</strong>
            <span>
              Local (L), remote (R), and dynamic SOCKS (D) forwards. {tunnelHint}
            </span>
          </div>
          <div className="settings-tunnel-list">
            {form.tunnels.map((tunnel) => {
              const isDynamic = tunnel.type === TUNNEL_TYPE_DYNAMIC
              return (
                <div key={tunnel.id} className="settings-tunnel-card">
                  <label className="settings-tunnel-enabled">
                    <input
                      type="checkbox"
                      checked={tunnel.enabled}
                      onChange={(e) => updateTunnel(tunnel.id, { enabled: e.target.checked })}
                    />
                    On
                  </label>
                  <select
                    aria-label="Tunnel type"
                    value={tunnel.type}
                    onChange={(e) =>
                      updateTunnel(tunnel.id, { type: e.target.value as TunnelType })
                    }
                  >
                    <option value={TUNNEL_TYPE_LOCAL}>Local</option>
                    <option value={TUNNEL_TYPE_REMOTE}>Remote</option>
                    <option value={TUNNEL_TYPE_DYNAMIC}>Dynamic (SOCKS)</option>
                  </select>
                  <input
                    type="text"
                    aria-label="Listen host"
                    placeholder="Listen host"
                    value={tunnel.listenHost}
                    onChange={(e) => updateTunnel(tunnel.id, { listenHost: e.target.value })}
                  />
                  <input
                    type="number"
                    aria-label="Listen port"
                    placeholder="Port"
                    min={TUNNEL_PORT_MIN}
                    max={TUNNEL_PORT_MAX}
                    value={tunnel.listenPort || ''}
                    onChange={(e) =>
                      updateTunnel(tunnel.id, {
                        listenPort: Number(e.target.value) || 0
                      })
                    }
                  />
                  <input
                    type="text"
                    aria-label="Destination host"
                    placeholder="Dest host"
                    value={tunnel.destHost}
                    disabled={isDynamic}
                    onChange={(e) => updateTunnel(tunnel.id, { destHost: e.target.value })}
                  />
                  <input
                    type="number"
                    aria-label="Destination port"
                    placeholder="Dest port"
                    min={TUNNEL_PORT_MIN}
                    max={TUNNEL_PORT_MAX}
                    value={isDynamic ? '' : tunnel.destPort || ''}
                    disabled={isDynamic}
                    onChange={(e) =>
                      updateTunnel(tunnel.id, {
                        destPort: Number(e.target.value) || 0
                      })
                    }
                  />
                  <button type="button" onClick={() => removeTunnel(tunnel.id)}>
                    Remove
                  </button>
                </div>
              )
            })}
            {showTunnelBuilder ? (
              <TunnelBuilder
                sshEndpoint={sshEndpoint}
                existing={form.tunnels}
                onAdd={(tunnel) => {
                  patch({ tunnels: [...form.tunnels, tunnel] })
                  setShowTunnelBuilder(false)
                }}
                onCancel={() => setShowTunnelBuilder(false)}
              />
            ) : (
              <button
                type="button"
                className="settings-tunnel-add"
                onClick={() => setShowTunnelBuilder(true)}
              >
                Add tunnel
              </button>
            )}
          </div>
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
    if (isSsh) {
      list.push({
        id: 'proxy',
        title: 'Jump host',
        content: proxyRows
      })
      if (mode === 'editHost') {
        list.push({
          id: 'remoteSession',
          title: 'Remote session',
          content: screenRows
        })
      }
      list.push({
        id: 'tunnels',
        title: 'Tunnels',
        content: tunnelRows
      })
    }
    list.push({
      id: 'appearance',
      title: 'Appearance',
      content: appearanceRows
    })

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

    for (const plugin of plugins) {
      if (!plugin.enabled) {
        continue
      }
      const schema = plugin.contributes.hostSettingsSchema
      if (!schema || schema.length === 0) {
        continue
      }
      const values = mergePluginSettings(schema, form.pluginSettings[plugin.id])
      list.push({
        id: `plugin-host-${plugin.id}`,
        title: plugin.contributes.hostSettingsHeading || plugin.name,
        content: (
          <>
            {schema.map((field) => (
              <div key={field.key} className="settings-row">
                <div className="settings-row-label">
                  <strong>{field.label}</strong>
                  {field.description ? <span>{field.description}</span> : null}
                </div>
                <PluginFieldEditor
                  field={field}
                  value={values[field.key]}
                  onChange={(value) => patchPluginHostSetting(plugin.id, field.key, value)}
                />
              </div>
            ))}
          </>
        )
      })
    }

    return list
  }, [
    form,
    fontFamilies,
    identityLocked,
    mode,
    password,
    passphrase,
    tags,
    pickPrivateKey,
    proxyOptions,
    connType,
    isSsh,
    isSerial,
    styleDefaults,
    plugins,
    showTunnelBuilder
  ])

  const buildHostProfile = (): HostProfile => {
    const id =
      form.hostId ||
      ('id' in initial ? initial.id : crypto.randomUUID())
    return {
      id,
      name:
        form.name ||
        (isSerial
          ? form.host
          : form.username
            ? `${form.username}@${form.host}`
            : form.host),
      host: form.host,
      port: form.port,
      username: form.username,
      passwordVaultId: form.passwordVaultId || `pwd-${id}`,
      privateKeyPath: form.privateKeyPath,
      passphraseVaultId: form.passphraseVaultId || (passphrase ? `pp-${id}` : ''),
      authMethod: form.authMethod,
      proxyHostId: isSsh ? form.proxyHostId || '' : '',
      ...protocolConfigFrom(form),
      ...sessionStyleOverridesFrom(form),
      ...tunnelConfigFrom(isSsh ? form : { ...form, tunnels: [], x11Forwarding: false }),
      pluginSettings: normalizeHostPluginSettings(form.pluginSettings),
      reconnectMode: reconnectModeFrom(form),
      ...screenConfigFrom(form),
      tags
    }
  }

  const handleSave = (): void => {
    if (mode === 'editHost') {
      onSaveHost(buildHostProfile(), password, passphrase)
      onClose()
      return
    }

    onSaveSession({
      ...form,
      ...protocolConfigFrom(form),
      ...sessionStyleOverridesFrom(form),
      ...tunnelConfigFrom(form),
      ephemeralPassword: password || form.ephemeralPassword,
      ephemeralPassphrase: passphrase || form.ephemeralPassphrase,
      pluginSettings: normalizeHostPluginSettings(form.pluginSettings),
      reconnectMode: reconnectModeFrom(form),
      ...screenConfigFrom(form)
    })
    onClose()
  }

  const applyToSessions = (): void => {
    if (!onApplyToSessions) {
      return
    }
    onApplyToSessions(buildHostProfile(), password, passphrase)
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
          {mode === 'editHost' && onApplyToSessions ? (
            <button type="button" onClick={applyToSessions}>
              Apply to sessions
            </button>
          ) : null}
          <button type="button" className="primary" onClick={handleSave}>
            Save
          </button>
        </>
      }
    />
  )
}
