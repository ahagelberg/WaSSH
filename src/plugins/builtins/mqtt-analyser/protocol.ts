export type MqttAnalyserStatusState =
  | 'idle'
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'unavailable'
  | 'error'

export type MqttAnalyserErrorKind =
  | 'not_ssh'
  | 'no_broker'
  | 'auth_failed'
  | 'unreachable'
  | 'other'

/** Main → renderer status event */
export interface MqttAnalyserStatusPayload {
  type: 'status'
  state: MqttAnalyserStatusState
  reason?: string
  errorKind?: MqttAnalyserErrorKind
}

/** Main → renderer message event */
export interface MqttAnalyserMessagePayload {
  type: 'message'
  topic: string
  /** UTF-8 text when binary is false */
  payloadText?: string
  /** Base64 when binary is true */
  payloadBase64?: string
  binary: boolean
  qos: 0 | 1 | 2
  retain: boolean
  timestamp: number
}

export type MqttAnalyserMainMessage = MqttAnalyserStatusPayload | MqttAnalyserMessagePayload

/** Renderer → main */
export type MqttAnalyserRendererMessage =
  | {
      type: 'publish'
      topic: string
      payloadBase64: string
      qos: 0 | 1 | 2
      retain: boolean
    }
  | { type: 'reconnect' }

export function isMqttAnalyserMainMessage(payload: unknown): payload is MqttAnalyserMainMessage {
  if (!payload || typeof payload !== 'object') {
    return false
  }
  const type = (payload as { type?: unknown }).type
  return type === 'status' || type === 'message'
}

export function isMqttAnalyserRendererMessage(
  payload: unknown
): payload is MqttAnalyserRendererMessage {
  if (!payload || typeof payload !== 'object') {
    return false
  }
  const type = (payload as { type?: unknown }).type
  return type === 'publish' || type === 'reconnect'
}
