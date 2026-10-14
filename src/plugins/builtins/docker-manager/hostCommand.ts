import { execFile } from 'node:child_process'
import type { PluginMainContext } from '@plugin-api/main'
import {
  DOCKER_HOST_COMMAND_MAX_BUFFER,
  DOCKER_HOST_COMMAND_TIMEOUT_MS
} from './defaults'

const UNIX_PLATFORMS = new Set(['linux', 'darwin', 'freebsd', 'openbsd', 'netbsd', 'sunos', 'aix'])

export interface HostCommandOptions {
  /** Written to the command's stdin, then the stream is closed. */
  stdin?: string
  timeoutMs?: number
  /** Called as output arrives (stdout and stderr merged). */
  onData?: (chunk: string) => void
}

export function canRunHostCommands(ctx: PluginMainContext): boolean {
  if (ctx.isSshSession()) {
    return true
  }
  return ctx.isLocalSession() && UNIX_PLATFORMS.has(process.platform)
}

/**
 * Run one shell command on the session host, merging stdout and stderr.
 * SSH sessions use the exec channel; Local sessions use `/bin/sh`.
 *
 * Mirrors the approach used by the server-monitor plugin: `ctx.execCapture` is
 * SSH-only, so a local session runs the command in the main process.
 */
export function runHostCommand(
  ctx: PluginMainContext,
  command: string,
  options: HostCommandOptions = {}
): Promise<string> {
  const timeoutMs = options.timeoutMs ?? DOCKER_HOST_COMMAND_TIMEOUT_MS

  if (ctx.isLocalSession()) {
    if (!UNIX_PLATFORMS.has(process.platform)) {
      return Promise.reject(new Error('Local Docker commands require a Unix-like platform'))
    }
    return new Promise((resolve, reject) => {
      const child = execFile(
        '/bin/sh',
        ['-c', command],
        { timeout: timeoutMs, maxBuffer: DOCKER_HOST_COMMAND_MAX_BUFFER },
        (error, stdout, stderr) => {
          if (error) {
            const detail = (stderr || stdout || error.message).trim()
            reject(new Error(detail || error.message))
            return
          }
          resolve(stdout)
        }
      )
      child.stdout?.on('data', (chunk: Buffer) => options.onData?.(chunk.toString('utf8')))
      child.stderr?.on('data', (chunk: Buffer) => options.onData?.(chunk.toString('utf8')))
      if (child.stdin) {
        child.stdin.end(options.stdin ?? '')
      }
    })
  }

  if (!ctx.isSshSession()) {
    return Promise.reject(new Error('This operation requires an SSH or local Unix session'))
  }

  if (options.stdin === undefined && !options.onData) {
    return ctx.execCapture(command)
  }

  return new Promise((resolve, reject) => {
    let connectionId: string | null = null
    let output = ''
    let timer: ReturnType<typeof setTimeout> | null = null
    let offData = (): void => undefined
    let offClosed = (): void => undefined
    let settled = false

    const finish = (error?: string): void => {
      if (settled) {
        return
      }
      settled = true
      if (timer) {
        clearTimeout(timer)
      }
      offData()
      offClosed()
      if (error) {
        reject(new Error(error))
      } else {
        resolve(output)
      }
    }

    void ctx
      .openSideConnection({ kind: 'ssh-exec', command })
      .then((id) => {
        connectionId = id
        offData = ctx.onSideData(id, (data) => {
          output += data
          options.onData?.(data)
        })
        offClosed = ctx.onSideClosed(id, (error) => finish(error))
        if (options.stdin !== undefined) {
          ctx.writeSideConnection(id, options.stdin)
        }
        timer = setTimeout(() => {
          const toClose = connectionId
          finish('Command timed out')
          if (toClose) {
            ctx.closeSideConnection(toClose)
          }
        }, timeoutMs)
      })
      .catch((error: unknown) => {
        finish(error instanceof Error ? error.message : String(error))
      })
  })
}
