import type { PluginMainModule } from '@plugin-api/main'
import { contentFromData, type ScratchpadData } from './protocol'

/** Scratchpad is renderer-driven; main module is a no-op lifecycle hook. */
export const scratchpadMain: PluginMainModule = {
  onActivate() {
    /* UI only */
  },
  onApiCall(ctx, method, params) {
    const scope = ctx.getSessionScopeId()
    if (method === 'read_notes') {
      return contentFromData(ctx.getData(scope))
    }
    const args = params && typeof params === 'object' ? (params as Record<string, unknown>) : {}
    if (method === 'write_notes') {
      const content = typeof args.content === 'string' ? args.content : ''
      ctx.setData({ content } satisfies ScratchpadData, scope)
      return { ok: true }
    }
    if (method === 'append_notes') {
      const text = typeof args.text === 'string' ? args.text : ''
      const current = contentFromData(ctx.getData(scope))
      const next = current ? `${current}\n${text}` : text
      ctx.setData({ content: next } satisfies ScratchpadData, scope)
      return { ok: true }
    }
    throw new Error(`Unknown scratchpad API method: ${method}`)
  }
}
