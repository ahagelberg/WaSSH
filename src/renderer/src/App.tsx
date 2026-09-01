import { useCallback, useEffect, useRef, useState } from 'react'
import {
  AppSettings,
  BUNDLED_FONT_FAMILIES,
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
import type { TerminalSearchController } from './terminalSearch'
import HostSessionSettingsDialog, {
  type HostSessionMode
} from './components/HostSessionSettingsDialog'
import {
  mergePluginSettings,
  PLUGIN_ID_SFTP,
  type PluginListItem,
  type SftpRendererMessage,
  type SftpStatusPayload
} from '@shared/plugins'
import {
  emptyTabPluginLayout,
  normalizeTabPluginLayout,
  removePluginFromLayout,
  type TabPluginLayout
} from '@shared/pluginLayout'
import PluginToolbar from './plugins/PluginToolbar'
import PluginSessionFrame from './plugins/PluginSessionFrame'

/** Closed session settings retained for Ctrl+Shift+T reopen */
const CLOSED_SESSION_RETENTION_MS = 60 * 60 * 1000

/** Max closed sessions kept for reopen */
const CLOSED_SESSION_STACK_MAX = 20

/** Live progress of a file dropped onto the terminal for SFTP upload */
interface DropUploadState {
  name: string
  transferredBytes: number
  totalBytes: number
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return '0 B'
  }
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const value = bytes / 1024 ** i
  return `${value >= 10 || i === 0 ? Math.round(value) : value.toFixed(1)} ${units[i]}`
}

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

interface ClosedSession {
  closedAt: number
  connection: ConnectionParams
  activePluginIds: string[]
  pluginLayout: TabPluginLayout
  insertIndex: number
}

function pruneClosedSessions(stack: ClosedSession[]): ClosedSession[] {
  const cutoff = Date.now() - CLOSED_SESSION_RETENTION_MS
  return stack.filter((e) => e.closedAt >= cutoff)
}

function cloneClosedSession(tab: TabState, insertIndex: number): ClosedSession {
  return {
    closedAt: Date.now(),
    connection: structuredClone(tab.connection),
    activePluginIds: tab.activePluginIds.slice(),
    pluginLayout: structuredClone(tab.pluginLayout),
    insertIndex
  }
}

function tabsByStablePaneOrder(tabs: TabState[]): TabState[] {
  return tabs.slice().sort((a, b) => {
    if (a.id < b.id) return -1
    if (a.id > b.id) return 1
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
    ...screenConfigFrom(null),
    tags: []
  }
}

export default function App() {
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS)
  const [hosts, setHosts] = useState<HostProfile[]>([])
  const [tabs, setTabs] = useState<TabState[]>([])
  const [activeTabId, setActiveTabId] = useState<string | null>(null)
  const [showOptions, setShowOptions] = useState(false)
  const [showAbout, setShowAbout] = useState(false)
  const [showFind, setShowFind] = useState(false)
  const [findQuery, setFindQuery] = useState('')
  const [findCaseSensitive, setFindCaseSensitive] = useState(false)
  const [findFound, setFindFound] = useState<boolean | null>(null)
  const [findFocusNonce, setFindFocusNonce] = useState(0)
  const [hostEditor, setHostEditor] = useState<{
    mode: HostSessionMode
    initial: ConnectionParams | HostProfile
    connected: boolean
    /** Tab to link when saving a new host from an open session */
    linkTabId?: string
  } | null>(null)
  const [saveAsHostName, setSaveAsHostName] = useState('')
  const [plugins, setPlugins] = useState<PluginListItem[]>([])
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [paletteQuery, setPaletteQuery] = useState('')
  const [paletteIndex, setPaletteIndex] = useState(0)
  const [paletteArgMode, setPaletteArgMode] = useState(false)
  const [paletteArgCommand, setPaletteArgCommand] = useState<{
    id: string
    title: string
    argOptions?: { value: string; label: string }[]
  } | null>(null)
  const [paletteArgValue, setPaletteArgValue] = useState('')
  const [paletteArgIndex, setPaletteArgIndex] = useState(0)
  /** Bumped to move focus back into the terminal after an overlay closes */
  const [termFocusNonce, setTermFocusNonce] = useState(0)

  const writers = useRef<Map<string, (data: string) => void>>(new Map())
  const searchControllers = useRef<Map<string, TerminalSearchController>>(new Map())
  const closedSessionsRef = useRef<ClosedSession[]>([])
  const sftpReadyRef = useRef<Set<string>>(new Set())
  const [, setSftpReadyTick] = useState(0)
  const [dropUploads, setDropUploads] = useState<Record<string, DropUploadState>>({})
  const findQueryRef = useRef(findQuery)
  const findCaseRef = useRef(findCaseSensitive)
  const lastFindQueryRef = useRef('')
  const tabsRef = useRef(tabs)
  const hostsRef = useRef(hosts)
  const activeRef = useRef(activeTabId)
  const settingsRef = useRef(settings)
  const restoredRef = useRef(false)
  const paletteRef = useRef<HTMLDivElement>(null)

  tabsRef.current = tabs
  hostsRef.current = hosts
  activeRef.current = activeTabId
  settingsRef.current = settings
  findQueryRef.current = findQuery
  findCaseRef.current = findCaseSensitive

  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme || DEFAULT_THEME
  }, [settings.theme])

  useEffect(() => {
    lastFindQueryRef.current = ''
    setFindFound(null)
  }, [activeTabId])

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

  const reconnectTab = useCallback(
    async (id: string): Promise<void> => {
      const tab = tabsRef.current.find((t) => t.id === id)
      if (!tab) return
      const restoreIds = tab.activePluginIds.slice()
      await window.wassh.disconnect(id)
      if (restoreIds.length > 0) {
        await window.wassh.queuePluginRestore(id, restoreIds)
      }
      await connectTab(id, tab.connection)
    },
    [connectTab]
  )

  const openSessionSettings = useCallback((id: string): void => {
    const tab = tabsRef.current.find((t) => t.id === id)
    if (!tab) return
    setActiveTabId(id)
    setHostEditor({
      mode: 'editOpenSession',
      initial: tab.connection,
      connected: tab.status === 'connected',
      linkTabId: id
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
      if (restoredRef.current) return
      restoredRef.current = true
      if (!s.reconnectOnStartup) return
      const snapshot = await window.wassh.getTabSnapshot()
      if (snapshot.length === 0) return
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
        if (ids.length > 0) await window.wassh.queuePluginRestore(t.id, ids)
        void connectTab(t.id, t.connection)
      }
    })()
  }, [refreshHosts, refreshPlugins, connectTab])

  const closeTab = useCallback((id: string): void => {
    const list = tabsRef.current
    const idx = list.findIndex((t) => t.id === id)
    if (idx >= 0) {
      const nextClosed = pruneClosedSessions(closedSessionsRef.current)
      nextClosed.push(cloneClosedSession(list[idx], idx))
      while (nextClosed.length > CLOSED_SESSION_STACK_MAX) {
        nextClosed.shift()
      }
      closedSessionsRef.current = nextClosed
    }
    void window.wassh.disconnect(id)
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id)
      if (activeRef.current === id) {
        setActiveTabId(next[next.length - 1]?.id ?? null)
      }
      return next
    })
  }, [])

  const reopenLastClosedSession = useCallback((): void => {
    const stack = pruneClosedSessions(closedSessionsRef.current)
    const entry = stack.pop()
    closedSessionsRef.current = stack
    if (!entry) return
    const id = crypto.randomUUID()
    const restoreIds = entry.activePluginIds.slice()
    setTabs((prev) => {
      const tab: TabState = {
        id,
        connection: entry.connection,
        status: 'connecting',
        activePluginIds: restoreIds,
        pluginLayout: normalizeTabPluginLayout(entry.pluginLayout)
      }
      const next = prev.slice()
      const at = Math.min(Math.max(entry.insertIndex, 0), next.length)
      next.splice(at, 0, tab)
      return next
    })
    setActiveTabId(id)
    void (async () => {
      if (restoreIds.length > 0) await window.wassh.queuePluginRestore(id, restoreIds)
      await connectTab(id, entry.connection)
    })()
  }, [connectTab])

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
    if (list.length === 0) return
    const idx = list.findIndex((t) => t.id === activeRef.current)
    const from = idx < 0 ? 0 : idx
    const nextIndex = (from + delta + list.length) % list.length
    setActiveTabId(list[nextIndex].id)
  }, [])

  const onTermData = useCallback((tabId: string, data: string) => {
    void window.wassh.write(tabId, data)
  }, [])

  useEffect(() => {
    const offData = window.wassh.onSessionData((tabId, data) => {
      writers.current.get(tabId)?.(data)
    })
    const offStatus = window.wassh.onSessionStatus((ev) => {
      if (ev.status === 'closed') {
        closeTab(ev.tabId)
        sftpReadyRef.current.delete(ev.tabId)
        setDropUploads((prev) => {
          if (!(ev.tabId in prev)) return prev
          const next = { ...prev }
          delete next[ev.tabId]
          return next
        })
        return
      }
      setTabs((prev) =>
        prev.map((t) =>
          t.id === ev.tabId ? { ...t, status: ev.status, statusMessage: ev.message } : t
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
      if (id) closeTab(id)
    })
    const offReopenClosed = window.wassh.onReopenClosedTab(() => {
      reopenLastClosedSession()
    })
    const offPrefs = window.wassh.onOpenPreferences(() => {
      setShowOptions(true)
    })
    const offAbout = window.wassh.onOpenAbout(() => {
      setShowAbout(true)
    })
    const offFind = window.wassh.onOpenFind(() => {
      setShowFind(true)
      setFindFocusNonce((n) => n + 1)
    })
    const offReconnectActive = window.wassh.onReconnectActive(() => {
      const id = activeRef.current
      if (id) void reconnectTab(id)
    })
    const offReconnectAll = window.wassh.onReconnectAll(() => {
      for (const tab of tabsRef.current) void reconnectTab(tab.id)
    })
    const offSessionSettings = window.wassh.onOpenSessionSettings(() => {
      const id = activeRef.current
      if (id) openSessionSettings(id)
    })
    const offPluginActive = window.wassh.onPluginActive((ev) => {
      setTabs((prev) =>
        prev.map((t) => {
          if (t.id !== ev.tabId) return t
          const set = new Set(t.activePluginIds)
          if (ev.active) set.add(ev.pluginId)
          else set.delete(ev.pluginId)
          return { ...t, activePluginIds: Array.from(set) }
        })
      )
      if (ev.pluginId === PLUGIN_ID_SFTP && !ev.active) {
        sftpReadyRef.current.delete(ev.tabId)
        setSftpReadyTick((n) => n + 1)
      }
    })
    const offPluginMessage = window.wassh.onPluginMessage((ev) => {
      if (ev.pluginId !== PLUGIN_ID_SFTP) return
      const payload = ev.payload as SftpStatusPayload
      if (payload.type !== 'status') return
      if (payload.state === 'connected') sftpReadyRef.current.add(ev.tabId)
      else sftpReadyRef.current.delete(ev.tabId)
      setSftpReadyTick((n) => n + 1)
    })
    return () => {
      offData()
      offStatus()
      offHostKey()
      offSavePwd()
      offCycle()
      offCloseActive()
      offReopenClosed()
      offPrefs()
      offAbout()
      offFind()
      offReconnectActive()
      offReconnectAll()
      offSessionSettings()
      offPluginActive()
      offPluginMessage()
    }
  }, [refreshHosts, closeTab, cycleTab, reconnectTab, openSessionSettings, reopenLastClosedSession])

  const updateSettings = (partial: Partial<AppSettings>): void => {
    void window.wassh.setSettings(partial).then((next) => {
      setSettings(next)
      void refreshPlugins()
    })
  }

  const onPluginSettingsPatch = useCallback(
    (tabId: string, pluginId: string, partial: Record<string, unknown>) => {
      const plugin = plugins.find((p) => p.id === pluginId)
      const hostSchema = plugin?.contributes.hostSettingsSchema
      const appSchema = plugin?.contributes.settingsSchema
      const hostKeys = new Set((hostSchema ?? []).map((f) => f.key))
      const appKeys = new Set((appSchema ?? []).map((f) => f.key))

      const hostPartial: Record<string, unknown> = {}
      const appPartial: Record<string, unknown> = {}
      for (const [key, value] of Object.entries(partial)) {
        if (hostKeys.has(key)) hostPartial[key] = value
        else if (appKeys.has(key)) appPartial[key] = value
        else hostPartial[key] = value
      }

      if (Object.keys(appPartial).length > 0) {
        const current = mergePluginSettings(
          appSchema,
          settingsRef.current.pluginSettings[pluginId]
        )
        const nextSettings = {
          ...settingsRef.current.pluginSettings,
          [pluginId]: { ...current, ...appPartial }
        }
        setSettings((prev) => ({ ...prev, pluginSettings: nextSettings }))
        void window.wassh.setSettings({ pluginSettings: nextSettings }).then(setSettings)
      }

      if (Object.keys(hostPartial).length === 0) return

      const tab = tabsRef.current.find((t) => t.id === tabId)
      if (!tab) return
      const hostId = tab.connection.hostId
      const nextPluginValues = {
        ...mergePluginSettings(hostSchema, tab.connection.pluginSettings?.[pluginId]),
        ...hostPartial
      }

      const withPluginValues = (connection: ConnectionParams): ConnectionParams => ({
        ...connection,
        pluginSettings: {
          ...connection.pluginSettings,
          [pluginId]: nextPluginValues
        }
      })

      setTabs((prev) =>
        prev.map((t) => {
          if (t.id === tabId || (hostId && t.connection.hostId === hostId)) {
            return { ...t, connection: withPluginValues(t.connection) }
          }
          return t
        })
      )

      for (const t of tabsRef.current) {
        if (t.id === tabId || (hostId && t.connection.hostId === hostId)) {
          void window.wassh.updateConnection(t.id, {
            pluginSettings: withPluginValues(t.connection).pluginSettings
          })
        }
      }

      if (!hostId) return
      const host = hostsRef.current.find((h) => h.id === hostId)
      if (!host) return
      const nextHost: HostProfile = {
        ...host,
        pluginSettings: {
          ...host.pluginSettings,
          [pluginId]: {
            ...mergePluginSettings(hostSchema, host.pluginSettings?.[pluginId]),
            ...hostPartial
          }
        }
      }
      void window.wassh.saveHost(nextHost).then(() => {
        void refreshHosts()
      })
    },
    [plugins, refreshHosts]
  )

  const onPanelLayoutChange = useCallback((tabId: string, pluginLayout: TabPluginLayout) => {
    setTabs((prev) => prev.map((t) => (t.id === tabId ? { ...t, pluginLayout } : t)))
  }, [])

  const togglePlugin = useCallback(async (tabId: string, pluginId: string, nextActive: boolean) => {
    if (nextActive) {
      await window.wassh.activatePlugin(tabId, pluginId)
      return
    }
    setTabs((prev) =>
      prev.map((t) =>
        t.id === tabId
          ? { ...t, pluginLayout: removePluginFromLayout(t.pluginLayout, pluginId) }
          : t
      )
    )
    await window.wassh.deactivatePlugin(tabId, pluginId)
  }, [])

  const uploadDroppedFiles = useCallback(async (tabId: string, files: File[]) => {
    for (const file of files) {
      if (!sftpReadyRef.current.has(tabId) || file.size === 0) continue
      setDropUploads((prev) => ({
        ...prev,
        [tabId]: { name: file.name, transferredBytes: 0, totalBytes: file.size }
      }))
      const send = (payload: SftpRendererMessage): Promise<void> =>
        window.wassh.sendPluginMessage(tabId, PLUGIN_ID_SFTP, payload)
      try {
        await send({ type: 'uploadStart', name: file.name, size: file.size })
        const chunkSize = 256 * 1024
        let offset = 0
        while (offset < file.size) {
          const end = Math.min(offset + chunkSize, file.size)
          const buffer = await file.slice(offset, end).arrayBuffer()
          await send({ type: 'uploadChunk', name: file.name, data: new Uint8Array(buffer) })
          offset = end
          setDropUploads((prev) => {
            const cur = prev[tabId]
            if (!cur) return prev
            return { ...prev, [tabId]: { ...cur, transferredBytes: offset } }
          })
        }
        await send({ type: 'uploadEnd', name: file.name })
      } catch {
        void send({ type: 'cancel' })
      } finally {
        setDropUploads((prev) => {
          if (!(tabId in prev)) return prev
          const next = { ...prev }
          delete next[tabId]
          return next
        })
      }
    }
  }, [])

  const styleDefaults = sessionStyleDefaultsFrom(settings.sessionStyleDefaults)

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null
  const activeStyle = activeTab ? resolveSessionStyle(activeTab.connection, styleDefaults) : null

  const registerWriter = useCallback((tabId: string, write: (data: string) => void) => {
    writers.current.set(tabId, write)
  }, [])
  const unregisterWriter = useCallback((tabId: string) => {
    writers.current.delete(tabId)
  }, [])

  const registerSearch = useCallback((tabId: string, controller: TerminalSearchController) => {
    searchControllers.current.set(tabId, controller)
  }, [])
  const unregisterSearch = useCallback((tabId: string) => {
    searchControllers.current.delete(tabId)
  }, [])

  const runFind = useCallback((direction: 'previous' | 'next'): void => {
    const id = activeRef.current
    const needle = findQueryRef.current
    if (!id || !needle) {
      setFindFound(null)
      return
    }
    const controller = searchControllers.current.get(id)
    if (!controller) {
      setFindFound(null)
      return
    }
    const fromEnd = lastFindQueryRef.current !== needle
    lastFindQueryRef.current = needle
    const options = { caseSensitive: findCaseRef.current, fromEnd }
    const found =
      direction === 'previous'
        ? controller.findPrevious(needle, options)
        : controller.findNext(needle, options)
    setFindFound(found)
  }, [])

  const closeFind = useCallback((): void => {
    setShowFind(false)
    setFindFound(null)
    lastFindQueryRef.current = ''
    const id = activeRef.current
    if (id) searchControllers.current.get(id)?.clear()
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
      if (host.authMethod === 'none') host.authMethod = 'password'
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

  const closePalette = useCallback(() => {
    setPaletteOpen(false)
    setTermFocusNonce((n) => n + 1)
  }, [])

  const openPalette = useCallback(() => {
    setPaletteQuery('')
    setPaletteIndex(0)
    setPaletteArgMode(false)
    setPaletteArgCommand(null)
    setPaletteArgValue('')
    setPaletteOpen(true)
  }, [])

  const pluginArgOptions = plugins.map((p) => ({ value: p.id, label: p.name }))
  const activePluginArgOptions = (
    activeTabId
      ? (tabs.find((t) => t.id === activeTabId)?.activePluginIds ?? [])
          .map((id) => plugins.find((p) => p.id === id))
          .filter((p): p is PluginListItem => Boolean(p))
          .map((p) => ({ value: p.id, label: p.name }))
      : []
  )
  const fontArgOptions = BUNDLED_FONT_FAMILIES.map((f) => ({ value: f, label: f }))

  const paletteCommands = [
    { id: 'open-settings', title: 'Open Settings' },
    { id: 'toggle-theme', title: 'Toggle Dark/Light Theme' },
    { id: 'new-host', title: 'New Host' },
    { id: 'quick-connect', title: 'Quick Connect' },
    { id: 'reconnect-active', title: 'Reconnect Active Tab' },
    { id: 'close-active', title: 'Close Active Tab' },
    { id: 'reopen-last-closed', title: 'Reopen Last Closed Tab' },
    { id: 'open-find', title: 'Open Find in Terminal' },
    { id: 'about', title: 'About' },
    { id: 'run-command', title: 'Run Command in Current Terminal', needsArgument: true },
    {
      id: 'activate-plugin',
      title: 'Activate Plugin',
      needsArgument: true,
      argOptions: pluginArgOptions
    },
    {
      id: 'deactivate-plugin',
      title: 'Deactivate Plugin',
      needsArgument: true,
      argOptions: activePluginArgOptions
    },
    {
      id: 'toggle-plugin',
      title: 'Toggle Plugin',
      needsArgument: true,
      argOptions: pluginArgOptions
    },
    { id: 'set-font-size', title: 'Set Font Size', needsArgument: true },
    {
      id: 'set-font-family',
      title: 'Set Font Family',
      needsArgument: true,
      argOptions: fontArgOptions
    },
    { id: 'set-term-background', title: 'Set Terminal Background Color', needsArgument: true },
    { id: 'set-term-foreground', title: 'Set Terminal Foreground Color', needsArgument: true },
    { id: 'set-tab-accent', title: 'Set Tab Accent Color', needsArgument: true }
  ]

  const filteredPaletteCommands = paletteCommands.filter((cmd) => {
    const q = paletteQuery.trim().toLowerCase()
    if (!q) return true
    const title = cmd.title.toLowerCase()
    if (title.includes(q)) return true
    let i = 0
    for (const ch of title) {
      if (ch === q[i]) i++
      if (i === q.length) return true
    }
    return false
  })

  useEffect(() => {
    setPaletteIndex(0)
  }, [paletteQuery])

  const filteredArgOptions = (paletteArgCommand?.argOptions ?? []).filter((opt) => {
    const q = paletteArgValue.trim().toLowerCase()
    if (!q) return true
    return (
      opt.label.toLowerCase().includes(q) ||
      opt.value.toLowerCase().includes(q)
    )
  })

  useEffect(() => {
    setPaletteArgIndex(0)
  }, [paletteArgValue, paletteArgCommand])

  const executeCommand = useCallback(
    async (commandId: string, rawArgs: string) => {
      const args = rawArgs.trim()
      switch (commandId) {
        case 'open-settings':
          setShowOptions(true)
          break
        case 'toggle-theme':
          updateSettings({ theme: settings.theme === 'dark' ? 'light' : 'dark' })
          break
        case 'new-host':
          setHostEditor({ mode: 'editHost', initial: emptyHost(), connected: false })
          break
        case 'quick-connect':
          updateSettings({ sidebarCollapsed: false })
          break
        case 'reconnect-active':
          if (activeTabId) void reconnectTab(activeTabId)
          break
        case 'close-active':
          if (activeTabId) closeTab(activeTabId)
          break
        case 'reopen-last-closed':
          reopenLastClosedSession()
          break
        case 'open-find':
          setShowFind(true)
          setFindFocusNonce((n) => n + 1)
          break
        case 'about':
          setShowAbout(true)
          break
        case 'run-command':
          if (activeTabId && args) void window.wassh.write(activeTabId, args + '\r')
          break
        case 'activate-plugin':
          if (activeTabId && args) await window.wassh.activatePlugin(activeTabId, args)
          break
        case 'deactivate-plugin':
          if (activeTabId && args) {
            setTabs((prev) =>
              prev.map((t) =>
                t.id === activeTabId
                  ? { ...t, pluginLayout: removePluginFromLayout(t.pluginLayout, args) }
                  : t
              )
            )
            await window.wassh.deactivatePlugin(activeTabId, args)
          }
          break
        case 'toggle-plugin':
          if (activeTabId && args) {
            const tab = tabsRef.current.find((t) => t.id === activeTabId)
            const currentlyActive = tab?.activePluginIds.includes(args) ?? false
            await togglePlugin(activeTabId, args, !currentlyActive)
          }
          break
        case 'set-font-size': {
          const size = Number.parseInt(args, 10)
          if (!Number.isNaN(size) && size >= 8 && size <= 48) {
            updateSettings({
              sessionStyleDefaults: { ...settings.sessionStyleDefaults, fontSizePx: size }
            })
          }
          break
        }
        case 'set-font-family':
          if (args) {
            updateSettings({
              sessionStyleDefaults: { ...settings.sessionStyleDefaults, fontFamily: args }
            })
          }
          break
        case 'set-term-background':
          if (/^#[0-9A-Fa-f]{6}$/.test(args)) {
            updateSettings({
              sessionStyleDefaults: { ...settings.sessionStyleDefaults, termBackground: args }
            })
          }
          break
        case 'set-term-foreground':
          if (/^#[0-9A-Fa-f]{6}$/.test(args)) {
            updateSettings({
              sessionStyleDefaults: { ...settings.sessionStyleDefaults, termForeground: args }
            })
          }
          break
        case 'set-tab-accent':
          if (/^#[0-9A-Fa-f]{6}$/.test(args)) {
            updateSettings({
              sessionStyleDefaults: { ...settings.sessionStyleDefaults, tabColor: args }
            })
          }
          break
      }
      setPaletteOpen(false)
    },
    [settings, activeTabId, updateSettings, reconnectTab, closeTab, reopenLastClosedSession, togglePlugin]
  )

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'p') {
        e.preventDefault()
        openPalette()
        return
      }
      if (e.key === 'F1') {
        e.preventDefault()
        openPalette()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [openPalette])

  return (
    <div className={`app-shell${settings.sidebarCollapsed ? ' sidebar-collapsed' : ''}`}>
      <div className="sidebar-column">
        <SessionsSidebar
          hosts={hosts}
          collapsed={settings.sidebarCollapsed}
          onToggleCollapse={() => updateSettings({ sidebarCollapsed: !settings.sidebarCollapsed })}
          onConnect={(host) => openTab(hostToConnection(host))}
          onEdit={(host) => setHostEditor({ mode: 'editHost', initial: host, connected: false })}
          onDelete={(host) => {
            void window.wassh.deleteHost(host.id).then(refreshHosts)
          }}
          onNewHost={() => setHostEditor({ mode: 'editHost', initial: emptyHost(), connected: false })}
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
            }
          })}
          onSelect={setActiveTabId}
          onClose={closeTab}
          onReorder={reorderTabs}
          onReconnect={(id) => {
            void reconnectTab(id)
          }}
          onConfigure={openSessionSettings}
          onSaveAsHost={(id) => {
            const tab = tabsRef.current.find((t) => t.id === id)
            if (!tab) return
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
              if (!activeTabId) return
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
                    prev.map((t) => (t.id === tabId ? { ...t, hostKeyPrompt: undefined } : t))
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
                    prev.map((t) => (t.id === tabId ? { ...t, hostKeyPrompt: undefined } : t))
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
                const dropUpload = dropUploads[t.id]
                const dropPct =
                  dropUpload && dropUpload.totalBytes > 0
                    ? Math.min(100, (dropUpload.transferredBytes / dropUpload.totalBytes) * 100)
                    : 0
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
                      onPluginSettingsPatch={(pluginId, partial) =>
                        onPluginSettingsPatch(t.id, pluginId, partial)
                      }
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
                        findOpen={showFind}
                        findQuery={findQuery}
                        findCaseSensitive={findCaseSensitive}
                        findFocusNonce={findFocusNonce}
                        findFound={findFound}
                        focusNonce={termFocusNonce}
                        onFindQueryChange={(q) => {
                          setFindQuery(q)
                          setFindFound(null)
                        }}
                        onFindCaseSensitiveChange={(v) => {
                          setFindCaseSensitive(v)
                          lastFindQueryRef.current = ''
                          setFindFound(null)
                        }}
                        onFindPrevious={() => runFind('previous')}
                        onFindNext={() => runFind('next')}
                        onFindClose={closeFind}
                        onData={onTermData}
                        onResize={onTermResize}
                        registerWriter={registerWriter}
                        unregisterWriter={unregisterWriter}
                        registerSearch={registerSearch}
                        unregisterSearch={unregisterSearch}
                        dropEnabled={sftpReadyRef.current.has(t.id)}
                        onDropFiles={(tabId, files) => {
                          void uploadDroppedFiles(tabId, files)
                        }}
                      />
                    </PluginSessionFrame>
                    {dropUpload ? (
                      <div className="drop-upload-banner">
                        <div className="drop-upload-row">
                          <span className="drop-upload-name">⬆ {dropUpload.name}</span>
                          <span className="drop-upload-meta">
                            {formatBytes(dropUpload.transferredBytes)} /{' '}
                            {formatBytes(dropUpload.totalBytes)}
                          </span>
                        </div>
                        <div className="drop-upload-track">
                          <div className="drop-upload-fill" style={{ width: `${dropPct}%` }} />
                        </div>
                      </div>
                    ) : null}
                  </div>
                )
              })}
            </div>
          )}
        </div>
      </div>

      {paletteOpen ? (
        <div
          className="command-palette-overlay"
          onClick={closePalette}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              e.preventDefault()
              if (paletteArgMode) {
                setPaletteArgMode(false)
                setPaletteArgCommand(null)
                setPaletteArgValue('')
              } else {
                closePalette()
              }
              return
            }
            if (e.key === 'ArrowDown') {
              e.preventDefault()
              if (paletteArgMode) {
                if (filteredArgOptions.length > 0) {
                  setPaletteArgIndex((i) => Math.min(i + 1, filteredArgOptions.length - 1))
                }
              } else if (filteredPaletteCommands.length > 0) {
                setPaletteIndex((i) => Math.min(i + 1, filteredPaletteCommands.length - 1))
              }
              return
            }
            if (e.key === 'ArrowUp') {
              e.preventDefault()
              if (paletteArgMode) {
                setPaletteArgIndex((i) => Math.max(i - 1, 0))
              } else {
                setPaletteIndex((i) => Math.max(i - 1, 0))
              }
              return
            }
            if (e.key === 'Enter') {
              e.preventDefault()
              if (paletteArgMode) {
                if (paletteArgCommand) {
                  if (paletteArgCommand.argOptions) {
                    const opt = filteredArgOptions[paletteArgIndex]
                    if (opt) {
                      executeCommand(paletteArgCommand.id, opt.value)
                    }
                  } else {
                    executeCommand(paletteArgCommand.id, paletteArgValue)
                  }
                }
                return
              }
              const cmd = filteredPaletteCommands[paletteIndex]
              if (cmd) {
                if (cmd.needsArgument) {
                  setPaletteArgCommand({ id: cmd.id, title: cmd.title, argOptions: cmd.argOptions })
                  setPaletteArgValue('')
                  setPaletteArgMode(true)
                } else {
                  executeCommand(cmd.id, '')
                }
              }
            }
          }}
        >
          <div
            className="command-palette"
            ref={paletteRef}
            onMouseDown={(e) => e.preventDefault()}
            onClick={(e) => e.stopPropagation()}
            onBlur={(e) => {
              if (!paletteRef.current?.contains(e.relatedTarget as Node)) {
                closePalette()
              }
            }}
          >
            <div className={paletteArgMode ? 'command-palette-arg-form' : 'command-palette-search'}>
              {paletteArgMode && paletteArgCommand ? (
                <span className="command-palette-arg-label">{paletteArgCommand.title}</span>
              ) : null}
              <input
                className={paletteArgMode ? 'command-palette-arg-input' : 'command-palette-input'}
                placeholder={
                  paletteArgMode
                    ? paletteArgCommand?.argOptions
                      ? 'Filter…'
                      : 'Enter value…'
                    : 'Type a command…'
                }
                autoFocus
                value={paletteArgMode ? paletteArgValue : paletteQuery}
                onChange={(e) => {
                  if (paletteArgMode) setPaletteArgValue(e.target.value)
                  else setPaletteQuery(e.target.value)
                }}
              />
            </div>
            {paletteArgMode ? (
              paletteArgCommand?.argOptions ? (
                <div className="command-palette-list">
                  {filteredArgOptions.map((opt, idx) => (
                    <div
                      key={opt.value}
                      className={`command-palette-item${idx === paletteArgIndex ? ' selected' : ''}`}
                      onMouseEnter={() => setPaletteArgIndex(idx)}
                      onClick={() => executeCommand(paletteArgCommand.id, opt.value)}
                    >
                      {opt.label}
                    </div>
                  ))}
                  {filteredArgOptions.length === 0 ? (
                    <div className="command-palette-empty">No matching options</div>
                  ) : null}
                </div>
              ) : null
            ) : (
              <div className="command-palette-list">
                {filteredPaletteCommands.map((cmd, idx) => (
                  <div
                    key={cmd.id}
                    className={`command-palette-item${idx === paletteIndex ? ' selected' : ''}`}
                    onMouseEnter={() => setPaletteIndex(idx)}
                    onClick={() => {
                      if (cmd.needsArgument) {
                        setPaletteArgCommand({ id: cmd.id, title: cmd.title, argOptions: cmd.argOptions })
                        setPaletteArgValue('')
                        setPaletteArgMode(true)
                      } else {
                        executeCommand(cmd.id, '')
                      }
                    }}
                  >
                    {cmd.title}
                    {cmd.needsArgument ? (
                      <span className="command-palette-requires-arg">…</span>
                    ) : null}
                  </div>
                ))}
                {filteredPaletteCommands.length === 0 ? (
                  <div className="command-palette-empty">No matching commands</div>
                ) : null}
              </div>
            )}
          </div>
        </div>
      ) : null}

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
            if (!linkTabId) return
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
            if (!tabId) return
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
