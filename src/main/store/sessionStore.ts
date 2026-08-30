import { randomUUID } from 'crypto'
import { app } from 'electron'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs'
import { join } from 'path'
import {
  AppSettings,
  DEFAULT_SETTINGS,
  DEFAULT_SSH_PORT,
  DEFAULT_THEME,
  HostProfile,
  KnownHostEntry,
  TabSnapshot,
  type AppTheme,
  type ConnectionParams
} from '../../shared/types'
import { sessionStyleFrom, tunnelConfigFrom, protocolConfigFrom, sessionStyleDefaultsFrom } from '../../shared/connection'
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

function normalizeHost(raw: Partial<HostProfile> & { id: string }): HostProfile {
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
    pluginSettings: normalizeHostPluginSettings(raw.pluginSettings)
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
  listHosts(): HostProfile[] {
    return readHostsFile().map((h) => normalizeHost({ ...h, id: h.id ?? randomUUID() }))
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
    return readJson<TabSnapshot[]>(TABS_FILE, []).map((t) => ({
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
        )
      }
    }))
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
