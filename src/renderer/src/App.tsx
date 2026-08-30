import { useCallback, useEffect, useRef, useState } from 'react'
import {
  AppSettings,
  ConnectionParams,
  DEFAULT_SETTINGS,
  DEFAULT_TERM_COLS,
  DEFAULT_TERM_ROWS,
  DEFAULT_THEME,
  HostKeyPrompt,
  HostProfile,
  SavePasswordPrompt,
  SessionStatus,
  TAB_SNAPSHOT_DEBOUNCE_MS,
  TabSnapshot
} from '@shared/types'
import {
  defaultPortForType,
  hostToConnection,
  hostProfileFromConnection,
  protocolConfigFrom,
  reconnectModeFrom,
  resolveSessionStyle,
  screenConfigFrom,
  sessionStyleDefaultsFrom,
  sessionStyleOverridesFrom,
  emptySessionStyleOverrides,
  sessionTitle,
  tunnelConfigFrom
} from '@shared/connection'
import SessionsSidebar from './components/SessionsSidebar'
import QuickConnect from './components/QuickConnect'
import TabBar from './components/TabBar'
import TerminalView from './components/TerminalView'
import AboutDialog from './components/AboutDialog'
import OptionsDialog from './components/OptionsDialog'
import HostSessionSettingsDialog, {
  type HostSessionMode
} from './components/HostSessionSettingsDialog'
import type { PluginListItem } from '@shared/plugins'
import { mergePluginSettings } from '@shared/plugins'
import {
  emptyTabPluginLayout,
  normalizeTabPluginLayout,
  type TabPluginLayout
} from '@shared/pluginLayout'
import PluginToolbar from './plugins/PluginToolbar'
import PluginSessionFrame from './plugins/PluginSessionFrame'

interface TabState {
  id: string
  connection: ConnectionParams
  status: SessionStatus
  statusMessage?: string
  hostKeyPrompt?: HostKeyPrompt
  savePasswordPrompt?: SavePasswordPrompt
  activePluginIds: string[]
  pluginLayout: TabPluginLayout
}

function tabsByStablePaneOrder(tabs: TabState[]): TabState[] {
  return tabs.slice().sort((a, b) => {
    if (a.id < b.id) {
      return -1
    }
    if (a.id > b.id) {
      return 1
    }
    return 0
  })
}

function emptyHost(): HostProfile {
  const proto = protocolConfigFrom(null)
  return {
    id: crypto.randomUUID(),
    name: '',
    host: '',
    port: defaultPortForType(proto.connectionType),
    username: '',
    passwordVaultId: '',
    privateKeyPath: '',
    passphraseVaultId: '',
    authMethod: 'none',
    proxyHostId: '',
    ...proto,
    ...emptySessionStyleOverrides(),
    ...tunnelConfigFrom(null),
    pluginSettings: {},
    reconnectMode: reconnectModeFrom(null),
    ...screenConfigFrom(null)
  }
}

export default function App() {
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS)
  const [hosts, setHosts] = useState<HostProfile[]>([])
  const [tabs, setTabs] = useState<TabState[]>([])
  const [activeTabId, setActiveTabId] = useState<string | null>(null)
  const [showOptions, setShowOptions] = useState(false)
  const [showAbout, setShowAbout] = useState(false)
  const [hostEditor, setHostEditor] = useState<{
    mode: HostSessionMode
    initial: ConnectionParams | HostProfile
    connected: boolean
    /** Tab to link when saving a new host from an open session */
    linkTabId?: string
  } | null>(null)
  const [saveAsHostName, setSaveAsHostName] = useState('')
  const [plugins, setPlugins] = useState<PluginListItem[]>([])
  const writers = useRef<Map<string, (data: string) => void>>(new Map())
  const tabsRef = useRef(tabs)
  const activeRef = useRef(activeTabId)
  const settingsRef = useRef(settings)
  const restoredRef = useRef(false)

  tabsRef.current = tabs
  activeRef.current = activeTabId
  settingsRef.current = settings

  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme || DEFAULT_THEME
  }, [settings.theme])

  const refreshHosts = useCallback(async () => {
    const list = await window.wassh.listHosts()
    setHosts(
      list.map((h) => ({
        ...h,
        ...protocolConfigFrom(h),
        ...sessionStyleOverridesFrom(h),
        ...tunnelConfigFrom(h),
        pluginSettings: h.pluginSettings ?? {},
        reconnectMode: reconnectModeFrom(h),
        ...screenConfigFrom(h)
      }))
    )
  }, [])

  const refreshPlugins = useCallback(async () => {
    const list = await window.wassh.listPlugins()
    setPlugins(list)
  }, [])

  const persistTabs = useCallback(() => {
    const snapshot: TabSnapshot[] = tabsRef.current.map((t) => ({
      id: t.id,
      connection: {
        ...t.connection,
        ...protocolConfigFrom(t.connection),
        ...sessionStyleOverridesFrom(t.connection),
        ...tunnelConfigFrom(t.connection),
        ephemeralPassword: '',
        ephemeralPassphrase: '',
        pluginSettings: t.connection.pluginSettings ?? {},
        reconnectMode: reconnectModeFrom(t.connection),
        ...screenConfigFrom(t.connection)
      },
      active: t.id === activeRef.current,
      activePluginIds: t.activePluginIds,
      pluginLayout: t.pluginLayout
    }))
    void window.wassh.saveTabSnapshot(snapshot)
  }, [])

  useEffect(() => {
    const timer = setTimeout(persistTabs, TAB_SNAPSHOT_DEBOUNCE_MS)
    return () => clearTimeout(timer)
  }, [tabs, activeTabId, persistTabs])

  const connectTab = useCallback(async (tabId: string, connection: ConnectionParams) => {
    const s = settingsRef.current
    await window.wassh.connect({
      tabId,
      connection,
      cols: DEFAULT_TERM_COLS,
      rows: DEFAULT_TERM_ROWS,
      termType: s.termType
    })
  }, [])

  const openTab = useCallback(
    (connection: ConnectionParams, existingId?: string) => {
      const id = existingId || crypto.randomUUID()
      setTabs((prev) => [
        ...prev,
        {
          id,
          connection,
          status: 'connecting' as SessionStatus,
          activePluginIds: [],
          pluginLayout: emptyTabPluginLayout()
        }
      ])
      setActiveTabId(id)
      void connectTab(id, connection)
      return id
    },
    [connectTab]
  )

  useEffect(() => {
    void (async () => {
      const s = await window.wassh.getSettings()
      setSettings(s)
      await refreshHosts()
      await refreshPlugins()
      if (restoredRef.current) {
        return
      }
      restoredRef.current = true
      if (!s.reconnectOnStartup) {
        return
      }
      const snapshot = await window.wassh.getTabSnapshot()
      if (snapshot.length === 0) {
        return
      }
      const restored: TabState[] = snapshot.map((t) => ({
        id: t.id,
        connection: {
          ...t.connection,
          ...protocolConfigFrom(t.connection),
          ...sessionStyleOverridesFrom(t.connection),
          ...tunnelConfigFrom(t.connection),
          reconnectMode: reconnectModeFrom(t.connection),
          ...screenConfigFrom(t.connection)
        },
        status: 'connecting',
        activePluginIds: Array.isArray(t.activePluginIds) ? t.activePluginIds : [],
        pluginLayout: normalizeTabPluginLayout(t.pluginLayout)
      }))
      setTabs(restored)
      const active = snapshot.find((t) => t.active)?.id || snapshot[0]?.id || null
      setActiveTabId(active)
      for (const t of restored) {
        const ids = t.activePluginIds
        if (ids.length > 0) {
          await window.wassh.queuePluginRestore(t.id, ids)
        }
        void connectTab(t.id, t.connection)
      }
    })()
  }, [refreshHosts, refreshPlugins, connectTab])

  const closeTab = useCallback((id: string): void => {
    void window.wassh.disconnect(id)
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id)
      if (activeRef.current === id) {
        setActiveTabId(next[next.length - 1]?.id ?? null)
      }
      return next
    })
  }, [])

  const reorderTabs = useCallback((fromId: string, insertIndex: number): void => {
    setTabs((prev) => {
      const from = prev.findIndex((t) => t.id === fromId)
      if (from < 0 || insertIndex < 0 || insertIndex >= prev.length || from === insertIndex) {
        return prev
      }
      const next = prev.slice()
      const [item] = next.splice(from, 1)
      next.splice(insertIndex, 0, item)
      return next
    })
  }, [])

  const cycleTab = useCallback((delta: number): void => {
    const list = tabsRef.current
    if (list.length === 0) {
      return
    }
    const idx = list.findIndex((t) => t.id === activeRef.current)
    const from = idx < 0 ? 0 : idx
    const nextIndex = (from + delta + list.length) % list.length
    setActiveTabId(list[nextIndex].id)
  }, [])

  useEffect(() => {
    const offData = window.wassh.onSessionData((tabId, data) => {
      writers.current.get(tabId)?.(data)
    })
    const offStatus = window.wassh.onSessionStatus((ev) => {
      if (ev.status === 'closed') {
        closeTab(ev.tabId)
        return
      }
      setTabs((prev) =>
        prev.map((t) =>
          t.id === ev.tabId
            ? { ...t, status: ev.status, statusMessage: ev.message }
            : t
        )
      )
      if (ev.status === 'connected') {
        void refreshHosts()
      }
    })
    const offHostKey = window.wassh.onHostKeyPrompt((prompt) => {
      setTabs((prev) =>
        prev.map((t) => (t.id === prompt.tabId ? { ...t, hostKeyPrompt: prompt } : t))
      )
    })
    const offSavePwd = window.wassh.onSavePasswordPrompt((prompt) => {
      setTabs((prev) =>
        prev.map((t) => (t.id === prompt.tabId ? { ...t, savePasswordPrompt: prompt } : t))
      )
      if (!prompt.hasHostProfile) {
        const tab = tabsRef.current.find((t) => t.id === prompt.tabId)
        setSaveAsHostName(tab ? sessionTitle(tab.connection) : '')
      }
    })
    const offCycle = window.wassh.onCycleTab(cycleTab)
    const offCloseActive = window.wassh.onCloseActiveTab(() => {
      const id = activeRef.current
      if (id) {
        closeTab(id)
      }
    })
    const offPrefs = window.wassh.onOpenPreferences(() => {
      setShowOptions(true)
    })
    const offAbout = window.wassh.onOpenAbout(() => {
      setShowAbout(true)
    })
    const offPluginActive = window.wassh.onPluginActive((ev) => {
      setTabs((prev) =>
        prev.map((t) => {
          if (t.id !== ev.tabId) {
            return t
          }
          const set = new Set(t.activePluginIds)
          if (ev.active) {
            set.add(ev.pluginId)
          } else {
            set.delete(ev.pluginId)
          }
          return { ...t, activePluginIds: Array.from(set) }
        })
      )
    })
    return () => {
      offData()
      offStatus()
      offHostKey()
      offSavePwd()
      offCycle()
      offCloseActive()
      offPrefs()
      offAbout()
      offPluginActive()
    }
  }, [refreshHosts, closeTab, cycleTab])

  const updateSettings = (partial: Partial<AppSettings>): void => {
    void window.wassh.setSettings(partial).then((next) => {
      setSettings(next)
      void refreshPlugins()
    })
  }

  const onPluginSettingsPatch = useCallback(
    (pluginId: string, partial: Record<string, unknown>) => {
      const plugin = plugins.find((p) => p.id === pluginId)
      const current = mergePluginSettings(
        plugin?.contributes.settingsSchema,
        settingsRef.current.pluginSettings[pluginId]
      )
      const nextSettings = {
        ...settingsRef.current.pluginSettings,
        [pluginId]: { ...current, ...partial }
      }
      setSettings((prev) => ({ ...prev, pluginSettings: nextSettings }))
      void window.wassh.setSettings({ pluginSettings: nextSettings }).then(setSettings)
    },
    [plugins]
  )

  const onPanelLayoutChange = useCallback((tabId: string, pluginLayout: TabPluginLayout) => {
    setTabs((prev) => prev.map((t) => (t.id === tabId ? { ...t, pluginLayout } : t)))
  }, [])

  const togglePlugin = useCallback(async (tabId: string, pluginId: string, nextActive: boolean) => {
    if (nextActive) {
      await window.wassh.activatePlugin(tabId, pluginId)
    } else {
      await window.wassh.deactivatePlugin(tabId, pluginId)
    }
  }, [])

  const styleDefaults = sessionStyleDefaultsFrom(settings.sessionStyleDefaults)

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null
  const activeStyle = activeTab
    ? resolveSessionStyle(activeTab.connection, styleDefaults)
    : null

  const registerWriter = useCallback((tabId: string, write: (data: string) => void) => {
    writers.current.set(tabId, write)
  }, [])
  const unregisterWriter = useCallback((tabId: string) => {
    writers.current.delete(tabId)
  }, [])

  const onTermData = useCallback((tabId: string, data: string) => {
    void window.wassh.write(tabId, data)
  }, [])

  const onTermResize = useCallback((tabId: string, cols: number, rows: number) => {
    void window.wassh.resize(tabId, cols, rows)
  }, [])

  const saveHost = async (
    host: HostProfile,
    password: string,
    passphrase: string
  ): Promise<void> => {
    if (password) {
      const vid = host.passwordVaultId || `pwd-${host.id}`
      await window.wassh.setSecret(vid, password)
      host.passwordVaultId = vid
      if (host.authMethod === 'none') {
        host.authMethod = 'password'
      }
    }
    if (passphrase) {
      const vid = host.passphraseVaultId || `pp-${host.id}`
      await window.wassh.setSecret(vid, passphrase)
      host.passphraseVaultId = vid
    }
    await window.wassh.saveHost(host)
    await refreshHosts()
    setTabs((prev) =>
      prev.map((t) =>
        t.connection.hostId === host.id
          ? {
              ...t,
              connection: {
                ...t.connection,
                pluginSettings: host.pluginSettings,
                reconnectMode: host.reconnectMode,
                ...screenConfigFrom(host)
              }
            }
          : t
      )
    )
    for (const t of tabsRef.current) {
      if (t.connection.hostId === host.id) {
        void window.wassh.updateConnection(t.id, {
          pluginSettings: host.pluginSettings,
          reconnectMode: host.reconnectMode,
          ...screenConfigFrom(host)
        })
      }
    }
  }

  return (
    <div className={`app-shell${settings.sidebarCollapsed ? ' sidebar-collapsed' : ''}`}>
      <div className="sidebar-column">
        <SessionsSidebar
          hosts={hosts}
          collapsed={settings.sidebarCollapsed}
          onToggleCollapse={() =>
            updateSettings({ sidebarCollapsed: !settings.sidebarCollapsed })
          }
          onConnect={(host) => openTab(hostToConnection(host))}
          onEdit={(host) =>
            setHostEditor({ mode: 'editHost', initial: host, connected: false })
          }
          onDelete={(host) => {
            void window.wassh.deleteHost(host.id).then(refreshHosts)
          }}
          onNewHost={() =>
            setHostEditor({ mode: 'editHost', initial: emptyHost(), connected: false })
          }
        />
        {!settings.sidebarCollapsed ? <QuickConnect onConnect={(c) => openTab(c)} /> : null}
      </div>

      <div className="main-pane">
        <TabBar
          tabs={tabs.map((t) => {
            const style = resolveSessionStyle(t.connection, styleDefaults)
            return {
            id: t.id,
            title: sessionTitle(t.connection),
            status: t.status,
            active: t.id === activeTabId,
            tabColor: style.tabColor,
            canSaveAsHost: !t.connection.hostId
          }})}
          onSelect={setActiveTabId}
          onClose={closeTab}
          onReorder={reorderTabs}
          onReconnect={(id) => {
            const tab = tabsRef.current.find((t) => t.id === id)
            if (!tab) {
              return
            }
            void (async () => {
              const restoreIds = tab.activePluginIds.slice()
              await window.wassh.disconnect(id)
              if (restoreIds.length > 0) {
                await window.wassh.queuePluginRestore(id, restoreIds)
              }
              await connectTab(id, tab.connection)
            })()
          }}
          onConfigure={(id) => {
            const tab = tabsRef.current.find((t) => t.id === id)
            if (!tab) {
              return
            }
            setActiveTabId(id)
            setHostEditor({
              mode: 'editOpenSession',
              initial: tab.connection,
              connected: tab.status === 'connected',
              linkTabId: id
            })
          }}
          onSaveAsHost={(id) => {
            const tab = tabsRef.current.find((t) => t.id === id)
            if (!tab) {
              return
            }
            setActiveTabId(id)
            setHostEditor({
              mode: 'editHost',
              initial: {
                ...hostProfileFromConnection(tab.connection, crypto.randomUUID()),
                name: sessionTitle(tab.connection),
                passwordVaultId: '',
                passphraseVaultId: ''
              },
              connected: false,
              linkTabId: id
            })
          }}
        />

        {tabs.length > 0 ? (
          <PluginToolbar
            plugins={plugins}
            activePluginIds={activeTab?.activePluginIds ?? []}
            disabled={!activeTabId}
            onToggle={(pluginId, nextActive) => {
              if (!activeTabId) {
                return
              }
              void togglePlugin(activeTabId, pluginId, nextActive)
            }}
          />
        ) : null}

        <div
          className="terminal-area"
          data-term-bg={activeStyle?.termBackground || undefined}
        >
          {activeTab?.hostKeyPrompt ? (
            <div className="inline-banner warn">
              <div className="msg">
                {activeTab.hostKeyPrompt.reason === 'mismatch'
                  ? 'WARNING: remote host key does not match the cached key!'
                  : 'The server host key is not cached.'}{' '}
                <code>{activeTab.hostKeyPrompt.fingerprint}</code> —{' '}
                {activeTab.hostKeyPrompt.host}:{activeTab.hostKeyPrompt.port}
              </div>
              <button
                type="button"
                className="primary"
                onClick={() => {
                  const tabId = activeTab.id
                  void window.wassh.respondHostKey(tabId, 'accept')
                  setTabs((prev) =>
                    prev.map((t) =>
                      t.id === tabId ? { ...t, hostKeyPrompt: undefined } : t
                    )
                  )
                }}
              >
                Accept
              </button>
              <button
                type="button"
                className="danger"
                onClick={() => {
                  const tabId = activeTab.id
                  void window.wassh.respondHostKey(tabId, 'reject')
                  setTabs((prev) =>
                    prev.map((t) =>
                      t.id === tabId ? { ...t, hostKeyPrompt: undefined } : t
                    )
                  )
                }}
              >
                Reject
              </button>
            </div>
          ) : null}

          {activeTab?.savePasswordPrompt ? (
            <div className="inline-banner info">
              <div className="msg">Save password to this host?</div>
              {!activeTab.savePasswordPrompt.hasHostProfile ? (
                <input
                  value={saveAsHostName}
                  onChange={(e) => setSaveAsHostName(e.target.value)}
                  placeholder="Host name"
                />
              ) : null}
              <button
                type="button"
                className="primary"
                onClick={() => {
                  const tabId = activeTab.id
                  const decision = activeTab.savePasswordPrompt?.hasHostProfile
                    ? 'save'
                    : 'save_as_host'
                  void window.wassh
                    .respondSavePassword(tabId, decision, saveAsHostName)
                    .then(refreshHosts)
                  setTabs((prev) =>
                    prev.map((t) =>
                      t.id === tabId ? { ...t, savePasswordPrompt: undefined } : t
                    )
                  )
                }}
              >
                Yes
              </button>
              <button
                type="button"
                onClick={() => {
                  const tabId = activeTab.id
                  void window.wassh.respondSavePassword(tabId, 'skip')
                  setTabs((prev) =>
                    prev.map((t) =>
                      t.id === tabId ? { ...t, savePasswordPrompt: undefined } : t
                    )
                  )
                }}
              >
                No
              </button>
            </div>
          ) : null}

          {activeTab?.statusMessage &&
          (activeTab.status === 'reconnecting' ||
            activeTab.status === 'failed' ||
            activeTab.status === 'connected') ? (
            <div
              className={`inline-banner ${activeTab.status === 'connected' ? 'warn' : 'info'}`}
            >
              <div className="msg">
                {activeTab.status === 'connected'
                  ? activeTab.statusMessage
                  : `${activeTab.status}: ${activeTab.statusMessage}`}
              </div>
            </div>
          ) : null}

          {tabs.length === 0 ? (
            <div className="empty-state">
              <div>
                <h2>WaSSH</h2>
                <p>Quick-connect from the sidebar or open a saved host.</p>
              </div>
            </div>
          ) : (
            <div className="terminal-stack">
              {tabsByStablePaneOrder(tabs).map((t) => {
                const style = resolveSessionStyle(t.connection, styleDefaults)
                return (
                <div
                  key={t.id}
                  className={`terminal-pane${t.id === activeTabId ? ' active' : ''}`}
                >
                  <PluginSessionFrame
                    tabId={t.id}
                    active={t.id === activeTabId}
                    plugins={plugins}
                    activePluginIds={t.activePluginIds}
                    layout={t.pluginLayout}
                    settings={settings}
                    hostPluginSettings={t.connection.pluginSettings ?? {}}
                    onPluginSettingsPatch={onPluginSettingsPatch}
                    onLayoutChange={(pluginLayout) => onPanelLayoutChange(t.id, pluginLayout)}
                    onDeactivatePlugin={(pluginId) => {
                      void togglePlugin(t.id, pluginId, false)
                    }}
                  >
                    <TerminalView
                      tabId={t.id}
                      active={t.id === activeTabId}
                      settings={settings}
                      fontSizePx={style.fontSizePx}
                      fontFamily={style.fontFamily}
                      scrollbackLines={style.scrollbackLines}
                      bellMode={style.bellMode}
                      cursorStyle={style.cursorStyle}
                      cursorBlink={style.cursorBlink}
                      termBackground={style.termBackground}
                      termForeground={style.termForeground}
                      onData={onTermData}
                      onResize={onTermResize}
                      registerWriter={registerWriter}
                      unregisterWriter={unregisterWriter}
                    />
                  </PluginSessionFrame>
                </div>
              )})}
            </div>
          )}
        </div>
      </div>

      {showOptions ? (
        <OptionsDialog
          settings={settings}
          onChange={updateSettings}
          onClose={() => setShowOptions(false)}
        />
      ) : null}

      {showAbout ? <AboutDialog onClose={() => setShowAbout(false)} /> : null}

      {hostEditor ? (
        <HostSessionSettingsDialog
          mode={hostEditor.mode}
          connected={hostEditor.connected}
          hosts={hosts}
          initial={hostEditor.initial}
          styleDefaults={styleDefaults}
          pickPrivateKey={() => window.wassh.pickPrivateKeyFile()}
          onClose={() => setHostEditor(null)}
          onSaveHost={(host, password, passphrase) => {
            const linkTabId = hostEditor.linkTabId
            void saveHost(host, password, passphrase)
            if (!linkTabId) {
              return
            }
            setTabs((prev) =>
              prev.map((t) =>
                t.id === linkTabId && !t.connection.hostId
                  ? {
                      ...t,
                      connection: {
                        ...t.connection,
                        hostId: host.id,
                        passwordVaultId: host.passwordVaultId || t.connection.passwordVaultId,
                        passphraseVaultId:
                          host.passphraseVaultId || t.connection.passphraseVaultId,
                        pluginSettings: host.pluginSettings,
                        reconnectMode: host.reconnectMode,
                        ...screenConfigFrom(host)
                      }
                    }
                  : t
              )
            )
          }}
          onSaveSession={(connection) => {
            const tabId = hostEditor.linkTabId
            if (!tabId) {
              return
            }
            setTabs((prev) =>
              prev.map((t) => (t.id === tabId ? { ...t, connection } : t))
            )
            void window.wassh.updateConnection(tabId, {
              pluginSettings: connection.pluginSettings,
              name: connection.name,
              reconnectMode: connection.reconnectMode,
              ...sessionStyleOverridesFrom(connection),
              ...tunnelConfigFrom(connection)
            })
            if (connection.hostId) {
              const existing = hosts.find((h) => h.id === connection.hostId)
              if (existing) {
                void saveHost(
                  {
                    ...existing,
                    ...sessionStyleOverridesFrom(connection),
                    ...tunnelConfigFrom(connection),
                    pluginSettings: connection.pluginSettings,
                    reconnectMode: connection.reconnectMode
                  },
                  '',
                  ''
                )
              }
            }
          }}
        />
      ) : null}
    </div>
  )
}
