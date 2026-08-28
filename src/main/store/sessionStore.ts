import { randomUUID } from 'crypto'
import { app } from 'electron'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs'
import { join } from 'path'
import {
  AppSettings,
  DEFAULT_SETTINGS,
  DEFAULT_SSH_PORT,
  HostProfile,
  KnownHostEntry,
  TabSnapshot
} from '../../shared/types'

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
  writeFileSync(join(dataDir(), file), JSON.stringify(value, null, 2), 'utf8')
}

const SESSIONS_FILE = 'sessions.json'
const TABS_FILE = 'tabs.json'
const SETTINGS_FILE = 'settings.json'
const KNOWN_HOSTS_FILE = 'known_hosts.json'

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
    proxyHostId: raw.proxyHostId ?? ''
  }
}

export class SessionStore {
  listHosts(): HostProfile[] {
    return readJson<Partial<HostProfile>[]>(SESSIONS_FILE, []).map((h) =>
      normalizeHost({ ...h, id: h.id ?? randomUUID() })
    )
  }

  saveHost(host: HostProfile): HostProfile {
    const hosts = this.listHosts()
    const idx = hosts.findIndex((h) => h.id === host.id)
    if (idx >= 0) {
      hosts[idx] = host
    } else {
      hosts.push(host)
    }
    writeJson(SESSIONS_FILE, hosts)
    return host
  }

  deleteHost(id: string): void {
    writeJson(
      SESSIONS_FILE,
      this.listHosts().filter((h) => h.id !== id)
    )
  }

  getHost(id: string): HostProfile | undefined {
    return this.listHosts().find((h) => h.id === id)
  }
}

export class TabStore {
  getTabs(): TabSnapshot[] {
    return readJson<TabSnapshot[]>(TABS_FILE, [])
  }

  saveTabs(tabs: TabSnapshot[]): void {
    writeJson(TABS_FILE, tabs)
  }
}

export class SettingsStore {
  get(): AppSettings {
    return { ...DEFAULT_SETTINGS, ...readJson<Partial<AppSettings>>(SETTINGS_FILE, {}) }
  }

  set(partial: Partial<AppSettings>): AppSettings {
    const next = { ...this.get(), ...partial }
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
