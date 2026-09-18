import { app } from 'electron'
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'fs'
import { join } from 'path'
import {
  BACKUP_FORMAT_VERSION,
  BACKUP_MANIFEST_ENTRY,
  isBackupManifest,
  type BackupImportProblem,
  type BackupImportResult,
  type BackupManifest
} from '../../shared/backup'
import { APP_VERSION } from '../../shared/version'
import { readZip, writeZip } from './zipArchive'

/** Config files always included, in archive order */
const CONFIG_FILES = [
  'settings.json',
  'hosts.json',
  'tabs.json',
  'known_hosts.json',
  'vault.json'
] as const

/** Prefix for per-plugin data files (`plugin-<id>[.<scope>].json`) */
const PLUGIN_FILE_PREFIX = 'plugin-'

/** Only plain JSON files are backed up (skips e.g. `plugin-x.json.bak`) */
const PLUGIN_FILE_RE = /^plugin-[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)?\.json$/

/** JSON files that must parse before they are restored */
const JSON_ENTRY_RE = /\.json$/

function dataDir(): string {
  const dir = app.getPath('userData')
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true })
  }
  return dir
}

/** Config file names present in userData, in a stable order */
function configFileNames(): string[] {
  const dir = dataDir()
  const present = new Set(readdirSync(dir))
  const names: string[] = CONFIG_FILES.filter((file) => present.has(file))
  const pluginFiles = Array.from(present)
    .filter((file) => file.startsWith(PLUGIN_FILE_PREFIX) && PLUGIN_FILE_RE.test(file))
    .sort()
  names.push(...pluginFiles)
  return names
}

/** Read a config file, or null when it is missing/unreadable */
function readConfigFile(name: string): Buffer | null {
  const path = join(dataDir(), name)
  if (!existsSync(path)) {
    return null
  }
  try {
    return readFileSync(path)
  } catch {
    return null
  }
}

/** Build the manifest for an archive written now */
function buildManifest(entries: string[]): BackupManifest {
  return {
    formatVersion: BACKUP_FORMAT_VERSION,
    appVersion: APP_VERSION,
    createdAt: Date.now(),
    entries
  }
}

/**
 * Bundle every config file into one zip. The manifest is written first so a
 * reader can identify the archive without scanning the whole central directory.
 */
export async function exportBackup(): Promise<Buffer> {
  const names = configFileNames()
  const entries: Array<{ name: string; data: Buffer }> = []
  for (const name of names) {
    const data = readConfigFile(name)
    if (data) {
      entries.push({ name, data })
    }
  }
  const manifest = buildManifest(entries.map((entry) => entry.name))
  return writeZip([
    { name: BACKUP_MANIFEST_ENTRY, data: Buffer.from(JSON.stringify(manifest, null, 2), 'utf8') },
    ...entries
  ])
}

/** Map a thrown zip/manifest error onto a user-facing import problem */
function importProblemFor(message: string): BackupImportProblem {
  if (message.includes('manifest')) {
    return 'missing-manifest'
  }
  return 'not-a-zip'
}

/**
 * Inspect an archive without touching any config file: validates the manifest
 * and reports whether its app version differs from the running app.
 */
export function inspectBackup(buffer: Buffer): BackupImportResult {
  let entries: Array<{ name: string; data: Buffer }>
  try {
    entries = readZip(buffer)
  } catch (err) {
    return {
      ok: false,
      problem: importProblemFor(err instanceof Error ? err.message : ''),
      error: err instanceof Error ? err.message : 'Unreadable archive'
    }
  }
  const manifestEntry = entries.find((entry) => entry.name === BACKUP_MANIFEST_ENTRY)
  if (!manifestEntry) {
    return { ok: false, problem: 'missing-manifest', error: 'Archive has no manifest.json' }
  }
  let parsed: unknown
  try {
    parsed = JSON.parse(manifestEntry.data.toString('utf8')) as unknown
  } catch {
    return { ok: false, problem: 'bad-manifest', error: 'manifest.json is not valid JSON' }
  }
  if (!isBackupManifest(parsed)) {
    return { ok: false, problem: 'bad-manifest', error: 'manifest.json is missing required fields' }
  }
  if (parsed.formatVersion > BACKUP_FORMAT_VERSION) {
    return {
      ok: false,
      problem: 'newer-format',
      manifest: parsed,
      error: `Archive format v${parsed.formatVersion} is newer than this app supports (v${BACKUP_FORMAT_VERSION})`
    }
  }
  return {
    ok: true,
    manifest: parsed,
    versionMismatch: parsed.appVersion !== APP_VERSION
  }
}

/**
 * Validate every config entry, then write them into userData. Nothing is
 * written unless the whole archive is valid, so a bad backup cannot leave a
 * half-restored config behind.
 */
export function restoreBackup(buffer: Buffer): {
  ok: boolean
  restored?: string[]
  error?: string
} {
  const inspection = inspectBackup(buffer)
  if (!inspection.ok) {
    return { ok: false, error: inspection.error ?? 'Invalid backup archive' }
  }
  let entries: Array<{ name: string; data: Buffer }>
  try {
    entries = readZip(buffer)
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : 'Unreadable archive' }
  }

  const configEntries = entries.filter(
    (entry) => entry.name !== BACKUP_MANIFEST_ENTRY && isRestorableName(entry.name)
  )
  if (configEntries.length === 0) {
    return { ok: false, error: 'Archive contains no config files' }
  }
  for (const entry of configEntries) {
    if (JSON_ENTRY_RE.test(entry.name)) {
      try {
        JSON.parse(entry.data.toString('utf8')) as unknown
      } catch {
        return { ok: false, error: `Archive entry is not valid JSON: ${entry.name}` }
      }
    }
  }

  const dir = dataDir()
  for (const entry of configEntries) {
    writeFileSync(join(dir, entry.name), entry.data)
  }
  return { ok: true, restored: configEntries.map((entry) => entry.name) }
}

/**
 * Guard against path traversal: only plain file names at the userData root are
 * accepted, and only ones this app writes.
 */
function isRestorableName(name: string): boolean {
  if (name.includes('/') || name.includes('\\') || name.includes('..')) {
    return false
  }
  return (CONFIG_FILES as readonly string[]).includes(name) || PLUGIN_FILE_RE.test(name)
}
