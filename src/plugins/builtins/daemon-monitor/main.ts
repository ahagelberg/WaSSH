import type { PluginMainContext, PluginMainModule } from '@plugin-api/main'
import {
  DAEMON_MONITOR_DEFAULT_RANGE,
  DAEMON_MONITOR_RANGES,
  isDaemonMonitorRange,
  type DaemonMonitorRange
} from './defaults'
import {
  type DaemonMonitorActionResult,
  type DaemonMonitorMainMessage,
  type DaemonMonitorSample,
  type DaemonMonitorServiceStatus,
  type DaemonMonitorStateEvent,
  isDaemonMonitorRendererMessage
} from './protocol'
import {
  buildInstallCommand,
  buildProbeCommand,
  buildStreamCommand,
  buildUninstallCommand,
  REMOTE_OPERATION_SUCCESS,
  REMOTE_SERVICE_VERSION
} from './remoteService'

const BYTES_PER_KIB = 1024
const EPOCH_MS = 1000
const RECORD_FIELD_COUNT_SAMPLE = 8
const RECORD_FIELD_COUNT_STATE = 6

interface DaemonMonitorSession {
  status: DaemonMonitorServiceStatus
  range: DaemonMonitorRange
  streamId: string | null
  streamBuffer: string
  closingStreamIds: Set<string>
  probeGeneration: number
  streamGeneration: number
  disposed: boolean
  seenLines: Map<string, number>
  samples: DaemonMonitorSample[]
  events: DaemonMonitorStateEvent[]
}

const sessions = new Map<string, DaemonMonitorSession>()

function instanceKey(ctx: PluginMainContext): string {
  return `${ctx.tabId}:${ctx.pluginId}`
}

function sendStatus(ctx: PluginMainContext, session: DaemonMonitorSession): void {
  const message: DaemonMonitorMainMessage = { type: 'status', status: session.status }
  ctx.sendToRenderer(message)
}

function setStatus(
  ctx: PluginMainContext,
  session: DaemonMonitorSession,
  status: DaemonMonitorServiceStatus
): void {
  session.status = status
  sendStatus(ctx, session)
}

function pingTargets(ctx: PluginMainContext): string[] {
  const value = ctx.getSettings().pingTargets
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : []
}

function parseInteger(value: string): number | null {
  if (!/^\d+$/.test(value)) {
    return null
  }
  const parsed = Number(value)
  return Number.isSafeInteger(parsed) ? parsed : null
}

function parseRecord(
  line: string,
  cutoffMs: number
): { sample?: DaemonMonitorSample; event?: DaemonMonitorStateEvent } | null {
  const fields = line.split('\t')
  const version = parseInteger(fields[0] || '')
  const epoch = parseInteger(fields[1] || '')
  if (version !== REMOTE_SERVICE_VERSION || epoch === null) {
    return null
  }
  const timestamp = epoch * EPOCH_MS
  if (fields[2] === 'sample' && fields.length === RECORD_FIELD_COUNT_SAMPLE) {
    if (timestamp < cutoffMs) {
      return null
    }
    const values = fields.slice(3).map(parseInteger)
    if (values.some((value) => value === null)) {
      return null
    }
    const [cpuTenths, memoryUsedKib, memoryTotalKib, diskUsedKib, diskTotalKib] =
      values as number[]
    return {
      sample: {
        timestamp,
        cpuPercent: cpuTenths / 10,
        memoryUsedBytes: memoryUsedKib * BYTES_PER_KIB,
        memoryTotalBytes: memoryTotalKib * BYTES_PER_KIB,
        diskUsedBytes: diskUsedKib * BYTES_PER_KIB,
        diskTotalBytes: diskTotalKib * BYTES_PER_KIB
      }
    }
  }
  if (
    (fields[2] === 'event' || fields[2] === 'current') &&
    fields.length === RECORD_FIELD_COUNT_STATE &&
    (fields[3] === 'interface' || fields[3] === 'ping') &&
    (fields[5] === 'up' || fields[5] === 'down')
  ) {
    if (fields[2] === 'event' && timestamp < cutoffMs) {
      return null
    }
    return {
      event: {
        timestamp,
        kind: fields[3],
        name: fields[4],
        state: fields[5],
        transition: fields[2] === 'event'
      }
    }
  }
  return null
}

function closeStream(ctx: PluginMainContext, session: DaemonMonitorSession): void {
  if (!session.streamId) {
    return
  }
  session.closingStreamIds.add(session.streamId)
  ctx.closeSideConnection(session.streamId)
  session.streamId = null
}

function consumeStreamData(
  ctx: PluginMainContext,
  session: DaemonMonitorSession,
  chunk: string
): void {
  session.streamBuffer += chunk
  const lines = session.streamBuffer.split(/\r?\n/)
  session.streamBuffer = lines.pop() || ''
  const cutoffMs = Date.now() - DAEMON_MONITOR_RANGES[session.range] * EPOCH_MS
  session.samples = session.samples.filter((sample) => sample.timestamp >= cutoffMs)
  session.events = session.events.filter(
    (event) => !event.transition || event.timestamp >= cutoffMs
  )
  for (const [line, timestamp] of session.seenLines) {
    if (timestamp < cutoffMs) {
      session.seenLines.delete(line)
    }
  }
  const samples: DaemonMonitorSample[] = []
  const events: DaemonMonitorStateEvent[] = []
  for (const line of lines) {
    if (!line || session.seenLines.has(line)) {
      continue
    }
    const record = parseRecord(line, cutoffMs)
    if (!record) {
      continue
    }
    session.seenLines.set(
      line,
      record.sample?.timestamp ?? record.event?.timestamp ?? Date.now()
    )
    if (record.sample) {
      session.samples.push(record.sample)
      samples.push(record.sample)
    }
    if (record.event) {
      session.events.push(record.event)
      events.push(record.event)
    }
  }
  if (samples.length === 0 && events.length === 0) {
    return
  }
  const message: DaemonMonitorMainMessage = { type: 'records', reset: false, samples, events }
  ctx.sendToRenderer(message)
}

async function startStream(ctx: PluginMainContext, session: DaemonMonitorSession): Promise<void> {
  const generation = ++session.streamGeneration
  closeStream(ctx, session)
  session.seenLines.clear()
  session.samples = []
  session.events = []
  session.streamBuffer = ''
  const reset: DaemonMonitorMainMessage = { type: 'records', reset: true, samples: [], events: [] }
  ctx.sendToRenderer(reset)
  const since = Math.floor(Date.now() / EPOCH_MS) - DAEMON_MONITOR_RANGES[session.range]
  const streamId = await ctx.openSideConnection({
    kind: 'ssh-exec',
    command: buildStreamCommand(since)
  })
  if (session.disposed || generation !== session.streamGeneration) {
    ctx.closeSideConnection(streamId)
    return
  }
  session.streamId = streamId
  const offData = ctx.onSideData(streamId, (data) => consumeStreamData(ctx, session, data))
  const offClosed = ctx.onSideClosed(streamId, (error) => {
    offData()
    offClosed()
    if (session.streamId === streamId) {
      session.streamId = null
    }
    if (session.closingStreamIds.delete(streamId)) {
      return
    }
    setStatus(ctx, session, {
      ...session.status,
      state: 'error',
      streamConnected: false,
      message: error || 'WaSSH Service stream closed'
    })
  })
  setStatus(ctx, session, { ...session.status, streamConnected: true, message: undefined })
}

async function probe(ctx: PluginMainContext, session: DaemonMonitorSession): Promise<void> {
  const generation = ++session.probeGeneration
  setStatus(ctx, session, { state: 'probing', streamConnected: false })
  if (!ctx.isSshSession()) {
    setStatus(ctx, session, {
      state: 'error',
      streamConnected: false,
      message: 'WaSSH Service requires an SSH session'
    })
    return
  }
  let result: string
  try {
    result = await ctx.execCapture(buildProbeCommand())
  } catch (error) {
    if (generation !== session.probeGeneration) {
      return
    }
    setStatus(ctx, session, {
      state: 'error',
      streamConnected: false,
      message: error instanceof Error ? error.message : String(error)
    })
    return
  }
  if (generation !== session.probeGeneration) {
    return
  }
  if (result === 'unsupported') {
    setStatus(ctx, session, {
      state: 'error',
      streamConnected: false,
      message: 'WaSSH Service requires Linux with systemd'
    })
    return
  }
  if (result === 'missing') {
    closeStream(ctx, session)
    setStatus(ctx, session, { state: 'missing', streamConnected: false })
    return
  }
  const [present, version, active] = result.split('\t')
  if (present !== 'present' || active !== 'active') {
    setStatus(ctx, session, {
      state: 'error',
      streamConnected: false,
      version,
      message: 'WaSSH Service is installed but not running'
    })
    return
  }
  const installedVersion = Number(version)
  if (installedVersion < REMOTE_SERVICE_VERSION) {
    closeStream(ctx, session)
    setStatus(ctx, session, {
      state: 'outdated',
      streamConnected: false,
      version,
      message: 'A newer version of WaSSH Service is available'
    })
    return
  }
  if (installedVersion > REMOTE_SERVICE_VERSION) {
    setStatus(ctx, session, {
      state: 'error',
      streamConnected: false,
      version,
      message: 'WaSSH Service version is newer than this app supports'
    })
    return
  }
  setStatus(ctx, session, { state: 'ready', version, streamConnected: false })
  try {
    await startStream(ctx, session)
  } catch (error) {
    setStatus(ctx, session, {
      state: 'error',
      version,
      streamConnected: false,
      message: error instanceof Error ? error.message : String(error)
    })
  }
}

async function runSudoOperation(
  ctx: PluginMainContext,
  session: DaemonMonitorSession,
  action: 'install' | 'uninstall',
  password: string
): Promise<DaemonMonitorActionResult> {
  session.probeGeneration += 1
  closeStream(ctx, session)
  setStatus(ctx, session, {
    state: action === 'install' ? 'installing' : 'uninstalling',
    streamConnected: false,
    message: `${action === 'install' ? 'Installing' : 'Uninstalling'} WaSSH Service…`
  })
  let command: string
  try {
    command =
      action === 'install' ? buildInstallCommand(pingTargets(ctx)) : buildUninstallCommand()
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    setStatus(ctx, session, { state: 'error', streamConnected: false, message })
    return { ok: false, error: message }
  }
  try {
    const connectionId = await ctx.openSideConnection({ kind: 'ssh-exec', command })
    const output = await new Promise<string>((resolve) => {
      let captured = ''
      const offData = ctx.onSideData(connectionId, (data) => {
        captured += data
      })
      const offClosed = ctx.onSideClosed(connectionId, () => {
        offData()
        offClosed()
        resolve(captured)
      })
      ctx.writeSideConnection(connectionId, `${password}\n`)
    })
    if (!output.split(/\r?\n/).includes(REMOTE_OPERATION_SUCCESS)) {
      const message = output.trim() || `Failed to ${action} WaSSH Service`
      setStatus(ctx, session, { state: 'error', streamConnected: false, message })
      return { ok: false, error: message }
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    setStatus(ctx, session, { state: 'error', streamConnected: false, message })
    return { ok: false, error: message }
  }
  await probe(ctx, session)
  return { ok: true }
}

export const daemonMonitorMain: PluginMainModule = {
  onActivate(ctx) {
    const session: DaemonMonitorSession = {
      status: { state: 'probing', streamConnected: false },
      range: DAEMON_MONITOR_DEFAULT_RANGE,
      streamId: null,
      streamBuffer: '',
      closingStreamIds: new Set(),
      probeGeneration: 0,
      streamGeneration: 0,
      disposed: false,
      seenLines: new Map(),
      samples: [],
      events: []
    }
    sessions.set(instanceKey(ctx), session)
    ctx.onDeactivateCleanup(() => {
      session.disposed = true
      session.streamGeneration += 1
      closeStream(ctx, session)
      sessions.delete(instanceKey(ctx))
    })
    void probe(ctx, session)
  },

  async onMessage(ctx, payload) {
    if (!isDaemonMonitorRendererMessage(payload)) {
      return { ok: false, error: 'Invalid daemon monitor message' }
    }
    const session = sessions.get(instanceKey(ctx))
    if (!session) {
      return { ok: false, error: 'Daemon monitor is not active' }
    }
    if (payload.type === 'probe') {
      await probe(ctx, session)
      return { ok: true }
    }
    if (payload.type === 'install' || payload.type === 'uninstall') {
      return runSudoOperation(ctx, session, payload.type, payload.password)
    }
    if (!isDaemonMonitorRange(payload.range)) {
      return { ok: false, error: 'Invalid history range' }
    }
    session.range = payload.range
    if (session.status.state === 'ready') {
      await startStream(ctx, session)
    }
    return { ok: true }
  },

  onApiCall(ctx, method) {
    if (method !== 'get_status') {
      throw new Error(`Unknown daemon-monitor API method: ${method}`)
    }
    const session = sessions.get(instanceKey(ctx))
    return {
      status: session?.status ?? null,
      sample: session?.samples.at(-1) ?? null
    }
  }
}
