import { connect, createServer, type Server, type Socket } from 'net'
import type { Client, ClientChannel } from 'ssh2'
import {
  DEFAULT_X11_HOST,
  TUNNEL_TYPE_DYNAMIC,
  TUNNEL_TYPE_LOCAL,
  TUNNEL_TYPE_REMOTE,
  X11_TCP_BASE_PORT,
  type SshTunnel
} from '../../shared/types'

/** SOCKS5 version byte */
const SOCKS5_VERSION = 0x05
/** SOCKS5 no-auth method */
const SOCKS5_NO_AUTH = 0x00
/** SOCKS5 CONNECT command */
const SOCKS5_CMD_CONNECT = 0x01
/** SOCKS5 address types */
const SOCKS5_ATYP_IPV4 = 0x01
const SOCKS5_ATYP_DOMAIN = 0x03
const SOCKS5_ATYP_IPV6 = 0x04
/** SOCKS5 success reply */
const SOCKS5_REP_SUCCESS = 0x00
/** SOCKS5 general failure reply */
const SOCKS5_REP_FAILURE = 0x01
/** Bytes to wait for before treating SOCKS greeting as incomplete */
const SOCKS_GREETING_MIN_BYTES = 2
/** Source IP reported to the SSH server for local→remote forwards */
const FORWARD_SRC_IP = '127.0.0.1'
/** Source port reported to the SSH server for local→remote forwards */
const FORWARD_SRC_PORT = 0

function remoteBindKey(host: string, port: number): string {
  return `${host}:${port}`
}

function tunnelLabel(tunnel: SshTunnel): string {
  if (tunnel.type === TUNNEL_TYPE_DYNAMIC) {
    return `D ${tunnel.listenHost}:${tunnel.listenPort}`
  }
  const arrow = tunnel.type === TUNNEL_TYPE_LOCAL ? 'L' : 'R'
  return `${arrow} ${tunnel.listenHost}:${tunnel.listenPort} → ${tunnel.destHost}:${tunnel.destPort}`
}

function pipeBidirectional(left: Socket | ClientChannel, right: Socket | ClientChannel): void {
  left.pipe(right)
  right.pipe(left)
  const closeBoth = (): void => {
    left.destroy()
    right.destroy()
  }
  left.on('error', closeBoth)
  right.on('error', closeBoth)
  left.on('close', () => right.destroy())
  right.on('close', () => left.destroy())
}

function localX11Endpoint(): { host: string; port: number } {
  const display = process.env.DISPLAY || process.env.WASSH_DISPLAY || ''
  const match = /^(?:([^:]+):)?(\d+)(?:\.(\d+))?$/.exec(display.trim())
  if (!match) {
    return { host: DEFAULT_X11_HOST, port: X11_TCP_BASE_PORT }
  }
  const hostPart = match[1]
  const displayNum = Number(match[2])
  const host =
    !hostPart || hostPart === 'unix' || hostPart.startsWith('/')
      ? DEFAULT_X11_HOST
      : hostPart
  return { host, port: X11_TCP_BASE_PORT + displayNum }
}

function readSocksGreeting(buf: Buffer): { ok: true; consumed: number } | { ok: false } | null {
  if (buf.length < SOCKS_GREETING_MIN_BYTES) {
    return null
  }
  if (buf[0] !== SOCKS5_VERSION) {
    return { ok: false }
  }
  const methodCount = buf[1]
  const need = SOCKS_GREETING_MIN_BYTES + methodCount
  if (buf.length < need) {
    return null
  }
  return { ok: true, consumed: need }
}

function parseSocksConnect(
  buf: Buffer
): { host: string; port: number; consumed: number } | { error: true } | null {
  const headerLen = 4
  if (buf.length < headerLen) {
    return null
  }
  if (buf[0] !== SOCKS5_VERSION || buf[1] !== SOCKS5_CMD_CONNECT) {
    return { error: true }
  }
  const atyp = buf[3]
  let offset = headerLen
  let host = ''
  if (atyp === SOCKS5_ATYP_IPV4) {
    if (buf.length < offset + 4 + 2) {
      return null
    }
    host = `${buf[offset]}.${buf[offset + 1]}.${buf[offset + 2]}.${buf[offset + 3]}`
    offset += 4
  } else if (atyp === SOCKS5_ATYP_DOMAIN) {
    if (buf.length < offset + 1) {
      return null
    }
    const len = buf[offset]
    offset += 1
    if (buf.length < offset + len + 2) {
      return null
    }
    host = buf.subarray(offset, offset + len).toString('utf8')
    offset += len
  } else if (atyp === SOCKS5_ATYP_IPV6) {
    if (buf.length < offset + 16 + 2) {
      return null
    }
    const parts: string[] = []
    for (let i = 0; i < 8; i++) {
      parts.push(buf.readUInt16BE(offset + i * 2).toString(16))
    }
    host = parts.join(':')
    offset += 16
  } else {
    return { error: true }
  }
  const port = buf.readUInt16BE(offset)
  offset += 2
  return { host, port, consumed: offset }
}

function socksReply(rep: number): Buffer {
  return Buffer.from([
    SOCKS5_VERSION,
    rep,
    0x00,
    SOCKS5_ATYP_IPV4,
    0,
    0,
    0,
    0,
    0,
    0
  ])
}

export class TunnelManager {
  private client: Client | null = null
  private localServers: Server[] = []
  private remoteBinds: Array<{ host: string; port: number }> = []
  private remoteRoutes = new Map<string, { destHost: string; destPort: number }>()
  private tcpHandler: ((...args: unknown[]) => void) | null = null
  private x11Handler: ((...args: unknown[]) => void) | null = null

  constructor(private readonly onNotice: (message: string) => void) {}

  async start(client: Client, tunnels: SshTunnel[], x11Forwarding: boolean): Promise<void> {
    this.stop()
    this.client = client
    if (x11Forwarding) {
      this.attachX11(client)
    }
    for (const tunnel of tunnels) {
      if (!tunnel.enabled || tunnel.listenPort <= 0) {
        continue
      }
      if (
        tunnel.type !== TUNNEL_TYPE_DYNAMIC &&
        (!tunnel.destHost || tunnel.destPort <= 0)
      ) {
        this.onNotice(`Skipped incomplete tunnel ${tunnelLabel(tunnel)}`)
        continue
      }
      try {
        await this.startTunnel(tunnel)
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        this.onNotice(`Tunnel failed (${tunnelLabel(tunnel)}): ${msg}`)
      }
    }
  }

  stop(): void {
    for (const server of this.localServers) {
      server.removeAllListeners()
      server.on('error', () => {
        /* absorb errors from closing a dead listen socket */
      })
      try {
        server.close()
      } catch {
        /* ignore */
      }
    }
    this.localServers = []

    const client = this.client
    if (client && this.tcpHandler) {
      client.removeListener('tcp connection', this.tcpHandler)
      this.tcpHandler = null
    }
    if (client && this.x11Handler) {
      client.removeListener('x11', this.x11Handler)
      this.x11Handler = null
    }
    if (client) {
      for (const bind of this.remoteBinds) {
        try {
          client.unforwardIn(bind.host, bind.port)
        } catch {
          /* ignore */
        }
      }
    }
    this.remoteBinds = []
    this.remoteRoutes.clear()
    this.client = null
  }

  private attachX11(client: Client): void {
    const endpoint = localX11Endpoint()
    const handler = (
      _details: unknown,
      accept: () => ClientChannel,
      reject: () => void
    ): void => {
      const xserversock = connect(endpoint.port, endpoint.host)
      xserversock.on('connect', () => {
        const xclientsock = accept()
        pipeBidirectional(xclientsock, xserversock)
      })
      xserversock.on('error', () => {
        reject()
        this.onNotice(
          `X11 forwarding failed: cannot reach local display ${endpoint.host}:${endpoint.port}`
        )
      })
    }
    this.x11Handler = handler as (...args: unknown[]) => void
    client.on('x11', handler)
  }

  private ensureRemoteTcpHandler(client: Client): void {
    if (this.tcpHandler) {
      return
    }
    const handler = (
      details: { destIP: string; destPort: number },
      accept: () => ClientChannel,
      reject: () => void
    ): void => {
      const route =
        this.remoteRoutes.get(remoteBindKey(details.destIP, details.destPort)) ||
        this.remoteRoutes.get(remoteBindKey('127.0.0.1', details.destPort)) ||
        this.remoteRoutes.get(remoteBindKey('localhost', details.destPort)) ||
        this.remoteRoutes.get(remoteBindKey('0.0.0.0', details.destPort)) ||
        this.remoteRoutes.get(remoteBindKey('::', details.destPort))
      if (!route) {
        reject()
        return
      }
      const local = connect(route.destPort, route.destHost)
      local.on('connect', () => {
        const channel = accept()
        pipeBidirectional(channel, local)
      })
      local.on('error', () => {
        reject()
      })
    }
    this.tcpHandler = handler as (...args: unknown[]) => void
    client.on('tcp connection', handler)
  }

  private startTunnel(tunnel: SshTunnel): Promise<void> {
    const client = this.client
    if (!client) {
      return Promise.reject(new Error('No SSH client'))
    }
    if (tunnel.type === TUNNEL_TYPE_REMOTE) {
      return this.startRemote(client, tunnel)
    }
    if (tunnel.type === TUNNEL_TYPE_DYNAMIC) {
      return this.startDynamic(client, tunnel)
    }
    return this.startLocal(client, tunnel)
  }

  private startLocal(client: Client, tunnel: SshTunnel): Promise<void> {
    return new Promise((resolve, reject) => {
      const server = createServer((socket) => {
        socket.on('error', () => {
          socket.destroy()
        })
        client.forwardOut(
          FORWARD_SRC_IP,
          FORWARD_SRC_PORT,
          tunnel.destHost,
          tunnel.destPort,
          (err, stream) => {
            if (err || !stream) {
              socket.destroy()
              return
            }
            pipeBidirectional(socket, stream)
          }
        )
      })
      server.once('error', reject)
      server.listen(tunnel.listenPort, tunnel.listenHost, () => {
        server.removeListener('error', reject)
        server.on('error', () => {
          /* absorb listen-socket errors after the tunnel is up */
        })
        this.localServers.push(server)
        resolve()
      })
    })
  }

  private startRemote(client: Client, tunnel: SshTunnel): Promise<void> {
    this.ensureRemoteTcpHandler(client)
    return new Promise((resolve, reject) => {
      client.forwardIn(tunnel.listenHost, tunnel.listenPort, (err, boundPort) => {
        if (err) {
          reject(err)
          return
        }
        const port = boundPort || tunnel.listenPort
        this.remoteBinds.push({ host: tunnel.listenHost, port })
        this.remoteRoutes.set(remoteBindKey(tunnel.listenHost, port), {
          destHost: tunnel.destHost,
          destPort: tunnel.destPort
        })
        resolve()
      })
    })
  }

  private startDynamic(client: Client, tunnel: SshTunnel): Promise<void> {
    return new Promise((resolve, reject) => {
      const server = createServer((socket) => {
        this.handleSocksClient(client, socket)
      })
      server.once('error', reject)
      server.listen(tunnel.listenPort, tunnel.listenHost, () => {
        server.removeListener('error', reject)
        server.on('error', () => {
          /* absorb listen-socket errors after the tunnel is up */
        })
        this.localServers.push(server)
        resolve()
      })
    })
  }

  private handleSocksClient(client: Client, socket: Socket): void {
    let buf = Buffer.alloc(0)
    let phase: 'greeting' | 'request' | 'proxy' = 'greeting'

    const onData = (chunk: Buffer): void => {
      if (phase === 'proxy') {
        return
      }
      buf = Buffer.concat([buf, chunk])
      if (phase === 'greeting') {
        const greeting = readSocksGreeting(buf)
        if (greeting === null) {
          return
        }
        if (!greeting.ok) {
          socket.destroy()
          return
        }
        buf = buf.subarray(greeting.consumed)
        socket.write(Buffer.from([SOCKS5_VERSION, SOCKS5_NO_AUTH]))
        phase = 'request'
      }
      if (phase === 'request') {
        const req = parseSocksConnect(buf)
        if (req === null) {
          return
        }
        if ('error' in req) {
          socket.write(socksReply(SOCKS5_REP_FAILURE))
          socket.destroy()
          return
        }
        buf = buf.subarray(req.consumed)
        socket.removeListener('data', onData)
        phase = 'proxy'
        client.forwardOut(FORWARD_SRC_IP, FORWARD_SRC_PORT, req.host, req.port, (err, stream) => {
          if (err || !stream) {
            socket.write(socksReply(SOCKS5_REP_FAILURE))
            socket.destroy()
            return
          }
          socket.write(socksReply(SOCKS5_REP_SUCCESS))
          if (buf.length > 0) {
            stream.write(buf)
          }
          pipeBidirectional(socket, stream)
        })
      }
    }

    socket.on('data', onData)
    socket.on('error', () => {
      socket.destroy()
    })
  }
}
