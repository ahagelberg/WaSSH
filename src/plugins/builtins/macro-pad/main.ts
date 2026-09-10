import type { PluginMainModule } from '@plugin-api/main'
import type { PluginCommand } from '@plugin-api/shared'
import { isMacroPadRendererMessage } from './protocol'

function findMacro(buttons: unknown, id: unknown, label: unknown): PluginCommand | undefined {
  if (!Array.isArray(buttons)) {
    return undefined
  }
  const commands = buttons as PluginCommand[]
  if (typeof id === 'string' && id) {
    return commands.find((c) => c.id === id)
  }
  if (typeof label === 'string' && label) {
    return commands.find((c) => c.label === label)
  }
  return undefined
}

/**
 * Macro pad injects from the renderer via session:write.
 * Main module exists so activate/deactivate lifecycle is consistent.
 */
export const macroPadMain: PluginMainModule = {
  onActivate() {
    /* UI + hotkeys in renderer */
  },
  onMessage(ctx, payload) {
    if (!isMacroPadRendererMessage(payload)) {
      return
    }
    ctx.writeToSession(payload.text)
  },
  onApiCall(ctx, method, params) {
    if (method !== 'execute_macro') {
      throw new Error(`Unknown macro-pad API method: ${method}`)
    }
    const args = params && typeof params === 'object' ? (params as Record<string, unknown>) : {}
    const macro = findMacro(ctx.getSettings().buttons, args.id, args.label)
    if (!macro) {
      throw new Error('No macro found matching the given id/label')
    }
    ctx.writeToSession(macro.text)
    return { ok: true, label: macro.label }
  }
}
