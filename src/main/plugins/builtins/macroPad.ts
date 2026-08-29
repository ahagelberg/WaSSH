import type { PluginMainModule } from '../PluginHost'

/**
 * Macro pad injects from the renderer via session:write.
 * Main module exists so activate/deactivate lifecycle is consistent.
 */
export const macroPadMain: PluginMainModule = {
  onActivate() {
    /* UI + hotkeys in renderer */
  },
  onMessage(ctx, payload) {
    if (!payload || typeof payload !== 'object') {
      return
    }
    const msg = payload as { type?: string; text?: string }
    if (msg.type === 'send' && typeof msg.text === 'string') {
      ctx.writeToSession(msg.text)
    }
  }
}
