import type { PluginMainModule } from '@plugin-api/main'
import { isMacroPadRendererMessage } from './protocol'

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
  }
}
