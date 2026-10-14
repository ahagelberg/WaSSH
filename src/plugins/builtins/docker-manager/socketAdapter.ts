import type { Duplex } from 'stream'

/**
 * The subset of `net.Socket` that Node's `http` client touches. Streams coming
 * from `ctx.openUnixStream` (an ssh2 channel) are plain Duplexes and lack all of
 * these, which makes `http` throw `TypeError: sock.setTimeout is not a function`.
 */
export interface SocketLike extends Duplex {
  setTimeout: (timeout: number, callback?: () => void) => SocketLike
  setNoDelay: (noDelay?: boolean) => SocketLike
  setKeepAlive: (enable?: boolean, initialDelay?: number) => SocketLike
  ref: () => SocketLike
  unref: () => SocketLike
  address: () => { address: string; family: string; port: number }
  destroySoon: () => void
  connecting: boolean
  pending: boolean
  remoteAddress?: string
  remoteFamily?: string
  remotePort?: number
}

function define(target: object, key: string, value: unknown): void {
  try {
    ;(target as Record<string, unknown>)[key] = value
  } catch {
    try {
      Object.defineProperty(target, key, { value, configurable: true, writable: true })
    } catch {
      /* frozen stream — the caller falls back to a wrapper */
    }
  }
}

/**
 * Add the missing `net.Socket` surface to a Duplex so it can be used as an HTTP
 * connection. Mutates in place when possible (cheapest and keeps identity); the
 * no-op methods return the stream so http's chaining style keeps working.
 */
export function asSocket(stream: Duplex): SocketLike {
  const socket = stream as SocketLike
  const self = (): SocketLike => socket

  if (typeof socket.setNoDelay !== 'function') {
    define(socket, 'setNoDelay', self)
  }
  if (typeof socket.setKeepAlive !== 'function') {
    define(socket, 'setKeepAlive', self)
  }
  if (typeof socket.setTimeout !== 'function') {
    // Node calls this unconditionally before/after connecting. Timeouts are
    // enforced by the caller, so the callback is intentionally unused.
    define(socket, 'setTimeout', () => socket)
  }
  if (typeof socket.ref !== 'function') {
    define(socket, 'ref', self)
  }
  if (typeof socket.unref !== 'function') {
    define(socket, 'unref', self)
  }
  if (typeof socket.address !== 'function') {
    define(socket, 'address', () => ({ address: 'unix', family: 'unix', port: 0 }))
  }
  if (typeof socket.destroySoon !== 'function') {
    define(socket, 'destroySoon', () => {
      socket.end()
      socket.destroy()
    })
  }
  define(socket, 'connecting', false)
  define(socket, 'pending', false)

  return socket
}
