import { existsSync } from 'fs'
import { homedir } from 'os'
import { spawn, type IDisposable, type IPty } from 'node-pty'
import {
  DEFAULT_TERM_TYPE,
  RECONNECT_MODE_NONE,
  type ConnectionParams
} from '../../shared/types'
import { ByteSession } from './ByteSession'

interface ShellCommand {
  file: string
  args: string[]
}

function localShell(): ShellCommand {
  if (process.platform === 'win32') {
    return { file: process.env.POWERSHELL_EXE || 'powershell.exe', args: ['-NoLogo'] }
  }

  const configuredShell = process.env.SHELL
  const fallbackShell = process.platform === 'darwin' ? '/bin/zsh' : '/bin/bash'
  const file = configuredShell && existsSync(configuredShell)
    ? configuredShell
    : existsSync(fallbackShell)
      ? fallbackShell
      : '/bin/sh'
  return { file, args: ['-l'] }
}

export class LocalConnection extends ByteSession {
  private terminal: IPty | null = null
  private dataListener: IDisposable | null = null
  private exitListener: IDisposable | null = null

  constructor(tabId: string, connection: ConnectionParams, isAppFocused: () => boolean) {
    super(tabId, connection, isAppFocused)
    this.setReconnectPolicy(RECONNECT_MODE_NONE)
  }

  protected isTransportOpen(): boolean {
    return this.terminal !== null
  }

  write(data: string): void {
    try {
      this.terminal?.write(data)
    } catch {
      return
    }
  }

  resize(cols: number, rows: number): void {
    this.cols = Math.max(1, Math.floor(cols))
    this.rows = Math.max(1, Math.floor(rows))
    try {
      this.terminal?.resize(this.cols, this.rows)
    } catch {
      return
    }
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

    const shell = localShell()
    const termType = this.termType || DEFAULT_TERM_TYPE
    let terminal: IPty
    try {
      terminal = spawn(shell.file, shell.args, {
        name: termType,
        cols: this.cols,
        rows: this.rows,
        cwd: homedir(),
        env: { ...process.env, TERM: termType }
      })
    } catch (err) {
      this.opening = false
      this.emitStatus('failed', err instanceof Error ? err.message : String(err))
      return
    }

    this.terminal = terminal
    this.dataListener = terminal.onData((data) => this.emit('data', data))
    this.exitListener = terminal.onExit(() => {
      if (this.terminal !== terminal) {
        return
      }
      this.terminal = null
      this.disposeListeners()
      this.remoteEnded = true
      this.handleTransportClose()
    })
    this.markConnected()
  }

  protected closeTransport(): void {
    const terminal = this.terminal
    this.terminal = null
    this.disposeListeners()
    if (!terminal) {
      return
    }
    try {
      terminal.kill()
    } catch {
      return
    }
  }

  prepareForSleep(): void {}

  private disposeListeners(): void {
    this.dataListener?.dispose()
    this.exitListener?.dispose()
    this.dataListener = null
    this.exitListener = null
  }
}