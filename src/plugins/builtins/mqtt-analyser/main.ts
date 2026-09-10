import { MqttClient, type IPublishPacket } from 'mqtt'
import type { Duplex } from 'stream'
import {
  MQTT_ANALYSER_DEFAULT_HOST,
  MQTT_ANALYSER_DEFAULT_PORT,
  MQTT_ANALYSER_HISTORY_LIMIT
} from './defaults'
import {
  type MqttAnalyserErrorKind,
  type MqttAnalyserMessagePayload,
  type MqttAnalyserSnapshotPayload,
  type MqttAnalyserStatusPayload
} from './protocol'
import { isMqttAnalyserRendererMessage } from './protocol'
import type { PluginMainContext, PluginMainModule } from '@plugin-api/main'

/** Subscribe to all topics */
const SUBSCRIBE_TOPIC = '#'

/** MQTT connect timeout (ms) */
const CONNECT_TIMEOUT_MS = 12_000

/** Disable mqtt.js auto-reconnect; we reconnect explicitly */
const RECONNECT_PERIOD_MS = 0

/** Subscribe QoS */
const SUBSCRIBE_QOS = 0 as const

function asString(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback
}

function asPort(value: unknown, fallback: number): number {
  const n = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(n) || n < 1 || n > 65535) {
    return fallback
  }
  return Math.floor(n)
}

function isUtf8Text(buf: Buffer): boolean {
  if (buf.length === 0) {
    return true
  }
  try {
    const text = buf.toString('utf8')
    const roundTrip = Buffer.from(text, 'utf8')
    if (!roundTrip.equals(buf)) {
      return false
    }
    // Reject buffers with many replacement chars from invalid sequences
    if (text.includes('\uFFFD') && buf.includes(0xff)) {
      return false
    }
    return true
  } catch {
    return false
  }
}

function encodePayload(buf: Buffer): Pick<
  MqttAnalyserMessagePayload,
  'payloadText' | 'payloadBase64' | 'binary'
> {
  if (isUtf8Text(buf)) {
    return { binary: false, payloadText: buf.toString('utf8') }
  }
  return { binary: true, payloadBase64: buf.toString('base64') }
}

function classifyConnectError(err: unknown): {
  errorKind: MqttAnalyserErrorKind
  reason: string
} {
  const message = err instanceof Error ? err.message : String(err)
  const lower = message.toLowerCase()
  const code =
    err && typeof err === 'object' && 'code' in err
      ? Number((err as { code: unknown }).code)
      : NaN

  // MQTT 3.1.1 CONNACK: 4 bad user/pass, 5 not authorized
  if (code === 4 || code === 5 || lower.includes('not authorized') || lower.includes('bad username')) {
    return { errorKind: 'auth_failed', reason: 'Authentication failed' }
  }
  if (
    lower.includes('econnrefused') ||
    lower.includes('connection refused') ||
    lower.includes('connect econnrefused')
  ) {
    return { errorKind: 'no_broker', reason: 'No MQTT broker on the configured host/port' }
  }
  if (
    lower.includes('etimedout') ||
    lower.includes('timed out') ||
    lower.includes('timeout') ||
    lower.includes('enotfound') ||
    lower.includes('ehostunreach') ||
    lower.includes('enetunreach')
  ) {
    return { errorKind: 'unreachable', reason: message || 'Broker unreachable' }
  }
  return { errorKind: 'other', reason: message || 'Connection failed' }
}

function sendStatus(ctx: PluginMainContext, payload: Omit<MqttAnalyserStatusPayload, 'type'>): void {
  ctx.sendToRenderer({ type: 'status', ...payload } satisfies MqttAnalyserStatusPayload)
}

function readBrokerSettings(ctx: PluginMainContext): {
  host: string
  port: number
  username: string
  password: string
} {
  const s = ctx.getSettings()
  return {
    host: asString(s.host, MQTT_ANALYSER_DEFAULT_HOST).trim() || MQTT_ANALYSER_DEFAULT_HOST,
    port: asPort(s.port, MQTT_ANALYSER_DEFAULT_PORT),
    username: asString(s.username, '').trim(),
    password: asString(s.password, '')
  }
}

/** One recent message recorded for a topic, mirroring what the renderer already tracks. */
interface MqttTopicMessage {
  payloadText?: string
  payloadBase64?: string
  binary: boolean
  qos: 0 | 1 | 2
  retain: boolean
  timestamp: number
}

/** Main-side mirror of a topic's last value + bounded history, for API calls. */
interface MqttTopicRecord {
  binary: boolean
  payloadText?: string
  payloadBase64?: string
  timestamp: number
  messageCount: number
  history: MqttTopicMessage[]
}

interface SessionState {
  client: MqttClient | null
  stream: Duplex | null
  stopped: boolean
  /** Bounded per-topic mirror of received messages, for `get_topics`/`get_topic_value`. */
  topics: Map<string, MqttTopicRecord>
}

function recordTopicMessage(state: SessionState, topic: string, message: MqttTopicMessage): void {
  let record = state.topics.get(topic)
  if (!record) {
    record = { binary: message.binary, timestamp: message.timestamp, messageCount: 0, history: [] }
    state.topics.set(topic, record)
  }
  record.binary = message.binary
  record.payloadText = message.payloadText
  record.payloadBase64 = message.payloadBase64
  record.timestamp = message.timestamp
  record.messageCount += 1
  record.history.push(message)
  if (record.history.length > MQTT_ANALYSER_HISTORY_LIMIT) {
    record.history.splice(0, record.history.length - MQTT_ANALYSER_HISTORY_LIMIT)
  }
}

function sendSnapshot(ctx: PluginMainContext, state: SessionState): void {
  ctx.sendToRenderer({
    type: 'snapshot',
    topics: Array.from(state.topics.entries()).map(([topic, record]) => ({
      topic,
      messageCount: record.messageCount,
      history: record.history.map((message) => ({ ...message }))
    }))
  } satisfies MqttAnalyserSnapshotPayload)
}

async function connectBroker(ctx: PluginMainContext, state: SessionState): Promise<void> {
  if (state.stopped) {
    return
  }
  if (!ctx.isSshSession()) {
    sendStatus(ctx, {
      state: 'unavailable',
      reason: 'MQTT Analyser requires an SSH session',
      errorKind: 'not_ssh'
    })
    return
  }

  teardownClient(state)

  const { host, port, username, password } = readBrokerSettings(ctx)
  sendStatus(ctx, { state: 'connecting' })

  let stream: Duplex
  try {
    stream = await ctx.openTcpStream(host, port)
  } catch (err) {
    if (state.stopped) {
      return
    }
    const classified = classifyConnectError(err)
    sendStatus(ctx, {
      state: 'error',
      reason: classified.reason,
      errorKind: classified.errorKind
    })
    return
  }

  if (state.stopped) {
    try {
      stream.destroy()
    } catch {
      /* ignore */
    }
    return
  }

  state.stream = stream

  const options = {
    protocol: 'mqtt' as const,
    protocolVersion: 4 as const,
    reconnectPeriod: RECONNECT_PERIOD_MS,
    connectTimeout: CONNECT_TIMEOUT_MS,
    clean: true,
    ...(username ? { username } : {}),
    ...(password ? { password } : {})
  }

  const client = new MqttClient(() => stream, options)
  state.client = client

  client.on('connect', () => {
    if (state.stopped) {
      return
    }
    client.subscribe(SUBSCRIBE_TOPIC, { qos: SUBSCRIBE_QOS }, (err) => {
      if (state.stopped) {
        return
      }
      if (err) {
        sendStatus(ctx, {
          state: 'error',
          reason: err.message || 'Subscribe failed',
          errorKind: 'other'
        })
        return
      }
      sendStatus(ctx, { state: 'connected' })
    })
  })

  client.on('message', (topic: string, payload: Buffer, packet: IPublishPacket) => {
    if (state.stopped) {
      return
    }
    const topicName =
      typeof topic === 'string' && topic.length > 0
        ? topic
        : typeof packet.topic === 'string' && packet.topic.length > 0
          ? packet.topic
          : ''
    if (!topicName) {
      return
    }
    const encoded = encodePayload(payload)
    const qos = (packet.qos === 1 || packet.qos === 2 ? packet.qos : 0) as 0 | 1 | 2
    const timestamp = Date.now()
    const retain = Boolean(packet.retain)
    // A zero-length payload signals the topic was cleared (typical retained-delete
    // pattern); mirror the renderer's own tree, which drops it rather than recording it.
    if (payload.length === 0) {
      state.topics.delete(topicName)
    } else {
      recordTopicMessage(state, topicName, { ...encoded, qos, retain, timestamp })
    }
    ctx.sendToRenderer({
      type: 'message',
      topic: topicName,
      ...encoded,
      qos,
      retain,
      timestamp
    } satisfies MqttAnalyserMessagePayload)
  })

  client.on('error', (err) => {
    if (state.stopped) {
      return
    }
    const classified = classifyConnectError(err)
    sendStatus(ctx, {
      state: 'error',
      reason: classified.reason,
      errorKind: classified.errorKind
    })
  })

  client.on('close', () => {
    if (state.stopped) {
      return
    }
    if (client.disconnecting) {
      return
    }
    sendStatus(ctx, { state: 'disconnected', reason: 'Connection closed' })
  })
}

function teardownClient(state: SessionState): void {
  const client = state.client
  state.client = null
  const stream = state.stream
  state.stream = null
  if (client) {
    try {
      client.removeAllListeners()
      client.end(true)
    } catch {
      /* ignore */
    }
  }
  if (stream) {
    try {
      stream.destroy()
    } catch {
      /* ignore */
    }
  }
}

export const mqttAnalyserMain: PluginMainModule = {
  async onActivate(ctx) {
    const state: SessionState = {
      client: null,
      stream: null,
      stopped: false,
      topics: new Map()
    }

    sessionStates.set(instanceKey(ctx), state)

    ctx.onDeactivateCleanup(() => {
      state.stopped = true
      teardownClient(state)
      sessionStates.delete(instanceKey(ctx))
    })

    await connectBroker(ctx, state)
  },

  async onDeactivate(ctx) {
    const key = instanceKey(ctx)
    const state = sessionStates.get(key)
    if (state) {
      state.stopped = true
      teardownClient(state)
      sessionStates.delete(key)
    }
  },

  onMessage(ctx, payload) {
    if (!isMqttAnalyserRendererMessage(payload)) {
      return
    }
    const state = sessionStates.get(instanceKey(ctx))
    if (!state || state.stopped) {
      return
    }

    if (payload.type === 'reconnect') {
      void connectBroker(ctx, state)
      return
    }

    if (payload.type === 'sync') {
      sendSnapshot(ctx, state)
      return
    }

    if (payload.type === 'publish') {
      const client = state.client
      if (!client?.connected) {
        return 'Not connected'
      }
      const topic = payload.topic.trim()
      if (!topic) {
        return 'Topic is required'
      }
      if (typeof payload.payloadBase64 !== 'string') {
        return 'Invalid payload'
      }
      const qos = payload.qos === 1 || payload.qos === 2 ? payload.qos : 0
      return publishMqtt(client, topic, payload.payloadBase64, qos, Boolean(payload.retain))
    }
  },

  async onApiCall(ctx, method, params) {
    const state = sessionStates.get(instanceKey(ctx))
    if (!state) {
      throw new Error('MQTT analyser is not active on this tab')
    }

    if (method === 'get_topics') {
      return Array.from(state.topics.entries()).map(([topic, record]) => ({
        topic,
        value: record.binary ? '(binary)' : (record.payloadText ?? ''),
        messageCount: record.messageCount,
        timestamp: record.timestamp
      }))
    }

    const args = params && typeof params === 'object' ? (params as Record<string, unknown>) : {}

    if (method === 'get_topic_value') {
      const topic = String(args.topic || '')
      const record = state.topics.get(topic)
      if (!record) {
        return { topic, found: false }
      }
      const result: Record<string, unknown> = {
        topic,
        found: true,
        value: record.binary ? '(binary)' : (record.payloadText ?? ''),
        timestamp: record.timestamp,
        messageCount: record.messageCount
      }
      if (args.includeHistory) {
        result.history = record.history.map((m) => ({
          value: m.binary ? '(binary)' : (m.payloadText ?? ''),
          qos: m.qos,
          retain: m.retain,
          timestamp: m.timestamp
        }))
      }
      return result
    }

    if (method === 'publish') {
      const client = state.client
      if (!client?.connected) {
        throw new Error('Not connected to MQTT broker')
      }
      const topic = String(args.topic || '').trim()
      if (!topic) {
        throw new Error('Topic is required')
      }
      const payloadText = String(args.payload ?? '')
      const qos = args.qos === 1 || args.qos === 2 ? args.qos : 0
      const retain = Boolean(args.retain)
      const payloadBase64 = Buffer.from(payloadText, 'utf8').toString('base64')
      const error = await publishMqtt(client, topic, payloadBase64, qos as 0 | 1 | 2, retain)
      if (error) {
        throw new Error(error)
      }
      return { ok: true }
    }

    throw new Error(`Unknown mqtt-analyser API method: ${method}`)
  }
}

function publishMqtt(
  client: MqttClient,
  topic: string,
  payloadBase64: string,
  qos: 0 | 1 | 2,
  retain: boolean
): Promise<string | undefined> {
  let buf: Buffer
  try {
    buf = Buffer.from(payloadBase64, 'base64')
  } catch (err) {
    return Promise.resolve(err instanceof Error ? err.message : 'Invalid payload')
  }
  return new Promise((resolve) => {
    try {
      client.publish(topic, buf, { qos, retain }, (err) => {
        resolve(err ? err.message || 'Publish failed' : undefined)
      })
    } catch (err) {
      resolve(err instanceof Error ? err.message : 'Publish failed')
    }
  })
}

function instanceKey(ctx: PluginMainContext): string {
  return `${ctx.tabId}::${ctx.pluginId}`
}

const sessionStates = new Map<string, SessionState>()
