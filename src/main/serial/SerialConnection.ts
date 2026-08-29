import type { SerialPort } from 'serialport'
import {
  SERIAL_FLOW_RTSCTS,
  SERIAL_FLOW_XONXOFF,
  type SerialFlowControl
} from '../../shared/types'
import { serialConfigFrom } from '../../shared/connection'
import { ByteSession } from '../session/ByteSession'

function flowFlags(flow: SerialFlowControl): { rtscts: boolean; xon: boolean; xoff: boolean } {
  if (flow === SERIAL_FLOW_RTSCTS) {
    return { rtscts: true, xon: false, xoff: false }
  }
  if (flow === SERIAL_FLOW_XONXOFF) {
    return { rtscts: false, xon: true, xoff: true }
  }
  return { rtscts: false, xon: false, xoff: false }
}

export class SerialConnection extends ByteSession {
  private port: SerialPort | null = null

  protected isTransportOpen(): boolean {
    return Boolean(this.port?.isOpen)
  }

  write(data: string): void {
    try {
      this.port?.write(data)
    } catch {
      /* ignore */
    }
  }

  resize(cols: number, rows: number): void {
    this.cols = cols
    this.rows = rows
  }

  protected async open(): Promise<void> {
    if (this.disposed) {
      return
    }
    this.opening = true
    this.clearReconnectTimer()
    this.closeTransport()
    this.remoteEnded = false
    this.emitStatus('connecting')

    const path = this.connection.host.trim()
    if (!path) {
      this.opening = false
      this.emitStatus('failed', 'Serial port is required')
      return
    }

    const cfg = serialConfigFrom(this.connection)
    const flow = flowFlags(cfg.serialFlowControl)

    let SerialPortCtor: typeof SerialPort
    try {
      const mod = await import('serialport')
      SerialPortCtor = mod.SerialPort
    } catch (err) {
      this.opening = false
      const msg = err instanceof Error ? err.message : String(err)
      this.emitStatus('failed', `Serial support unavailable: ${msg}`)
      return
    }

    const port = new SerialPortCtor({
      path,
      baudRate: cfg.serialBaudRate,
      dataBits: cfg.serialDataBits,
      stopBits: cfg.serialStopBits,
      parity: cfg.serialParity,
      rtscts: flow.rtscts,
      xon: flow.xon,
      xoff: flow.xoff,
      autoOpen: false
    })
    this.port = port
    port.on('error', (err: Error) => {
      if (this.intentionalDisconnect || this.disposed || !this.everConnected) {
        return
      }
      this.emitStatus('disconnected', err.message)
      this.closeTransport()
      this.scheduleReconnect()
    })

    try {
      await new Promise<void>((resolve, reject) => {
        port.open((err) => {
          if (err) {
            reject(err)
            return
          }
          resolve()
        })
      })
    } catch (err) {
      this.opening = false
      const msg = err instanceof Error ? err.message : String(err)
      this.emitStatus('failed', msg)
      this.closeTransport()
      this.scheduleReconnect()
      return
    }

    if (this.disposed || this.intentionalDisconnect) {
      this.opening = false
      this.closeTransport()
      return
    }

    port.on('data', (chunk: Buffer) => {
      this.emit('data', chunk.toString('utf8'))
    })
    port.on('close', () => {
      this.handleTransportClose()
    })

    this.markConnected()
  }

  protected closeTransport(): void {
    const port = this.port
    this.port = null
    if (!port) {
      return
    }
    port.removeAllListeners()
    port.on('error', () => {
      /* absorb errors from closing a dead port */
    })
    if (port.isOpen) {
      try {
        port.close()
      } catch {
        /* ignore */
      }
    }
  }
}
