import type { PluginManifest } from '../../shared/plugins'

/**
 * Future: scan `userData/plugins/<id>/` for manifest.json + main entry.
 * Expected layout (not loaded in v1):
 *   userData/plugins/<id>/manifest.json
 *   userData/plugins/<id>/main.js
 *   userData/plugins/<id>/ui.js
 */
export function loadExternalPlugins(): PluginManifest[] {
  return []
}
