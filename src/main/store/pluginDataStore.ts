import { app } from 'electron'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs'
import { join } from 'path'

const JSON_INDENT = 2

/** Filename for a plugin's private JSON data in userData */
export function pluginDataFileName(pluginId: string, scopeId?: string): string {
  const safe = pluginId.replace(/[^a-zA-Z0-9_-]/g, '_')
  if (!scopeId) {
    return `plugin-${safe}.json`
  }
  const safeScope = scopeId.replace(/[^a-zA-Z0-9_-]/g, '_')
  return `plugin-${safe}.${safeScope}.json`
}

function dataDir(): string {
  const dir = app.getPath('userData')
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true })
  }
  return dir
}

/**
 * Per-plugin JSON files next to hosts.json / settings.json / tabs.json.
 * Example: userData/plugin-scratchpad.json
 * A non-empty scopeId keeps the value in a scoped sibling file, e.g.
 * userData/plugin-scratchpad.<scope>.json (per-host plugin data).
 */
export class PluginDataStore {
  get(pluginId: string, scopeId?: string): unknown {
    const path = join(dataDir(), pluginDataFileName(pluginId, scopeId))
    if (!existsSync(path)) {
      return null
    }
    try {
      return JSON.parse(readFileSync(path, 'utf8')) as unknown
    } catch {
      return null
    }
  }

  set(pluginId: string, value: unknown, scopeId?: string): void {
    writeFileSync(
      join(dataDir(), pluginDataFileName(pluginId, scopeId)),
      JSON.stringify(value, null, JSON_INDENT),
      'utf8'
    )
  }
}
