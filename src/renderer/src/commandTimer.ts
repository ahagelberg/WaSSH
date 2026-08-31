/**
 * Client-side command execution timer.
 *
 * Detects command completion by watching the session output for the next shell
 * prompt: prompts are drawn without a trailing newline, so while a command is
 * pending the only line that could be a prompt is the current, incomplete line.
 * When that line reads like a prompt, the elapsed time is inserted before the
 * prompt text. Lines terminated by \r/\n are command output and pass through.
 */

/** Longest line that can still be a prompt; anything longer is flushed as output */
const PROMPT_MAX_CHARS = 120

/** Prompt endings: $ # (with optional trailing spaces) */
const PROMPT_END_RE = /[$#] *$/

/** zsh/csh "%" prompts fire only with a trailing space (avoids matching progress bars like "50%") */
const PERCENT_PROMPT_RE = /% +$/

/** Rare ">"-prompts (PowerShell PS>, mysql>, Router>, >>>); only for short lines */
const GT_PROMPT_MAX_CHARS = 24
const GT_PROMPT_END_RE = /> *$/

const MS_PER_SEC = 1000
const SEC_PER_MIN = 60
const MIN_MS = SEC_PER_MIN * MS_PER_SEC

/** ANSI SGR for a dim (faint) timer; reset before the prompt so prompt colors apply */
const TIMER_DIM = '\x1b[2m'
const TIMER_RESET = '\x1b[0m'
const TIMER_SEP = ' '

/** Strip ANSI CSI / OSC / escape sequences for prompt matching */
function stripAnsi(s: string): string {
  return s
    .replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '')
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, '')
    .replace(/\x1b[()][0-9A-Za-z]/g, '')
    .replace(/\x1b[@-Z\\-_]/g, '')
}

/** CSI sequence that erases the current line/screen */
const CSI_ERASE_RE = /^\x1b\[[0-9;?]*[KJ]/

/** A shell may emit a leading erase before its prompt (\r ESC[2K …); replay it first so it does not wipe the timer */
function leadingErasePrefix(s: string): string {
  let i = 0
  while (s[i] === '\x1b') {
    const m = /^\x1b\[[0-9;?]*[ -/]*[@-~]/.exec(s.slice(i))
    if (!m || !CSI_ERASE_RE.test(m[0])) {
      break
    }
    i += m[0].length
  }
  return s.slice(0, i)
}

export function formatElapsed(ms: number): string {
  const t = Math.max(0, ms)
  if (t < MS_PER_SEC) {
    return `[${Math.round(t)}ms]`
  }
  if (t < MIN_MS) {
    return `[${(t / MS_PER_SEC).toFixed(2)}s]`
  }
  const m = Math.floor(t / MIN_MS)
  const s = Math.round((t % MIN_MS) / MS_PER_SEC)
  return `[${m}m ${s}s]`
}

function isPrompt(visible: string): boolean {
  if (visible.length === 0) {
    return false
  }
  if (PROMPT_END_RE.test(visible) || PERCENT_PROMPT_RE.test(visible)) {
    return true
  }
  return visible.length <= GT_PROMPT_MAX_CHARS && GT_PROMPT_END_RE.test(visible)
}

export class CommandTimer {
  private startedAt: number | null = null
  private buf = ''
  private inCandidate = false
  private atLineStart = false
  private readonly now: () => number

  constructor(now: () => number = Date.now) {
    this.now = now
  }

  get pending(): boolean {
    return this.startedAt !== null
  }

  /** The user submitted a command; start measuring. */
  start(): void {
    this.startedAt = this.now()
  }

  /** Drop any in-flight measurement and buffered candidate. */
  reset(): void {
    this.startedAt = null
    this.buf = ''
    this.inCandidate = false
    this.atLineStart = false
  }

  /**
   * Feed a chunk of session output. While a command is pending, the current
   * incomplete line is held back until it can be classified; if it is the next
   * prompt, the elapsed time is inserted before it. Returns the data to write.
   */
  process(data: string): string {
    if (this.startedAt === null) {
      return data
    }
    let out = ''
    for (const ch of data) {
      if (ch === '\r' || ch === '\n') {
        if (this.inCandidate) {
          out += this.buf
          this.buf = ''
          this.inCandidate = false
        }
        out += ch
        this.atLineStart = true
        continue
      }
      if (this.inCandidate) {
        this.buf += ch
        const visible = stripAnsi(this.buf)
        if (isPrompt(visible)) {
          const lead = leadingErasePrefix(this.buf)
          const elapsed = this.now() - this.startedAt
          out += lead + TIMER_DIM + formatElapsed(elapsed) + TIMER_RESET + TIMER_SEP
          out += this.buf.slice(lead.length)
          this.reset()
        } else if (visible.length > PROMPT_MAX_CHARS) {
          out += this.buf
          this.buf = ''
          this.inCandidate = false
        }
        continue
      }
      if (this.atLineStart) {
        this.buf = ch
        this.inCandidate = true
        this.atLineStart = false
        continue
      }
      out += ch
    }
    return out
  }
}
