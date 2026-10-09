import type { StreamHandlerRegistration, StreamTransform } from './types'

const RECENT_INBOUND_MAX_CHARS = 64_000

interface Entry {
  pluginId: string
  mode: 'observe' | 'intercept'
  handler: StreamTransform
}

/**
 * Ordered observe/intercept pipeline for the main session PTY stream.
 * Interceptors may rewrite or drop (return null). Observers receive a copy.
 */
export class SessionDataPipeline {
  private inbound = new Map<string, Entry[]>()
  private outbound = new Map<string, Entry[]>()
  private recentInbound = new Map<string, string>()

  clearTab(tabId: string): void {
    this.inbound.delete(tabId)
    this.outbound.delete(tabId)
    this.recentInbound.delete(tabId)
  }

  unregisterPlugin(tabId: string, pluginId: string): void {
    const strip = (map: Map<string, Entry[]>): void => {
      const list = map.get(tabId)
      if (!list) {
        return
      }
      const next = list.filter((e) => e.pluginId !== pluginId)
      if (next.length === 0) {
        map.delete(tabId)
      } else {
        map.set(tabId, next)
      }
    }
    strip(this.inbound)
    strip(this.outbound)
  }

  register(tabId: string, reg: StreamHandlerRegistration): void {
    const map = reg.direction === 'inbound' ? this.inbound : this.outbound
    const list = map.get(tabId) ?? []
    list.push({ pluginId: reg.pluginId, mode: reg.mode, handler: reg.handler })
    map.set(tabId, list)
  }

  processInbound(tabId: string, data: string): string | null {
    const output = this.process(this.inbound.get(tabId) ?? [], data)
    if (output !== null) {
      const combined = (this.recentInbound.get(tabId) ?? '') + output
      this.recentInbound.set(tabId, combined.slice(-RECENT_INBOUND_MAX_CHARS))
    }
    return output
  }

  getRecentInbound(tabId: string, maxChars = RECENT_INBOUND_MAX_CHARS): string {
    const output = this.recentInbound.get(tabId) ?? ''
    const limit = Math.max(0, Math.floor(maxChars))
    return limit === 0 ? '' : output.slice(-limit)
  }

  processOutbound(tabId: string, data: string): string | null {
    return this.process(this.outbound.get(tabId) ?? [], data)
  }

  private process(entries: Entry[], data: string): string | null {
    let current: string | null = data
    for (const entry of entries) {
      if (current === null) {
        break
      }
      if (entry.mode === 'observe') {
        try {
          entry.handler(current)
        } catch {
          /* observer errors must not break the stream */
        }
        continue
      }
      try {
        current = entry.handler(current)
      } catch {
        /* interceptor errors: keep previous data */
      }
    }
    return current
  }
}
