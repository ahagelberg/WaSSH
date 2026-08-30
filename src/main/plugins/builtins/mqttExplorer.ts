import { MqttClient, type IPublishPacket } from 'mqtt'
import type { Duplex } from 'stream'
import {
  MQTT_EXPLORER_DEFAULT_HOST,
  MQTT_EXPLORER_DEFAULT_PORT,
  type MqttExplorerErrorKind,
  type MqttExplorerMessagePayload,
  type MqttExplorerRendererMessage,
  type MqttExplorerStatusPayload
} from '../../../shared/plugins'
import type { PluginMainContext, PluginMainModule } from '../PluginHost'

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
  MqttExplorerMessagePayload,
  'payloadText' | 'payloadBase64' | 'binary'
> {
  if (isUtf8Text(buf)) {
    return { binary: false, payloadText: buf.toString('utf8') }
  }
  return { binary: true, payloadBase64: buf.toString('base64') }
}

function classifyConnectError(err: unknown): {
  errorKind: MqttExplorerErrorKind
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

function sendStatus(ctx: PluginMainContext, payload: Omit<MqttExplorerStatusPayload, 'type'>): void {
  ctx.sendToRenderer({ type: 'status', ...payload } satisfies MqttExplorerStatusPayload)
}

function readBrokerSettings(ctx: PluginMainContext): {
  host: string
  port: number
  username: string
  password: string
} {
  const s = ctx.getSettings()
  return {
    host: asString(s.host, MQTT_EXPLORER_DEFAULT_HOST).trim() || MQTT_EXPLORER_DEFAULT_HOST,
    port: asPort(s.port, MQTT_EXPLORER_DEFAULT_PORT),
    username: asString(s.username, '').trim(),
    password: asString(s.password, '')
  }
}

interface SessionState {
  client: MqttClient | null
  stream: Duplex | null
  stopped: boolean
}

async function connectBroker(ctx: PluginMainContext, state: SessionState): Promise<void> {
  if (state.stopped) {
    return
  }
  if (!ctx.isSshSession()) {
    sendStatus(ctx, {
      state: 'unavailable',
      reason: 'MQTT Explorer requires an SSH session',
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
    const encoded = encodePayload(payload)
    const qos = (packet.qos === 1 || packet.qos === 2 ? packet.qos : 0) as 0 | 1 | 2
    ctx.sendToRenderer({
      type: 'message',
      topic,
      ...encoded,
      qos,
      retain: Boolean(packet.retain),
      timestamp: Date.now()
    } satisfies MqttExplorerMessagePayload)
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

function isRendererMessage(payload: unknown): payload is MqttExplorerRendererMessage {
  if (!payload || typeof payload !== 'object') {
    return false
  }
  const type = (payload as { type?: unknown }).type
  return type === 'publish' || type === 'reconnect'
}

export const mqttExplorerMain: PluginMainModule = {
  async onActivate(ctx) {
    const state: SessionState = {
      client: null,
      stream: null,
      stopped: false
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
    if (!isRendererMessage(payload)) {
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

    if (payload.type === 'publish') {
      const client = state.client
      if (!client?.connected) {
        return
      }
      const topic = payload.topic.trim()
      if (!topic) {
        return
      }
      const buf = Buffer.from(payload.payloadBase64, 'base64')
      const qos = payload.qos === 1 || payload.qos === 2 ? payload.qos : 0
      client.publish(topic, buf, { qos, retain: Boolean(payload.retain) })
    }
  }
}

function instanceKey(ctx: PluginMainContext): string {
  return `${ctx.tabId}::${ctx.pluginId}`
}

const sessionStates = new Map<string, SessionState>()
