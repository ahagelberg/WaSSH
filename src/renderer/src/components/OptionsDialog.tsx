import { useEffect, useMemo, useState } from 'react'
import {
  BELL_MODE_INVERT_LINE,
  BELL_MODE_INVERT_WINDOW,
  BELL_MODE_SYSTEM,
  BUNDLED_FONT_FAMILIES,
  CURSOR_STYLE_BAR,
  CURSOR_STYLE_BLOCK,
  CURSOR_STYLE_UNDERLINE,
  FONT_SIZE_MAX_PX,
  FONT_SIZE_MIN_PX,
  SCROLLBACK_LINES_MAX,
  SCROLLBACK_LINES_MIN,
  type AppSettings,
  type AppTheme,
  type BellMode,
  type CursorStyle,
  type SessionStyleDefaults
} from '@shared/types'
import type { PluginListItem } from '@shared/pluginApi'
import { mergePluginSettings, pluginSettingsSectionId } from '@shared/pluginApi'
import { sessionStyleDefaultsFrom } from '@shared/connection'
import SettingsDialog, { type SettingsSection } from './SettingsDialog'
import ClampedNumberInput from './ClampedNumberInput'
import SettingsColorRow from './SettingsColorRow'
import BackupSettingsSection from './BackupSettingsSection'
import { TAB_COLOR_THEME_VAR, TERM_BG_THEME_VAR, TERM_FG_THEME_VAR } from './settingsColor'
import { fontSelectOptions, listMonospaceFontFamilies } from '../fonts'
import PluginSettingsFieldList from '../plugins/api/PluginSettingsFieldList'
import { getPluginSettingsView } from '../plugins/registry'
import DialogShell from './DialogShell'

interface Props {
  activeTabId: string | null
  settings: AppSettings
  onChange: (partial: Partial<AppSettings>) => void
  onClose: () => void
  initialSectionId?: string
  initialFieldKey?: string
}

function optionsPayload(draft: AppSettings): Partial<AppSettings> {
  return {
    reconnectOnStartup: draft.reconnectOnStartup,
    termType: draft.termType,
    theme: draft.theme,
    enabledPlugins: draft.enabledPlugins,
    pluginSettings: draft.pluginSettings,
    sessionStyleDefaults: sessionStyleDefaultsFrom(draft.sessionStyleDefaults)
  }
}

export default function OptionsDialog({
  activeTabId,
  settings,
  onChange,
  onClose,
  initialSectionId,
  initialFieldKey
}: Props) {
  const [draft, setDraft] = useState<AppSettings>(() => ({
    ...settings,
    sessionStyleDefaults: sessionStyleDefaultsFrom(settings.sessionStyleDefaults)
  }))
  const [plugins, setPlugins] = useState<PluginListItem[]>([])
  const [fontFamilies, setFontFamilies] = useState<string[]>(Array.from(BUNDLED_FONT_FAMILIES))
  /** Plugin id whose custom settings editor is open, if any. */
  const [settingsViewPluginId, setSettingsViewPluginId] = useState<string | null>(null)

  const patch = (partial: Partial<AppSettings>): void => {
    setDraft((prev) => ({ ...prev, ...partial }))
  }

  const patchDefaults = (partial: Partial<SessionStyleDefaults>): void => {
    setDraft((prev) => ({
      ...prev,
      sessionStyleDefaults: sessionStyleDefaultsFrom({
        ...prev.sessionStyleDefaults,
        ...partial
      })
    }))
  }

  useEffect(() => {
    document.documentElement.dataset.theme = draft.theme
  }, [draft.theme])

  useEffect(() => {
    void window.wassh.listPlugins().then(setPlugins)
  }, [])

  useEffect(() => {
    let cancelled = false
    void listMonospaceFontFamilies().then((list) => {
      if (!cancelled) {
        setFontFamilies(list)
      }
    })
    return () => {
      cancelled = true
    }
  }, [])

  const handleCancel = (): void => {
    document.documentElement.dataset.theme = settings.theme
    onClose()
  }

  const handleSave = (): void => {
    onChange(optionsPayload(draft))
    onClose()
  }

  const setPluginEnabled = (pluginId: string, enabled: boolean): void => {
    const set = new Set(draft.enabledPlugins)
    if (enabled) {
      set.add(pluginId)
    } else {
      set.delete(pluginId)
    }
    patch({ enabledPlugins: Array.from(set) })
  }

  const patchPluginSetting = (pluginId: string, key: string, value: unknown): void => {
    const current = mergePluginSettings(
      plugins.find((p) => p.id === pluginId)?.contributes.settingsSchema,
      draft.pluginSettings[pluginId]
    )
    patch({
      pluginSettings: {
        ...draft.pluginSettings,
        [pluginId]: { ...current, [key]: value }
      }
    })
  }

  const sections: SettingsSection[] = useMemo(() => {
    const base: SettingsSection[] = [
      {
        id: 'appearance',
        title: 'Appearance',
        content: (
          <>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Theme</strong>
                <span>Applies to the whole app. Font and colors are set per host or session.</span>
              </div>
              <select
                value={draft.theme}
                onChange={(e) => patch({ theme: e.target.value as AppTheme })}
              >
                <option value="dark">Dark</option>
                <option value="light">Light</option>
              </select>
            </div>
          </>
        )
      },
      {
        id: 'general',
        title: 'General',
        content: (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>Reconnect sessions on startup</strong>
              <span>Re-open and reconnect all tabs that were open when WaSSH exited or crashed.</span>
            </div>
            <input
              type="checkbox"
              checked={draft.reconnectOnStartup}
              onChange={(e) => patch({ reconnectOnStartup: e.target.checked })}
            />
          </div>
        )
      },
      {
        id: 'terminal',
        title: 'Terminal',
        content: (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>TERM</strong>
              <span>PTY terminal type sent to the remote host.</span>
            </div>
            <input
              type="text"
              value={draft.termType}
              onChange={(e) => patch({ termType: e.target.value })}
            />
          </div>
        )
      },
      {
        id: 'backup',
        title: 'Backup',
        content: <BackupSettingsSection />
      },
      {
        id: 'host-defaults',
        title: 'Host defaults',
        content: (
          <>
            <p className="plugin-settings-external-note">
              Applied to new hosts and quick connect, and to any host/session field set to “Use
              default”. Changing a value here updates those fields immediately.
            </p>
            <SettingsColorRow
              label="Tab color"
              hint="Theme default follows the app accent."
              value={draft.sessionStyleDefaults.tabColor}
              themeVar={TAB_COLOR_THEME_VAR}
              defaultLabel="Theme default"
              onChange={(tabColor) => patchDefaults({ tabColor })}
            />
            <SettingsColorRow
              label="Terminal background"
              hint="Theme default follows the app terminal background."
              value={draft.sessionStyleDefaults.termBackground}
              themeVar={TERM_BG_THEME_VAR}
              defaultLabel="Theme default"
              onChange={(termBackground) => patchDefaults({ termBackground })}
            />
            <SettingsColorRow
              label="Terminal text"
              hint="The built-in default is fixed light gray; it does not follow the app theme."
              value={draft.sessionStyleDefaults.termForeground}
              themeVar={TERM_FG_THEME_VAR}
              defaultLabel="Theme default"
              onChange={(termForeground) => patchDefaults({ termForeground })}
            />
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Font family</strong>
                <span>Default monospace font for hosts using default.</span>
              </div>
              <select
                value={draft.sessionStyleDefaults.fontFamily}
                onChange={(e) => patchDefaults({ fontFamily: e.target.value })}
              >
                {fontSelectOptions(fontFamilies, draft.sessionStyleDefaults.fontFamily).map(
                  (family) => (
                    <option key={family} value={family}>
                      {family}
                    </option>
                  )
                )}
              </select>
            </div>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Font size</strong>
              </div>
              <ClampedNumberInput
                min={FONT_SIZE_MIN_PX}
                max={FONT_SIZE_MAX_PX}
                value={draft.sessionStyleDefaults.fontSizePx}
                integer
                onCommit={(fontSizePx) => patchDefaults({ fontSizePx })}
              />
            </div>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Scrollback lines</strong>
              </div>
              <ClampedNumberInput
                min={SCROLLBACK_LINES_MIN}
                max={SCROLLBACK_LINES_MAX}
                value={draft.sessionStyleDefaults.scrollbackLines}
                integer
                onCommit={(scrollbackLines) => patchDefaults({ scrollbackLines })}
              />
            </div>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Bell</strong>
              </div>
              <select
                value={draft.sessionStyleDefaults.bellMode}
                onChange={(e) => patchDefaults({ bellMode: e.target.value as BellMode })}
              >
                <option value={BELL_MODE_SYSTEM}>Default system sound</option>
                <option value={BELL_MODE_INVERT_WINDOW}>
                  Blink whole terminal
                </option>
                <option value={BELL_MODE_INVERT_LINE}>Blink current line</option>
              </select>
            </div>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Cursor</strong>
              </div>
              <select
                value={draft.sessionStyleDefaults.cursorStyle}
                onChange={(e) => patchDefaults({ cursorStyle: e.target.value as CursorStyle })}
              >
                <option value={CURSOR_STYLE_BLOCK}>Block</option>
                <option value={CURSOR_STYLE_UNDERLINE}>Underline</option>
                <option value={CURSOR_STYLE_BAR}>Vertical line</option>
              </select>
            </div>
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Cursor blink</strong>
              </div>
              <input
                type="checkbox"
                checked={draft.sessionStyleDefaults.cursorBlink}
                onChange={(e) => patchDefaults({ cursorBlink: e.target.checked })}
              />
            </div>
          </>
        )
      },
      {
        id: 'plugins',
        title: 'Plugins',
        content: (
          <div className="plugin-settings-group">
            <div className="settings-row">
              <div className="settings-row-label">
                <strong>Enable plugins</strong>
                <span>Turn built-in and installed plugins on or off.</span>
              </div>
            </div>
            <div className="plugin-settings-group-children depth-1">
              {plugins.map((p) => (
                <div key={p.id} className="settings-row plugin-settings-row-nested depth-1">
                  <div className="settings-row-label">
                    <strong>
                      {p.name}
                      {p.source === 'external' ? ' (external)' : ''}
                    </strong>
                    <span>{p.description}</span>
                  </div>
                  <input
                    type="checkbox"
                    checked={draft.enabledPlugins.includes(p.id)}
                    onChange={(e) => setPluginEnabled(p.id, e.target.checked)}
                    aria-label={`Enable ${p.name}`}
                  />
                </div>
              ))}
              <p className="plugin-settings-external-note">
                External plugins: none loaded. Future versions can scan userData/plugins/.
              </p>
            </div>
          </div>
        )
      }
    ]

    for (const plugin of plugins) {
      if (!draft.enabledPlugins.includes(plugin.id)) {
        continue
      }
      const schema = plugin.contributes.settingsSchema
      if (!schema || schema.length === 0) {
        continue
      }
      const values = mergePluginSettings(schema, draft.pluginSettings[plugin.id])
      // A plugin that owns a custom settings editor gets a button instead of a
      // schema-driven field list; the host renders its view in a modal.
      const hasSettingsView = Boolean(getPluginSettingsView(plugin.id))
      base.push({
        id: pluginSettingsSectionId(plugin.id),
        title: plugin.contributes.settingsHeading || plugin.name,
        content: hasSettingsView ? (
          <div className="settings-row">
            <div className="settings-row-label">
              <strong>{plugin.name} settings</strong>
              <span>Configured in a dedicated editor.</span>
            </div>
            <button type="button" onClick={() => setSettingsViewPluginId(plugin.id)}>
              Configure…
            </button>
          </div>
        ) : (
          <PluginSettingsFieldList
            schema={schema}
            values={values}
            onChange={(key, value) => patchPluginSetting(plugin.id, key, value)}
          />
        )
      })
    }

    return base
  }, [draft, plugins, fontFamilies])

  const settingsViewPlugin = settingsViewPluginId
    ? plugins.find((p) => p.id === settingsViewPluginId) ?? null
    : null
  const SettingsView = settingsViewPlugin ? getPluginSettingsView(settingsViewPlugin.id) : null

  return (
    <>
      <SettingsDialog
        title="Options"
        sections={sections}
        initialSectionId={initialSectionId}
        initialFieldKey={initialFieldKey}
        onClose={handleCancel}
        footer={
          <>
            <button type="button" onClick={handleCancel}>Cancel</button>
            <button type="button" className="primary" onClick={handleSave}>Save</button>
          </>
        }
      />
      {settingsViewPlugin && SettingsView ? (
        <DialogShell
          titleId="plugin-settings-view-title"
          title={`${settingsViewPlugin.name} settings`}
          onClose={() => setSettingsViewPluginId(null)}
          className="plugin-settings-view-dialog"
        >
          <SettingsView
            tabId={activeTabId}
            settings={mergePluginSettings(
              settingsViewPlugin.contributes.settingsSchema,
              draft.pluginSettings[settingsViewPlugin.id]
            )}
            onSettingsPatch={(partial) => {
              for (const [key, value] of Object.entries(partial)) {
                patchPluginSetting(settingsViewPlugin.id, key, value)
              }
            }}
            onClose={() => setSettingsViewPluginId(null)}
          />
        </DialogShell>
      ) : null}
    </>
  )
}
