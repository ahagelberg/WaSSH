import type { PluginMainModule } from '@plugin-api/main'

/** Scratchpad is renderer-driven; main module is a no-op lifecycle hook. */
export const scratchpadMain: PluginMainModule = {
  onActivate() {
    /* UI only */
  }
}
