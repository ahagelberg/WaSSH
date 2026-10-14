import { createConnection } from 'net'
import type { Duplex } from 'stream'
import type { PluginMainContext } from '@plugin-api/main'
import { asSocket, type SocketLike } from './socketAdapter'

/** How (and whether) this session can reach a Docker daemon. */
export type DockerTransportKind = 'ssh' | 'local' | 'unsupported'

const UNIX_PLATFORMS = new Set(['linux', 'darwin', 'freebsd', 'openbsd', 'netbsd', 'sunos', 'aix'])

/**
 * Sessions reach the daemon socket in one of two ways: an SSH session tunnels to
 * the remote unix socket, a Local session connects to the host's own socket.
 */
export function dockerTransportKind(ctx: PluginMainContext): DockerTransportKind {
  if (ctx.isSshSession()) {
    return 'ssh'
  }
  if (ctx.isLocalSession()) {
    return UNIX_PLATFORMS.has(process.platform) ? 'local' : 'unsupported'
  }
  return 'unsupported'
}

/** Human-readable reason a session cannot reach a Docker daemon. */
export function unsupportedTransportReason(ctx: PluginMainContext): string {
  if (ctx.isLocalSession() && !UNIX_PLATFORMS.has(process.platform)) {
    return 'Docker over a named pipe is not supported on this platform'
  }
  return 'Docker requires an SSH or local Unix session'
}

/** Error raised when the daemon socket cannot be reached. */
export class DockerSocketError extends Error {
  readonly code: string | undefined
  readonly socketPath: string

  constructor(socketPath: string, message: string, code?: string) {
    super(message)
    this.name = 'DockerSocketError'
    this.socketPath = socketPath
    this.code = code
  }
}

/** True when the error means "no daemon is listening on that socket". */
export function isSocketMissingError(error: unknown): boolean {
  if (!(error instanceof DockerSocketError)) {
    return false
  }
  return (
    error.code === 'ENOENT' ||
    error.code === 'ECONNREFUSED' ||
    error.code === 'EACCES' ||
    error.code === 'EPERM' ||
    error.code === 'ENOTDIR'
  )
}

function openLocalSocket(socketPath: string): Promise<Duplex> {
  return new Promise((resolve, reject) => {
    const socket = createConnection(socketPath)
    const onError = (error: NodeJS.ErrnoException): void => {
      socket.destroy()
      reject(
        new DockerSocketError(socketPath, error.message || String(error), error.code ?? undefined)
      )
    }
    socket.once('error', onError)
    socket.once('connect', () => {
      socket.off('error', onError)
      resolve(socket)
    })
  })
}

/**
 * Open one connection to the Docker daemon socket. Every HTTP request gets a
 * fresh socket and closes it afterwards (`Connection: close`); the ssh2 channel
 * does not support keep-alive reuse reliably.
 */
export async function openDockerSocket(
  ctx: PluginMainContext,
  socketPath: string
): Promise<SocketLike> {
  const kind = dockerTransportKind(ctx)

  if (kind === 'unsupported') {
    throw new DockerSocketError(socketPath, unsupportedTransportReason(ctx))
  }

  try {
    const duplex = kind === 'ssh' ? await ctx.openUnixStream(socketPath) : await openLocalSocket(socketPath)
    return asSocket(duplex)
  } catch (error) {
    if (error instanceof DockerSocketError) {
      throw error
    }
    const message = error instanceof Error ? error.message : String(error)
    const code =
      error && typeof error === 'object' && 'code' in error
        ? String((error as { code?: unknown }).code)
        : undefined
    throw new DockerSocketError(socketPath, message, code)
  }
}

/** Explain a socket failure in terms a Docker user understands. */
export function describeSocketError(error: DockerSocketError): string {
  const { socketPath, code } = error
  if (code === 'ENOENT') {
    return `No Docker socket at ${socketPath}. Is the Docker daemon installed and running?`
  }
  if (code === 'EACCES' || code === 'EPERM') {
    return `Permission denied for ${socketPath}. Add your user to the "docker" group or check the socket's permissions.`
  }
  if (code === 'ECONNREFUSED') {
    return `Nothing is listening on ${socketPath}. Is the Docker daemon running?`
  }
  return error.message || `Cannot reach the Docker socket at ${socketPath}`
}
