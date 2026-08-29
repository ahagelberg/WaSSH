import type { PluginMainModule } from '../PluginHost'
import {
  SERVER_MONITOR_DEFAULT_INTERVAL_MS,
  SERVER_MONITOR_MIN_INTERVAL_MS
} from '../../../shared/plugins'

/** Collect Linux-ish CPU / mem / disk in one shot */
const MONITOR_COMMAND =
  "echo '===CPU==='; (grep -E '^cpu ' /proc/stat 2>/dev/null || echo cpu n/a); " +
  "echo '===MEM==='; (free -m 2>/dev/null || echo n/a); " +
  "echo '===DISK==='; (df -h / 2>/dev/null | tail -n 1 || echo n/a)"

/** Wait for exec channel to finish (ms) */
const EXEC_WAIT_MS = 15000

interface MonitorSnapshot {
  raw: string
  updatedAt: number
  error?: string
}

function parseInterval(settings: Record<string, unknown>): number {
  const n = Number(settings.intervalMs)
  if (!Number.isFinite(n)) {
    return SERVER_MONITOR_DEFAULT_INTERVAL_MS
  }
  return Math.max(SERVER_MONITOR_MIN_INTERVAL_MS, Math.floor(n))
}

export const serverMonitorMain: PluginMainModule = {
  async onActivate(ctx) {
    let timer: ReturnType<typeof setInterval> | null = null
    let busy = false
    let stopped = false

    const poll = async (): Promise<void> => {
      if (busy || stopped) {
        return
      }
      busy = true
      try {
        const connectionId = await ctx.openSideConnection({
          kind: 'ssh-exec',
          command: MONITOR_COMMAND
        })
        let buf = ''
        const offData = ctx.onSideData(connectionId, (chunk) => {
          buf += chunk
        })
        await new Promise<void>((resolve) => {
          const finish = (): void => resolve()
          const offClose = ctx.onSideClosed(connectionId, () => {
            offClose()
            finish()
          })
          setTimeout(() => {
            offClose()
            ctx.closeSideConnection(connectionId)
            finish()
          }, EXEC_WAIT_MS)
        })
        offData()
        ctx.sendToRenderer({
          type: 'stats',
          snapshot: {
            raw: buf.trim() || '(no output)',
            updatedAt: Date.now()
          } satisfies MonitorSnapshot
        })
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        ctx.sendToRenderer({
          type: 'stats',
          snapshot: {
            raw: '',
            updatedAt: Date.now(),
            error: message
          } satisfies MonitorSnapshot
        })
      } finally {
        busy = false
      }
    }

    void poll()
    const interval = parseInterval(ctx.getSettings())
    timer = setInterval(() => {
      void poll()
    }, interval)

    ctx.onDeactivateCleanup(() => {
      stopped = true
      if (timer) {
        clearInterval(timer)
        timer = null
      }
    })
  }
}
