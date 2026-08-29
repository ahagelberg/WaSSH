import { EventEmitter } from 'events'
import {
  ConnectionParams,
  DEFAULT_TERM_COLS,
  DEFAULT_TERM_ROWS,
  DEFAULT_TERM_TYPE,
  HostKeyDecision,
  RECONNECT_INITIAL_BACKOFF_MS,
  RECONNECT_MAX_BACKOFF_MS,
  SavePasswordDecision,
  SessionStatus
} from '../../shared/types'

/** Status when the remote side ends the session (not a drop) */
const SESSION_CLOSED_MESSAGE = 'Session closed'

export abstract class ByteSession extends EventEmitter {
  protected disposed = false
  protected intentionalDisconnect = false
  protected everConnected = false
  protected remoteEnded = false
  protected reconnectAttempt = 0
  protected reconnectTimer: ReturnType<typeof setTimeout> | null = null
  protected autoReconnect = false
  protected maxReconnectAttempts = 0
  /** True while open() is connecting, before the transport is ready */
  protected opening = false
  protected cols = DEFAULT_TERM_COLS
  protected rows = DEFAULT_TERM_ROWS
  protected termType = DEFAULT_TERM_TYPE

  constructor(
    readonly tabId: string,
    protected connection: ConnectionParams
  ) {
    super()
  }

  getConnection(): ConnectionParams {
    return this.connection
  }

  setReconnectPolicy(enabled: boolean, maxAttempts: number): void {
    this.autoReconnect = enabled
    this.maxReconnectAttempts = maxAttempts
  }

  respondHostKey(_decision: HostKeyDecision): void {
    /* SSH only */
  }

  respondSavePassword(_decision: SavePasswordDecision, _hostName?: string): void {
    /* SSH only */
  }

  async connect(cols: number, rows: number, termType: string): Promise<void> {
    this.cols = cols
    this.rows = rows
    this.termType = termType
    this.intentionalDisconnect = false
    this.reconnectAttempt = 0
    this.everConnected = false
    this.remoteEnded = false
    await this.open()
  }

  disconnect(): void {
    this.intentionalDisconnect = true
    this.clearReconnectTimer()
    this.closeTransport()
    this.emitStatus('disconnected')
  }

  dispose(): void {
    this.disposed = true
    this.disconnect()
    this.removeAllListeners()
  }

  abstract write(data: string): void
  abstract resize(cols: number, rows: number): void

  protected abstract open(): Promise<void>
  protected abstract closeTransport(): void

  protected emitStatus(status: SessionStatus, message?: string): void {
    this.emit('status', status, message)
  }

  protected abstract isTransportOpen(): boolean

  prepareForSleep(): void {
    if (this.disposed || this.intentionalDisconnect) {
      return
    }
    this.clearReconnectTimer()
    if (!this.isTransportOpen() && !this.everConnected) {
      return
    }
    this.closeTransport()
    this.emitStatus('disconnected', 'Computer sleep')
  }

  reconnectNow(): void {
    if (this.disposed || this.intentionalDisconnect) {
      return
    }
    if (!this.everConnected) {
      return
    }
    if (this.isTransportOpen() || this.opening) {
      return
    }
    this.reconnectAttempt = 0
    this.clearReconnectTimer()
    void this.open()
  }

  protected markConnected(): void {
    this.opening = false
    this.reconnectAttempt = 0
    this.everConnected = true
    this.emitStatus('connected')
  }

  protected handleTransportClose(): void {
    if (this.intentionalDisconnect || this.disposed) {
      this.emitStatus('disconnected')
      return
    }
    if (this.remoteEnded) {
      this.intentionalDisconnect = true
      this.clearReconnectTimer()
      this.closeTransport()
      this.emitStatus('closed', SESSION_CLOSED_MESSAGE)
      return
    }
    this.emitStatus('disconnected', SESSION_CLOSED_MESSAGE)
    this.scheduleReconnect()
  }

  protected scheduleReconnect(): void {
    if (!this.autoReconnect || this.intentionalDisconnect || this.disposed || !this.everConnected) {
      return
    }
    if (this.reconnectTimer) {
      return
    }
    if (this.reconnectAttempt >= this.maxReconnectAttempts) {
      this.emitStatus('failed', 'Reconnect attempts exhausted')
      return
    }
    const attempt = this.reconnectAttempt
    this.reconnectAttempt += 1
    const delay = Math.min(
      RECONNECT_INITIAL_BACKOFF_MS * 2 ** attempt,
      RECONNECT_MAX_BACKOFF_MS
    )
    this.emitStatus(
      'reconnecting',
      `Attempt ${this.reconnectAttempt}/${this.maxReconnectAttempts}`
    )
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      void this.open()
    }, delay)
  }

  protected clearReconnectTimer(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
  }
}
