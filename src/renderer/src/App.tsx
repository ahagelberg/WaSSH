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
  sessionStyleFrom,
  hostToConnection,
  tunnelConfigFrom,
  protocolConfigFrom,
  hostProfileFromConnection,
  sessionTitle,
  defaultPortForType
} from '@shared/connection'
import SessionsSidebar from './components/SessionsSidebar'
import QuickConnect from './components/QuickConnect'
import TabBar from './components/TabBar'
import TerminalView from './components/TerminalView'
import OptionsDialog from './components/OptionsDialog'
import HostSessionSettingsDialog, {
  type HostSessionMode
} from './components/HostSessionSettingsDialog'

interface TabState {
  id: string
  connection: ConnectionParams
  status: SessionStatus
  statusMessage?: string
  hostKeyPrompt?: HostKeyPrompt
  savePasswordPrompt?: SavePasswordPrompt
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
    ...sessionStyleFrom(null),
    ...tunnelConfigFrom(null)
  }
}

export default function App() {
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS)
  const [hosts, setHosts] = useState<HostProfile[]>([])
  const [tabs, setTabs] = useState<TabState[]>([])
  const [activeTabId, setActiveTabId] = useState<string | null>(null)
  const [showOptions, setShowOptions] = useState(false)
  const [hostEditor, setHostEditor] = useState<{
    mode: HostSessionMode
    initial: ConnectionParams | HostProfile
    connected: boolean
    /** Tab to link when saving a new host from an open session */
    linkTabId?: string
  } | null>(null)
  const [saveAsHostName, setSaveAsHostName] = useState('')
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
    setHosts(list.map((h) => ({ ...h, ...protocolConfigFrom(h), ...sessionStyleFrom(h), ...tunnelConfigFrom(h) })))
  }, [])

  const persistTabs = useCallback(() => {
    const snapshot: TabSnapshot[] = tabsRef.current.map((t) => ({
      id: t.id,
      connection: {
        ...t.connection,
        ...protocolConfigFrom(t.connection),
        ...sessionStyleFrom(t.connection),
        ...tunnelConfigFrom(t.connection),
        ephemeralPassword: '',
        ephemeralPassphrase: ''
      },
      active: t.id === activeRef.current
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
          status: 'connecting' as SessionStatus
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
          ...sessionStyleFrom(t.connection),
          ...tunnelConfigFrom(t.connection)
        },
        status: 'connecting'
      }))
      setTabs(restored)
      const active = snapshot.find((t) => t.active)?.id || snapshot[0]?.id || null
      setActiveTabId(active)
      for (const t of snapshot) {
        void connectTab(t.id, t.connection)
      }
    })()
  }, [refreshHosts, connectTab])

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
    return () => {
      offData()
      offStatus()
      offHostKey()
      offSavePwd()
      offCycle()
      offCloseActive()
      offPrefs()
    }
  }, [refreshHosts, closeTab, cycleTab])

  const updateSettings = (partial: Partial<AppSettings>): void => {
    void window.wassh.setSettings(partial).then(setSettings)
  }

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null

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
          tabs={tabs.map((t) => ({
            id: t.id,
            title: sessionTitle(t.connection),
            status: t.status,
            active: t.id === activeTabId,
            tabColor: sessionStyleFrom(t.connection).tabColor,
            canSaveAsHost: !t.connection.hostId
          }))}
          onSelect={setActiveTabId}
          onClose={closeTab}
          onReorder={reorderTabs}
          onReconnect={(id) => {
            const tab = tabsRef.current.find((t) => t.id === id)
            if (!tab) {
              return
            }
            void window.wassh.disconnect(id)
            void connectTab(id, tab.connection)
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

        <div
          className="terminal-area"
          data-term-bg={
            activeTab
              ? sessionStyleFrom(activeTab.connection).termBackground || undefined
              : undefined
          }
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
          (activeTab.status === 'reconnecting' || activeTab.status === 'failed') ? (
            <div className="inline-banner info">
              <div className="msg">
                {activeTab.status}: {activeTab.statusMessage}
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
              {tabsByStablePaneOrder(tabs).map((t) => (
                <div
                  key={t.id}
                  className={`terminal-pane${t.id === activeTabId ? ' active' : ''}`}
                >
                  <TerminalView
                    tabId={t.id}
                    active={t.id === activeTabId}
                    settings={settings}
                    fontSizePx={sessionStyleFrom(t.connection).fontSizePx}
                    fontFamily={sessionStyleFrom(t.connection).fontFamily}
                    scrollbackLines={sessionStyleFrom(t.connection).scrollbackLines}
                    bellMode={sessionStyleFrom(t.connection).bellMode}
                    cursorStyle={sessionStyleFrom(t.connection).cursorStyle}
                    cursorBlink={sessionStyleFrom(t.connection).cursorBlink}
                    termBackground={sessionStyleFrom(t.connection).termBackground}
                    termForeground={sessionStyleFrom(t.connection).termForeground}
                    onData={onTermData}
                    onResize={onTermResize}
                    registerWriter={registerWriter}
                    unregisterWriter={unregisterWriter}
                  />
                </div>
              ))}
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

      {hostEditor ? (
        <HostSessionSettingsDialog
          mode={hostEditor.mode}
          connected={hostEditor.connected}
          hosts={hosts}
          initial={hostEditor.initial}
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
                          host.passphraseVaultId || t.connection.passphraseVaultId
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
          }}
        />
      ) : null}
    </div>
  )
}
