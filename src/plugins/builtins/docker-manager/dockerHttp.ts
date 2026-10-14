import http from 'node:http'
import type { Duplex } from 'stream'
import { DOCKER_MAX_JSON_BYTES } from './defaults'
import type { SocketLike } from './socketAdapter'

/** Error carrying the HTTP status returned by the daemon. */
export class DockerApiError extends Error {
  readonly status: number

  constructor(status: number, message: string) {
    super(message)
    this.name = 'DockerApiError'
    this.status = status
  }
}

export interface DockerHttpRequest {
  method: string
  path: string
  query?: Record<string, string | number | boolean | undefined>
  headers?: Record<string, string>
  body?: string
  timeoutMs?: number
}

export interface DockerHttpResponse {
  status: number
  headers: http.IncomingHttpHeaders
  body: string
}

/** Callbacks for a streaming (long-lived) request. */
export interface DockerStreamHandlers {
  onResponse: (status: number, headers: http.IncomingHttpHeaders) => void
  onData: (chunk: Buffer) => void
  onEnd: (error?: Error) => void
}

export interface DockerStreamRequest extends DockerHttpRequest {
  /** Keep the connection open after headers (follow logs, stream stats). */
  stream: true
}

export function buildQuery(query: DockerHttpRequest['query']): string {
  if (!query) {
    return ''
  }
  const parts: string[] = []
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined) {
      continue
    }
    parts.push(`${encodeURIComponent(key)}=${encodeURIComponent(String(value))}`)
  }
  return parts.length > 0 ? `?${parts.join('&')}` : ''
}

/**
 * A one-connection agent: the transport hands us a single already-open socket
 * (an ssh2 channel or a local unix socket), so the agent must hand that exact
 * stream back exactly once.
 *
 * Note: `createConnection` passed to `http.request()` directly is ignored by
 * Node, so overriding it on an Agent instance is the only way to route a
 * non-`net.Socket` Duplex into the HTTP client.
 */
function oneShotAgent(socket: SocketLike): http.Agent {
  const agent = new http.Agent({ keepAlive: false, maxSockets: 1 })
  let used = false
  agent.createConnection = ((_options: unknown, callback?: (err: Error | null, stream: Duplex) => void) => {
    if (used) {
      const error = new Error('Docker connection socket was already used')
      if (callback) {
        process.nextTick(callback, error, socket)
        return undefined
      }
      throw error
    }
    used = true
    if (callback) {
      process.nextTick(callback, null, socket)
      return undefined
    }
    return socket
  }) as typeof agent.createConnection
  return agent
}

function requestOptions(req: DockerHttpRequest): http.RequestOptions {
  const headers: Record<string, string> = { ...req.headers }
  if (req.body !== undefined && headers['Content-Type'] === undefined) {
    headers['Content-Type'] = 'application/json'
  }
  if (headers['Content-Length'] === undefined && req.body !== undefined) {
    headers['Content-Length'] = String(Buffer.byteLength(req.body))
  }
  // One socket per request, so the daemon should always close it — unless the
  // caller is asking for a protocol upgrade (container attach/exec).
  if (headers.Connection === undefined) {
    headers.Connection = 'close'
  }
  return {
    method: req.method,
    path: `${req.path}${buildQuery(req.query)}`,
    headers
  }
}

function statusMessage(status: number, body: string): string {
  if (body) {
    try {
      const parsed = JSON.parse(body) as { message?: unknown }
      if (typeof parsed.message === 'string' && parsed.message) {
        return parsed.message
      }
    } catch {
      /* not JSON (usually a proxy error page) */
    }
    return body.slice(0, 500)
  }
  return `Docker API request failed with status ${status}`
}

function normalizeError(error: unknown): Error {
  if (error instanceof DockerApiError) {
    return error
  }
  const message = error instanceof Error ? error.message : String(error)
  return new Error(message || 'Docker API request failed')
}

/**
 * One request/response over an already-open socket. The socket is always
 * destroyed when the exchange settles.
 */
export function dockerRequest(
  socket: SocketLike,
  req: DockerHttpRequest
): Promise<DockerHttpResponse> {
  return new Promise((resolve, reject) => {
    const agent = oneShotAgent(socket)
    let settled = false
    let timer: ReturnType<typeof setTimeout> | null = null

    const finish = (fn: () => void): void => {
      if (settled) {
        return
      }
      settled = true
      if (timer) {
        clearTimeout(timer)
        timer = null
      }
      agent.destroy()
      try {
        socket.destroy()
      } catch {
        /* already gone */
      }
      fn()
    }

    const request = http.request({ ...requestOptions(req), agent }, (res) => {
      let body = ''
      let overflow = false
      res.setEncoding('utf8')
      res.on('data', (chunk: string) => {
        if (overflow) {
          return
        }
        body += chunk
        if (body.length > DOCKER_MAX_JSON_BYTES) {
          overflow = true
          finish(() => reject(new Error('Docker API response was too large')))
        }
      })
      res.on('end', () => {
        const status = res.statusCode ?? 0
        finish(() => {
          if (status >= 200 && status < 300) {
            resolve({ status, headers: res.headers, body })
          } else {
            reject(new DockerApiError(status, statusMessage(status, body)))
          }
        })
      })
      res.on('error', (error) => finish(() => reject(normalizeError(error))))
    })

    request.on('error', (error) => finish(() => reject(normalizeError(error))))

    const timeoutMs = req.timeoutMs
    if (timeoutMs && timeoutMs > 0) {
      timer = setTimeout(() => {
        finish(() => reject(new Error('Docker API request timed out')))
      }, timeoutMs)
    }

    if (req.body !== undefined) {
      request.write(req.body)
    }
    request.end()
  })
}

/**
 * Streaming request for endpoints that keep the connection open (log follow,
 * stats stream, image pull). Returns a handle the caller uses to abort.
 */
export function dockerStreamRequest(
  socket: SocketLike,
  req: DockerStreamRequest,
  handlers: DockerStreamHandlers
): () => void {
  const agent = oneShotAgent(socket)
  let settled = false

  const finish = (error?: Error): void => {
    if (settled) {
      return
    }
    settled = true
    agent.destroy()
    try {
      socket.destroy()
    } catch {
      /* already gone */
    }
    handlers.onEnd(error)
  }

  const request = http.request({ ...requestOptions(req), agent }, (res) => {
    handlers.onResponse(res.statusCode ?? 0, res.headers)
    if ((res.statusCode ?? 0) >= 400) {
      let body = ''
      res.setEncoding('utf8')
      res.on('data', (chunk: string) => {
        body += chunk
      })
      res.on('end', () => finish(new DockerApiError(res.statusCode ?? 0, statusMessage(res.statusCode ?? 0, body))))
      return
    }
    res.on('data', (chunk: Buffer) => handlers.onData(chunk))
    res.on('end', () => finish())
    res.on('error', (error) => finish(normalizeError(error)))
  })

  request.on('error', (error) => finish(normalizeError(error)))

  if (req.timeoutMs && req.timeoutMs > 0) {
    setTimeout(() => finish(new Error('Docker API stream timed out')), req.timeoutMs)
  }

  if (req.body !== undefined) {
    request.write(req.body)
  }
  request.end()

  return () => finish()
}

export interface DockerUpgradeHandlers {
  onUpgrade: (socket: Duplex, head: Buffer) => void
  onError: (error: Error) => void
  /** Called when the daemon responds without upgrading (usually an error). */
  onResponse: (status: number, body: string) => void
}

/**
 * Request that switches protocols (`Upgrade: tcp`) — used to attach to a
 * container exec session, where the connection becomes a raw bidirectional byte
 * stream after the 101 response.
 */
export function dockerUpgradeRequest(
  socket: SocketLike,
  req: DockerHttpRequest,
  handlers: DockerUpgradeHandlers
): void {
  const agent = oneShotAgent(socket)
  let settled = false

  const fail = (error: Error): void => {
    if (settled) {
      return
    }
    settled = true
    agent.destroy()
    try {
      socket.destroy()
    } catch {
      /* already gone */
    }
    handlers.onError(error)
  }

  const request = http.request({ ...requestOptions(req), agent })

  request.on('upgrade', (res, upgraded, head) => {
    if (settled) {
      upgraded.destroy()
      return
    }
    settled = true
    if ((res.statusCode ?? 0) !== 101) {
      upgraded.destroy()
      handlers.onError(new DockerApiError(res.statusCode ?? 0, res.statusMessage ?? 'Upgrade failed'))
      return
    }
    handlers.onUpgrade(upgraded, head ?? Buffer.alloc(0))
  })

  request.on('response', (res) => {
    let body = ''
    res.setEncoding('utf8')
    res.on('data', (chunk: string) => {
      body += chunk
    })
    res.on('end', () => {
      if (settled) {
        return
      }
      settled = true
      agent.destroy()
      try {
        socket.destroy()
      } catch {
        /* already gone */
      }
      handlers.onResponse(res.statusCode ?? 0, body)
    })
  })

  request.on('error', (error) => fail(normalizeError(error)))

  if (req.timeoutMs && req.timeoutMs > 0) {
    setTimeout(() => fail(new Error('Docker API request timed out')), req.timeoutMs)
  }

  if (req.body !== undefined) {
    request.write(req.body)
  }
  request.end()
}
