/**
 * Tracks the step an `open()` attempt is currently awaiting.
 *
 * Transport teardown removes the listeners/callbacks those steps wait on
 * (destroyed socket, closed serial port, ended SSH client), so without an
 * explicit settle the attempt would hang: `opening` would stay true, every
 * reconnect (focus, resume, backoff) would be refused, and callers awaiting
 * `connect()` would never return.
 */
export class OpenAttempt {
  private settles = new Set<() => void>()
  private cancelled = false
  private token: object = {}

  /** True once a transport teardown ended the current attempt. */
  get isCancelled(): boolean {
    return this.cancelled
  }

  /** Begins a new attempt; call after tearing down any stale transport. */
  reset(): object {
    this.settles.clear()
    this.cancelled = false
    this.token = {}
    return this.token
  }

  /** Whether `token` still identifies the live attempt. */
  isCurrent(token: object): boolean {
    return this.token === token
  }

  /** Registers the settle function of an awaited step; returns a release fn. */
  track(settle: () => void): () => void {
    this.settles.add(settle)
    return () => {
      this.settles.delete(settle)
    }
  }

  /** Ends the current attempt: settle every pending step and mark it cancelled. */
  cancel(): void {
    this.cancelled = true
    const settles = [...this.settles]
    this.settles.clear()
    for (const settle of settles) {
      settle()
    }
  }
}

/** Rejection used to settle a cancelled step; never surfaced (callers check `isCancelled`). */
export const OPEN_ATTEMPT_CANCELLED = 'Connection attempt cancelled'
