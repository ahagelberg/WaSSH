import { randomUUID } from 'crypto'
import { app } from 'electron'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs'
import { join } from 'path'
import {
  AppSettings,
  DEFAULT_RECONNECT_MODE,
  DEFAULT_SETTINGS,
  DEFAULT_SSH_PORT,
  DEFAULT_THEME,
  HostProfile,
  KnownHostEntry,
  RECONNECT_MODE_NONE,
  TabSnapshot,
  type AppTheme,
  type ConnectionParams,
  type ReconnectMode
} from '../../shared/types'
import { sessionStyleFrom, tunnelConfigFrom, protocolConfigFrom, sessionStyleDefaultsFrom, reconnectModeFrom, screenConfigFrom } from '../../shared/connection'
import { normalizeTabPluginLayout } from '../../shared/pluginLayout'
import { normalizeHostPluginSettings } from '../../shared/plugins'

const HOSTS_FILE = 'hosts.json'
const PREVIOUS_HOSTS_FILE = 'sessions.json'
const TABS_FILE = 'tabs.json'
const SETTINGS_FILE = 'settings.json'
const KNOWN_HOSTS_FILE = 'known_hosts.json'
const JSON_INDENT = 2

function dataDir(): string {
  const dir = app.getPath('userData')
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true })
  }
  return dir
}

function readJson<T>(file: string, fallback: T): T {
  const path = join(dataDir(), file)
  if (!existsSync(path)) {
    return fallback
  }
  try {
    return JSON.parse(readFileSync(path, 'utf8')) as T
  } catch {
    return fallback
  }
}

function writeJson(file: string, value: unknown): void {
  writeFileSync(join(dataDir(), file), JSON.stringify(value, null, JSON_INDENT), 'utf8')
}

type StoredSettings = Partial<AppSettings> & {
  fontSizePx?: number
  fontFamily?: string
  scrollbackLines?: number
  /** Removed; stripped on load */
  autoReconnectOnDrop?: boolean
  /** Removed; stripped on load */
  reconnectMaxAttempts?: number
}

function storedStyleFallback(): {
  fontSizePx?: number
  fontFamily?: string
  scrollbackLines?: number
} {
  const raw = readJson<StoredSettings>(SETTINGS_FILE, {})
  return {
    fontSizePx: raw.fontSizePx,
    fontFamily: raw.fontFamily,
    scrollbackLines: raw.scrollbackLines
  }
}

/** Map removed global auto-reconnect toggle onto per-host default once */
function legacyReconnectModeDefault(): ReconnectMode {
  const raw = readJson<StoredSettings>(SETTINGS_FILE, {})
  if (raw.autoReconnectOnDrop === false) {
    return RECONNECT_MODE_NONE
  }
  return DEFAULT_RECONNECT_MODE
}

function normalizeTags(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return []
  }
  const seen = new Set<string>()
  const tags: string[] = []
  for (const item of value) {
    if (typeof item !== 'string') {
      continue
    }
    const tag = item.trim()
    if (!tag) {
      continue
    }
    const key = tag.toLowerCase()
    if (seen.has(key)) {
      continue
    }
    seen.add(key)
    tags.push(tag)
  }
  return tags
}

function normalizeHost(
  raw: Partial<HostProfile> & { id: string },
  fallbackMode: ReconnectMode = DEFAULT_RECONNECT_MODE
): HostProfile {
  return {
    id: raw.id,
    name: raw.name ?? '',
    host: raw.host ?? '',
    port: raw.port ?? DEFAULT_SSH_PORT,
    username: raw.username ?? '',
    passwordVaultId: raw.passwordVaultId ?? '',
    privateKeyPath: raw.privateKeyPath ?? '',
    passphraseVaultId: raw.passphraseVaultId ?? '',
    authMethod: raw.authMethod ?? 'none',
    proxyHostId: raw.proxyHostId ?? '',
    ...protocolConfigFrom(raw),
    ...sessionStyleFrom({ ...storedStyleFallback(), ...raw }),
    ...tunnelConfigFrom(raw),
    pluginSettings: normalizeHostPluginSettings(raw.pluginSettings),
    reconnectMode: reconnectModeFrom(raw, fallbackMode),
    ...screenConfigFrom(raw),
    tags: normalizeTags(raw.tags)
  }
}

function readHostsFile(): Partial<HostProfile>[] {
  const dir = dataDir()
  const current = join(dir, HOSTS_FILE)
  if (existsSync(current)) {
    return readJson<Partial<HostProfile>[]>(HOSTS_FILE, [])
  }
  const previous = join(dir, PREVIOUS_HOSTS_FILE)
  if (!existsSync(previous)) {
    return []
  }
  const hosts = readJson<Partial<HostProfile>[]>(PREVIOUS_HOSTS_FILE, [])
  writeJson(HOSTS_FILE, hosts)
  return hosts
}

export class SessionStore {
  /** Persist reconnectMode onto hosts/tabs that predate the per-host setting */
  migrateReconnectModes(): void {
    this.listHosts()
    new TabStore().getTabs()
  }

  listHosts(): HostProfile[] {
    const fallbackMode = legacyReconnectModeDefault()
    const rawList = readHostsFile()
    const hosts = rawList.map((h) =>
      normalizeHost({ ...h, id: h.id ?? randomUUID() }, fallbackMode)
    )
    const needsWrite = rawList.some(
      (h) =>
        h.reconnectMode === undefined ||
        h.openInScreen === undefined ||
        h.tags === undefined
    )
    if (needsWrite && hosts.length > 0) {
      writeJson(HOSTS_FILE, hosts)
    }
    return hosts
  }

  saveHost(host: HostProfile): HostProfile {
    const next = normalizeHost({ ...host, id: host.id })
    const hosts = this.listHosts()
    const idx = hosts.findIndex((h) => h.id === next.id)
    if (idx >= 0) {
      hosts[idx] = next
    } else {
      hosts.push(next)
    }
    writeJson(HOSTS_FILE, hosts)
    return next
  }

  deleteHost(id: string): void {
    writeJson(
      HOSTS_FILE,
      this.listHosts().filter((h) => h.id !== id)
    )
  }

  getHost(id: string): HostProfile | undefined {
    return this.listHosts().find((h) => h.id === id)
  }
}

export class TabStore {
  getTabs(): TabSnapshot[] {
    const fallback = storedStyleFallback()
    const fallbackMode = legacyReconnectModeDefault()
    const rawTabs = readJson<TabSnapshot[]>(TABS_FILE, [])
    const tabs = rawTabs.map((t) => ({
      ...t,
      activePluginIds: Array.isArray(t.activePluginIds) ? t.activePluginIds : [],
      pluginLayout: normalizeTabPluginLayout(
        (t as TabSnapshot & { pluginLayout?: unknown }).pluginLayout
      ),
      connection: {
        ...t.connection,
        ...protocolConfigFrom(t.connection),
        ...sessionStyleFrom({ ...fallback, ...t.connection }),
        ...tunnelConfigFrom(t.connection),
        pluginSettings: normalizeHostPluginSettings(
          (t.connection as ConnectionParams & { pluginSettings?: unknown }).pluginSettings
        ),
        reconnectMode: reconnectModeFrom(t.connection, fallbackMode),
        ...screenConfigFrom(t.connection)
      }
    }))
    const needsWrite = rawTabs.some(
      (t) =>
        t.connection?.reconnectMode === undefined || t.connection?.openInScreen === undefined
    )
    if (needsWrite && tabs.length > 0) {
      writeJson(TABS_FILE, tabs)
    }
    return tabs
  }

  saveTabs(tabs: TabSnapshot[]): void {
    writeJson(TABS_FILE, tabs)
  }
}

export class SettingsStore {
  get(): AppSettings {
    const raw = readJson<StoredSettings>(SETTINGS_FILE, {})
    const {
      fontSizePx: _fontSizePx,
      fontFamily: _fontFamily,
      scrollbackLines: _scrollbackLines,
      autoReconnectOnDrop: _autoReconnectOnDrop,
      reconnectMaxAttempts: _reconnectMaxAttempts,
      ...rest
    } = raw
    const theme: AppTheme = rest.theme === 'light' ? 'light' : DEFAULT_THEME
    const enabledPlugins = Array.isArray(rest.enabledPlugins)
      ? rest.enabledPlugins
      : DEFAULT_SETTINGS.enabledPlugins
    const pluginSettings =
      rest.pluginSettings && typeof rest.pluginSettings === 'object' && !Array.isArray(rest.pluginSettings)
        ? rest.pluginSettings
        : {}
    const pluginPanelPlacements =
      rest.pluginPanelPlacements &&
      typeof rest.pluginPanelPlacements === 'object' &&
      !Array.isArray(rest.pluginPanelPlacements)
        ? rest.pluginPanelPlacements
        : {}
    const pluginPanelOrder = Array.isArray(rest.pluginPanelOrder) ? rest.pluginPanelOrder : []
    const sessionStyleDefaults = sessionStyleDefaultsFrom(
      rest.sessionStyleDefaults && typeof rest.sessionStyleDefaults === 'object'
        ? rest.sessionStyleDefaults
        : {
            fontSizePx: raw.fontSizePx,
            fontFamily: raw.fontFamily,
            scrollbackLines: raw.scrollbackLines
          }
    )
    return {
      ...DEFAULT_SETTINGS,
      ...rest,
      theme,
      enabledPlugins,
      pluginSettings,
      pluginPanelPlacements,
      pluginPanelOrder,
      sessionStyleDefaults
    }
  }

  set(partial: Partial<AppSettings>): AppSettings {
    const merged = { ...this.get(), ...partial }
    const theme: AppTheme = merged.theme === 'light' ? 'light' : DEFAULT_THEME
    const next = { ...merged, theme }
    writeJson(SETTINGS_FILE, next)
    return next
  }
}

export class KnownHostsStore {
  list(): KnownHostEntry[] {
    return readJson<KnownHostEntry[]>(KNOWN_HOSTS_FILE, [])
  }

  find(host: string, port: number): KnownHostEntry | undefined {
    return this.list().find((e) => e.host === host && e.port === port)
  }

  upsert(entry: KnownHostEntry): void {
    const list = this.list().filter((e) => !(e.host === entry.host && e.port === entry.port))
    list.push(entry)
    writeJson(KNOWN_HOSTS_FILE, list)
  }
}
