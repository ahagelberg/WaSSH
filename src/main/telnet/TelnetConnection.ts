import { Socket } from 'net'
import { DEFAULT_TERM_COLS, DEFAULT_TERM_ROWS } from '../../shared/types'
import { ByteSession } from '../session/ByteSession'

/** Telnet IAC (Interpret As Command) */
const IAC = 255
const DONT = 254
const DO = 253
const WONT = 252
const WILL = 251
const SB = 250
const SE = 240
const NOP = 241
const GA = 249

const OPT_ECHO = 1
const OPT_SUPPRESS_GA = 3
const OPT_TERMINAL_TYPE = 24
const OPT_NAWS = 31

const TTYPE_IS = 0
const TTYPE_SEND = 1

const STATE_DATA = 0
const STATE_IAC = 1
const STATE_WILL = 2
const STATE_WONT = 3
const STATE_DO = 4
const STATE_DONT = 5
const STATE_SB = 6
const STATE_SB_IAC = 7

const UINT16_HIGH = 8
const BYTE_MASK = 0xff
const UINT16_MAX = 65535

function u16Bytes(value: number): [number, number] {
  const clamped = Math.max(0, Math.min(UINT16_MAX, value))
  return [clamped >> UINT16_HIGH, clamped & BYTE_MASK]
}

function iacEscape(bytes: number[]): number[] {
  const out: number[] = []
  for (const b of bytes) {
    out.push(b)
    if (b === IAC) {
      out.push(IAC)
    }
  }
  return out
}

class TelnetFilter {
  private state = STATE_DATA
  private sub: number[] = []
  nawsEnabled = false
  termType = 'xterm-256color'
  cols = DEFAULT_TERM_COLS
  rows = DEFAULT_TERM_ROWS

  constructor(private send: (bytes: number[]) => void) {}

  feed(chunk: Buffer): Buffer {
    const app: number[] = []
    for (const byte of chunk) {
      this.step(byte, app)
    }
    return Buffer.from(app)
  }

  escapeWrite(data: string): Buffer {
    const raw = Buffer.from(data, 'utf8')
    const out: number[] = []
    for (const b of raw) {
      out.push(b)
      if (b === IAC) {
        out.push(IAC)
      }
    }
    return Buffer.from(out)
  }

  start(): void {
    this.send([
      IAC, DO, OPT_SUPPRESS_GA,
      IAC, WILL, OPT_SUPPRESS_GA,
      IAC, DO, OPT_ECHO,
      IAC, WILL, OPT_NAWS,
      IAC, WILL, OPT_TERMINAL_TYPE
    ])
    this.nawsEnabled = true
  }

  sendNaws(): void {
    if (!this.nawsEnabled) {
      return
    }
    const [ch, cl] = u16Bytes(this.cols)
    const [rh, rl] = u16Bytes(this.rows)
    this.send([IAC, SB, OPT_NAWS, ...iacEscape([ch, cl, rh, rl]), IAC, SE])
  }

  private step(byte: number, app: number[]): void {
    switch (this.state) {
      case STATE_DATA:
        if (byte === IAC) {
          this.state = STATE_IAC
        } else {
          app.push(byte)
        }
        return
      case STATE_IAC:
        if (byte === IAC) {
          app.push(IAC)
          this.state = STATE_DATA
          return
        }
        if (byte === WILL) {
          this.state = STATE_WILL
          return
        }
        if (byte === WONT) {
          this.state = STATE_WONT
          return
        }
        if (byte === DO) {
          this.state = STATE_DO
          return
        }
        if (byte === DONT) {
          this.state = STATE_DONT
          return
        }
        if (byte === SB) {
          this.sub = []
          this.state = STATE_SB
          return
        }
        if (byte === NOP || byte === GA) {
          this.state = STATE_DATA
          return
        }
        this.state = STATE_DATA
        return
      case STATE_WILL:
        this.onWill(byte)
        this.state = STATE_DATA
        return
      case STATE_WONT:
        if (byte === OPT_NAWS) {
          this.nawsEnabled = false
        }
        this.state = STATE_DATA
        return
      case STATE_DO:
        this.onDo(byte)
        this.state = STATE_DATA
        return
      case STATE_DONT:
        if (byte === OPT_NAWS) {
          this.nawsEnabled = false
        }
        this.state = STATE_DATA
        return
      case STATE_SB:
        if (byte === IAC) {
          this.state = STATE_SB_IAC
        } else {
          this.sub.push(byte)
        }
        return
      case STATE_SB_IAC:
        if (byte === IAC) {
          this.sub.push(IAC)
          this.state = STATE_SB
          return
        }
        if (byte === SE) {
          this.onSub(this.sub)
          this.sub = []
          this.state = STATE_DATA
          return
        }
        this.state = STATE_SB
        return
      default:
        this.state = STATE_DATA
    }
  }

  private onWill(opt: number): void {
    if (opt === OPT_ECHO || opt === OPT_SUPPRESS_GA) {
      this.send([IAC, DO, opt])
      return
    }
    this.send([IAC, DONT, opt])
  }

  private onDo(opt: number): void {
    if (opt === OPT_SUPPRESS_GA || opt === OPT_TERMINAL_TYPE) {
      this.send([IAC, WILL, opt])
      return
    }
    if (opt === OPT_NAWS) {
      this.nawsEnabled = true
      this.send([IAC, WILL, OPT_NAWS])
      this.sendNaws()
      return
    }
    this.send([IAC, WONT, opt])
  }

  private onSub(sub: number[]): void {
    if (sub[0] === OPT_TERMINAL_TYPE && sub[1] === TTYPE_SEND) {
      const name = Buffer.from(this.termType, 'ascii')
      this.send([IAC, SB, OPT_TERMINAL_TYPE, TTYPE_IS, ...name, IAC, SE])
    }
  }
}

export class TelnetConnection extends ByteSession {
  private socket: Socket | null = null
  private filter: TelnetFilter | null = null

  write(data: string): void {
    const sock = this.socket
    const filter = this.filter
    if (!sock || !filter) {
      return
    }
    sock.write(filter.escapeWrite(data))
  }

  resize(cols: number, rows: number): void {
    this.cols = cols
    this.rows = rows
    if (this.filter) {
      this.filter.cols = cols
      this.filter.rows = rows
      this.filter.sendNaws()
    }
  }

  protected async open(): Promise<void> {
    if (this.disposed) {
      return
    }
    this.clearReconnectTimer()
    this.closeTransport()
    this.remoteEnded = false
    this.emitStatus('connecting')

    const host = this.connection.host.trim()
    const port = this.connection.port
    if (!host) {
      this.emitStatus('failed', 'Host is required')
      return
    }

    const socket = new Socket()
    this.socket = socket
    const filter = new TelnetFilter((bytes) => {
      if (!socket.destroyed) {
        socket.write(Buffer.from(bytes))
      }
    })
    filter.termType = this.termType
    filter.cols = this.cols
    filter.rows = this.rows
    this.filter = filter

    try {
      await new Promise<void>((resolve, reject) => {
        const onErr = (err: Error): void => {
          cleanup()
          reject(err)
        }
        const cleanup = (): void => {
          socket.removeListener('error', onErr)
          socket.removeListener('connect', onConnect)
        }
        const onConnect = (): void => {
          cleanup()
          resolve()
        }
        socket.once('error', onErr)
        socket.once('connect', onConnect)
        socket.connect(port, host)
      })
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      this.emitStatus('failed', msg)
      this.closeTransport()
      this.scheduleReconnect()
      return
    }

    if (this.disposed || this.intentionalDisconnect) {
      this.closeTransport()
      return
    }

    socket.on('data', (chunk: Buffer) => {
      const app = filter.feed(chunk)
      if (app.length > 0) {
        this.emit('data', app.toString('utf8'))
      }
    })
    socket.on('error', (err) => {
      if (this.intentionalDisconnect || this.disposed) {
        return
      }
      this.emitStatus('disconnected', err.message)
      this.scheduleReconnect()
    })
    socket.on('close', () => {
      this.handleTransportClose()
    })

    filter.start()
    filter.sendNaws()
    this.markConnected()
  }

  protected closeTransport(): void {
    const sock = this.socket
    this.socket = null
    this.filter = null
    if (!sock) {
      return
    }
    sock.removeAllListeners()
    try {
      sock.destroy()
    } catch {
      /* ignore */
    }
  }
}
