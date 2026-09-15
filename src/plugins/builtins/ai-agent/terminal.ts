import type { PluginMainContext } from '@plugin-api/main'

/** Max characters of terminal output retained per tab */
export const TERMINAL_BUFFER_CHARS = 64_000
/** Max characters returned to the model in one terminal_read call */
export const TERMINAL_READ_MAX_CHARS = 24_000
/** Poll interval while waiting for terminal output to settle */
export const TERMINAL_WAIT_POLL_MS = 120
/** Default/max time to wait for terminal output to settle (ms) */
export const TERMINAL_WAIT_DEFAULT_MS = 5_000
export const TERMINAL_WAIT_MAX_MS = 60_000
/** Quiet period after the last output chunk before the terminal is "settled" (ms) */
export const TERMINAL_SETTLE_MS = 400
/** Max characters accepted in one terminal_write call */
export const TERMINAL_WRITE_MAX_CHARS = 4_000

/**
 * CSI / OSC / single-character escape sequences, plus the control characters
 * a PTY emits. Without stripping these, the model sees raw cursor-positioning
 * noise instead of the text a human would read on screen.
 */
// eslint-disable-next-line no-control-regex
const ANSI_OSC_RE = /\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)/g
// eslint-disable-next-line no-control-regex
const ANSI_CSI_RE = /\u001b\[[0-9;?]*[ -/]*[@-~]/g
// eslint-disable-next-line no-control-regex
const ANSI_ESCAPE_RE = /\u001b[@-Z\\-_]/g
// eslint-disable-next-line no-control-regex
const CONTROL_CHARS_RE = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g

/** Strip ANSI escape sequences and non-printing control characters. */
export function stripAnsi(text: string): string {
  return text
    .replace(ANSI_OSC_RE, '')
    .replace(ANSI_CSI_RE, '')
    .replace(ANSI_ESCAPE_RE, '')
    .replace(/\r\n/g, '\n')
    .replace(/\r/g, '\n')
    .replace(CONTROL_CHARS_RE, '')
}

/** Keys the agent may send, mapped to the byte sequence a PTY expects. */
const TERMINAL_KEY_SEQUENCES: Record<string, string> = {
  enter: '\r',
  tab: '\t',
  escape: '\u001b',
  backspace: '\u007f',
  space: ' ',
  up: '\u001b[A',
  down: '\u001b[B',
  right: '\u001b[C',
  left: '\u001b[D',
  home: '\u001b[H',
  end: '\u001b[F',
  pageup: '\u001b[5~',
  pagedown: '\u001b[6~',
  'ctrl+a': '\u0001',
  'ctrl+b': '\u0002',
  'ctrl+c': '\u0003',
  'ctrl+d': '\u0004',
  'ctrl+e': '\u0005',
  'ctrl+f': '\u0006',
  'ctrl+g': '\u0007',
  'ctrl+k': '\u000b',
  'ctrl+l': '\u000c',
  'ctrl+n': '\u000e',
  'ctrl+p': '\u0010',
  'ctrl+r': '\u0012',
  'ctrl+u': '\u0015',
  'ctrl+w': '\u0017',
  'ctrl+x': '\u0018',
  'ctrl+y': '\u0019',
  'ctrl+z': '\u001a'
}

/** Canonical key names, for error messages. */
export const TERMINAL_KEY_NAMES = Object.keys(TERMINAL_KEY_SEQUENCES)

/** Resolve a key name to its PTY byte sequence, or null when unknown. */
export function terminalKeySequence(name: string): string | null {
  return TERMINAL_KEY_SEQUENCES[name.trim().toLowerCase()] ?? null
}

/**
 * Rolling view of one tab's live PTY output.
 *
 * The raw stream is kept (so escape sequences that span chunks are stripped
 * correctly) and the cleaned text is derived on read. Only inbound data is
 * tracked - what the user sees on screen - not what the agent writes.
 */
export class TerminalBuffer {
  private raw = ''
  /** Byte offset of the end of the stream, used to detect new output. */
  private offset = 0
  private lastOutputAt = 0
  /** True once any output has arrived, so "never produced output" is distinguishable. */
  private hasOutput = false

  append(data: string): void {
    this.raw += data
    if (this.raw.length > TERMINAL_BUFFER_CHARS) {
      this.raw = this.raw.slice(-TERMINAL_BUFFER_CHARS)
    }
    this.offset += data.length
    this.lastOutputAt = Date.now()
    this.hasOutput = true
  }

  /** Current stream offset (monotonic, survives trimming). */
  get currentOffset(): number {
    return this.offset
  }

  /** Milliseconds since the last inbound chunk, or Infinity when none yet. */
  idleMs(): number {
    return this.hasOutput ? Date.now() - this.lastOutputAt : Number.POSITIVE_INFINITY
  }

  /** Cleaned terminal text, optionally only the last `maxChars` characters. */
  text(maxChars?: number): string {
    const clean = stripAnsi(this.raw)
    if (maxChars === undefined || clean.length <= maxChars) {
      return clean
    }
    return clean.slice(-maxChars)
  }

  /**
   * True when no output has arrived for at least `TERMINAL_SETTLE_MS`.
   * A buffer that has never received output is *not* settled: a just-issued
   * command has not started printing yet, so waiting must keep waiting.
   */
  isSettled(): boolean {
    return this.hasOutput && this.idleMs() >= TERMINAL_SETTLE_MS
  }
}

/** Per-tab terminal buffers, keyed by the plugin main context. */
const buffersByContext = new WeakMap<PluginMainContext, TerminalBuffer>()

/** The terminal buffer for a tab, created on first use. */
export function terminalBufferFor(ctx: PluginMainContext): TerminalBuffer {
  let buffer = buffersByContext.get(ctx)
  if (!buffer) {
    buffer = new TerminalBuffer()
    buffersByContext.set(ctx, buffer)
  }
  return buffer
}

/**
 * Wait until the terminal has produced no output for `TERMINAL_SETTLE_MS`,
 * or `timeoutMs` elapses. Resolves with whether it settled.
 */
export async function waitForTerminalSettle(
  buffer: TerminalBuffer,
  timeoutMs: number
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (buffer.isSettled()) {
      return true
    }
    await new Promise((resolve) => setTimeout(resolve, TERMINAL_WAIT_POLL_MS))
  }
  return buffer.isSettled()
}

function describeTerminalState(buffer: TerminalBuffer): string {
  const idle = buffer.idleMs()
  if (idle === Number.POSITIVE_INFINITY) {
    return 'no output received yet'
  }
  return idle >= TERMINAL_SETTLE_MS ? 'idle' : `still producing output (${idle}ms since last chunk)`
}
/** Read the current terminal contents. */
export function executeTerminalRead(buffer: TerminalBuffer, maxChars?: number): string {
  const limit = Math.min(Math.max(Number(maxChars) || TERMINAL_READ_MAX_CHARS, 200), TERMINAL_READ_MAX_CHARS)
  const text = buffer.text(limit)
  if (!text.trim()) {
    return `Terminal is empty (${describeTerminalState(buffer)}).`
  }
  const truncated = buffer.text().length > text.length
  return (
    `Terminal contents${truncated ? ` (last ${limit} characters)` : ''}:\n` +
    '```\n' +
    text +
    '\n```\n' +
    `[${describeTerminalState(buffer)}]`
  )
}

/** Write text into the live terminal PTY. */
export function executeTerminalWrite(
  ctx: PluginMainContext,
  buffer: TerminalBuffer,
  text: string,
  pressEnter: boolean
): string {
  if (text.length > TERMINAL_WRITE_MAX_CHARS) {
    return `Error: text exceeds ${TERMINAL_WRITE_MAX_CHARS} characters. Send it in smaller pieces.`
  }
  ctx.writeToSession(pressEnter ? `${text}\r` : text)
  const shown = text.length > 200 ? `${text.slice(0, 200)}…` : text
  return `Wrote to terminal${pressEnter ? ' and pressed Enter' : ''}: ${JSON.stringify(shown)}`
}

/** Send named keys to the live terminal. */
export function executeTerminalKeys(
  ctx: PluginMainContext,
  buffer: TerminalBuffer,
  keys: string[]
): string {
  if (keys.length === 0) {
    return 'Error: keys must contain at least one key name.'
  }
  const sequences: string[] = []
  for (const key of keys) {
    const sequence = terminalKeySequence(key)
    if (sequence === null) {
      return `Error: unknown key "${key}". Valid keys: ${TERMINAL_KEY_NAMES.join(', ')}.`
    }
    sequences.push(sequence)
  }
  ctx.writeToSession(sequences.join(''))
  return `Sent keys: ${keys.join(', ')}`
}

/**
 * Wait for the terminal to settle, then return its contents. This is the
 * primary way to observe the result of a command typed into the live shell.
 */
export async function executeTerminalWait(
  buffer: TerminalBuffer,
  timeoutMs?: number,
  maxChars?: number
): Promise<string> {
  const timeout = Math.min(
    Math.max(Number(timeoutMs) || TERMINAL_WAIT_DEFAULT_MS, TERMINAL_SETTLE_MS),
    TERMINAL_WAIT_MAX_MS
  )
  const startOffset = buffer.currentOffset
  const settled = await waitForTerminalSettle(buffer, timeout)
  const producedOutput = buffer.currentOffset !== startOffset
  const body = executeTerminalRead(buffer, maxChars)
  const status = settled
    ? producedOutput
      ? 'Terminal settled after producing output.'
      : 'Terminal settled with no new output.'
    : `Terminal still busy after ${timeout}ms.`
  return `${status}\n${body}`
}
