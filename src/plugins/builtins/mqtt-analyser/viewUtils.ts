import type { MqttAnalyserSavedMessage, MqttAnalyserSavedMessageMode } from './protocol'

/** Prefix for JSON payload validation errors */
export const JSON_ERROR_PREFIX = 'Invalid JSON'

/** Validate a JSON payload; returns the error message, or null when valid */
export function jsonParseError(text: string): string | null {
  try {
    JSON.parse(text)
    return null
  } catch (err) {
    const detail = err instanceof Error ? err.message : 'parse failed'
    return `${JSON_ERROR_PREFIX} — ${detail}`
  }
}

/** Valid saved-message payload formats */
export const SAVED_MESSAGE_MODES: MqttAnalyserSavedMessageMode[] = ['text', 'json']

/** Validate stored saved-message list (defensive: data lives in app settings) */
export function normalizeSavedMessages(raw: unknown): MqttAnalyserSavedMessage[] {
  if (!Array.isArray(raw)) {
    return []
  }
  const seen = new Set<string>()
  const out: MqttAnalyserSavedMessage[] = []
  for (const entry of raw) {
    if (!entry || typeof entry !== 'object') {
      continue
    }
    const id = (entry as { id?: unknown }).id
    if (typeof id !== 'string' || !id || seen.has(id)) {
      continue
    }
    const mode = (entry as { mode?: unknown }).mode
    seen.add(id)
    out.push({
      id,
      label: typeof (entry as { label?: unknown }).label === 'string'
        ? ((entry as { label: string }).label)
        : '',
      topic: typeof (entry as { topic?: unknown }).topic === 'string'
        ? ((entry as { topic: string }).topic)
        : '',
      payload: typeof (entry as { payload?: unknown }).payload === 'string'
        ? ((entry as { payload: string }).payload)
        : '',
      mode: SAVED_MESSAGE_MODES.includes(mode as MqttAnalyserSavedMessageMode)
        ? (mode as MqttAnalyserSavedMessageMode)
        : 'text'
    })
  }
  return out
}
