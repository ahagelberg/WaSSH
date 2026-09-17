import type { Duplex } from 'node:stream'
import type { PluginMainContext } from '@plugin-api/main'
import {
  FRAME_KIND_HEADER,
  MonitorFrameDecoder,
  type MonitorSample,
  type MonitorStreamHeader
} from '@shared/monitorFrames'
import { MonitorLadder } from '@shared/monitorLadder'
import type { MonitorSeries } from '@shared/monitorSeries'
import {
  MONITOR_SERVICE_INITIAL_STATUS,
  type MonitorActionResult,
  type MonitorMainMessage,
  type MonitorServiceStatus
} from './serviceProtocol'
import {
  buildInstallCommand,
  buildProbeCommand,
  buildUninstallCommand,
  REMOTE_OPERATION_SUCCESS,
  REMOTE_SERVICE_VERSION,
  REMOTE_SOCKET_PATH
} from './remoteService'

/** Delay before re-probing when the daemon stream drops unexpectedly */
const MONITOR_RECONNECT_DELAY_MS = 5000

export interface MonitorServiceSession {
  status: MonitorServiceStatus
  /** Series set announced by the daemon stream, null until the header arrives */
  header: MonitorStreamHeader | null
  /** Ladder fed by the daemon stream while it is connected */
  ladder: MonitorLadder | null
  /** Live socket to the daemon, null while disconnected */
  stream: Duplex | null
  /** Set while closeStream tears the socket down, so 'close' does not reconnect */
  deliberateClose: boolean
  /** Newest sample folded into the ladder; replayed samples at or below it are skipped */
  lastSampleMs: number
  /** True while retrying after an unexpected stream drop */
  reconnecting: boolean
  reconnectTimer: ReturnType<typeof setTimeout> | null
  decoder: MonitorFrameDecoder
  probeGeneration: number
  streamGeneration: number
  disposed: boolean
}

export interface MonitorServiceHooks {
  /** Called with every decoded sample, plus the ladder it was folded into. */
  onSample: (sample: MonitorSample, ladder: MonitorLadder, header: MonitorStreamHeader) => void
  /** Called when the daemon's series set changes (first connect, upgrade). */
  onHeader: (header: MonitorStreamHeader) => void
  /** Called when the daemon history is dropped and the app-side ladder takes over. */
  onHistoryCleared: () => void
  /** Called when the daemon stream disconnects (including deliberate closes). */
  onStreamClosed: (error?: string) => void
}

export function createServiceSession(): MonitorServiceSession {
  return {
    status: { ...MONITOR_SERVICE_INITIAL_STATUS },
    header: null,
    ladder: null,
    stream: null,
    deliberateClose: false,
    lastSampleMs: 0,
    reconnecting: false,
    reconnectTimer: null,
    decoder: new MonitorFrameDecoder(),
    probeGeneration: 0,
    streamGeneration: 0,
    disposed: false
  }
}

function send(ctx: PluginMainContext, message: MonitorMainMessage): void {
  ctx.sendToRenderer(message)
}

function setStatus(
  ctx: PluginMainContext,
  session: MonitorServiceSession,
  status: MonitorServiceStatus
): void {
  session.status = status
  send(ctx, { type: 'service', status })
}

function sameSeriesIds(a: MonitorSeries[], b: MonitorSeries[]): boolean {
  return a.length === b.length && a.every((series, index) => series.id === b[index].id)
}

function cancelReconnect(session: MonitorServiceSession): void {
  if (session.reconnectTimer) {
    clearTimeout(session.reconnectTimer)
    session.reconnectTimer = null
  }
  session.reconnecting = false
}

/** Drop the daemon history so the view falls back to the app-side ladder. */
function clearHistory(session: MonitorServiceSession, hooks: MonitorServiceHooks): void {
  const hadHistory = session.ladder !== null
  session.header = null
  session.ladder = null
  session.lastSampleMs = 0
  cancelReconnect(session)
  if (hadHistory) {
    hooks.onHistoryCleared()
  }
}

export function closeStream(ctx: PluginMainContext, session: MonitorServiceSession): void {
  cancelReconnect(session)
  const stream = session.stream
  if (!stream) {
    return
  }
  session.stream = null
  session.deliberateClose = true
  stream.destroy()
}

/** Fold one decoded sample into the ladder and forward it to the view. */
function consumeSample(
  session: MonitorServiceSession,
  sample: MonitorSample,
  hooks: MonitorServiceHooks
): void {
  const header = session.header
  const ladder = session.ladder
  // A reconnecting stream replays the daemon ring; already-folded samples
  // would otherwise be counted twice (the ladder does not dedupe).
  if (!header || !ladder || sample.timestamp <= session.lastSampleMs) {
    return
  }
  session.lastSampleMs = sample.timestamp
  const values: Record<string, number> = {
    cpu: sample.cpuPercent,
    'mem:used': sample.memoryUsedBytes,
    'mem:total': sample.memoryTotalBytes,
    'disk:used': sample.diskUsedBytes,
    'disk:total': sample.diskTotalBytes,
    'diskio:read': sample.diskReadRate,
    'diskio:write': sample.diskWriteRate
  }
  header.temperatureZones.forEach((zone, index) => {
    const celsius = sample.temperatures[index]
    if (celsius !== undefined) {
      values[`temp:${zone}`] = celsius
    }
  })
  header.interfaces.forEach((name, index) => {
    const iface = sample.interfaces[index]
    if (!iface) {
      return
    }
    values[`net:${name}:rx`] = iface.rxRate
    values[`net:${name}:tx`] = iface.txRate
    values[`iface:${name}:state`] = iface.up ? 1 : 0
  })
  ladder.push(sample.timestamp, values)
  hooks.onSample(sample, ladder, header)
}

async function startStream(
  ctx: PluginMainContext,
  session: MonitorServiceSession,
  hooks: MonitorServiceHooks
): Promise<void> {
  const generation = ++session.streamGeneration
  closeStream(ctx, session)
  session.decoder.reset()

  const stream = await ctx.openUnixStream(REMOTE_SOCKET_PATH)
  if (session.disposed || generation !== session.streamGeneration) {
    stream.destroy()
    return
  }
  session.stream = stream
  session.deliberateClose = false

  let streamError: string | undefined
  stream.on('error', (error: Error) => {
    streamError = error.message
  })
  stream.on('data', (chunk: Buffer) => {
    for (const frame of session.decoder.push(chunk)) {
      if (frame.kind === FRAME_KIND_HEADER) {
        const changed = !session.header || !sameSeriesIds(session.header.series, frame.header.series)
        session.header = frame.header
        if (changed) {
          // Keep history across reconnects while the series set is unchanged.
          session.ladder = new MonitorLadder(frame.header.series)
          session.lastSampleMs = 0
          hooks.onHeader(frame.header)
        }
        continue
      }
      consumeSample(session, frame.sample, hooks)
    }
  })
  stream.on('close', () => {
    const deliberate = session.deliberateClose
    session.deliberateClose = false
    if (session.stream === stream) {
      session.stream = null
    }
    if (session.disposed || generation !== session.streamGeneration) {
      return
    }
    setStatus(ctx, session, {
      ...session.status,
      streamConnected: false,
      message: streamError || 'WaSSH Service stream closed'
    })
    hooks.onStreamClosed(streamError)
    if (!deliberate) {
      scheduleReconnect(ctx, session, hooks)
    }
  })
  setStatus(ctx, session, { ...session.status, streamConnected: true, message: undefined })
}

/**
 * Retry after an unexpected drop; keeps re-probing while the daemon stays
 * unreachable (e.g. systemd restarting). Definitive probe states stop it.
 */
function scheduleReconnect(
  ctx: PluginMainContext,
  session: MonitorServiceSession,
  hooks: MonitorServiceHooks
): void {
  if (session.disposed || session.reconnectTimer || session.status.state !== 'ready') {
    return
  }
  session.reconnecting = true
  session.reconnectTimer = setTimeout(() => {
    session.reconnectTimer = null
    void probeService(ctx, session, hooks).then(() => {
      if (session.reconnecting && !session.disposed && session.status.state === 'error') {
        scheduleReconnect(ctx, session, hooks)
      }
    })
  }, MONITOR_RECONNECT_DELAY_MS)
}

export async function probeService(
  ctx: PluginMainContext,
  session: MonitorServiceSession,
  hooks: MonitorServiceHooks
): Promise<void> {
  const generation = ++session.probeGeneration
  setStatus(ctx, session, { state: 'probing', streamConnected: false })
  if (!ctx.isSshSession()) {
    clearHistory(session, hooks)
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
    clearHistory(session, hooks)
    setStatus(ctx, session, {
      state: 'error',
      streamConnected: false,
      message: 'WaSSH Service requires Linux with systemd'
    })
    return
  }
  if (result === 'nopython') {
    clearHistory(session, hooks)
    setStatus(ctx, session, {
      state: 'error',
      streamConnected: false,
      message: 'WaSSH Service requires python3 on the remote host'
    })
    return
  }
  if (result === 'missing') {
    clearHistory(session, hooks)
    setStatus(ctx, session, { state: 'missing', streamConnected: false })
    return
  }
  const [present, version, active] = result.split('\t')
  const installedVersion = Number(version)
  if (installedVersion < REMOTE_SERVICE_VERSION) {
    clearHistory(session, hooks)
    setStatus(ctx, session, {
      state: 'outdated',
      streamConnected: false,
      version,
      message: 'A newer version of WaSSH Service is available'
    })
    return
  }
  if (present !== 'present' || active !== 'active') {
    setStatus(ctx, session, {
      state: 'error',
      streamConnected: false,
      version,
      message: 'WaSSH Service is installed but not running'
    })
    return
  }
  if (installedVersion > REMOTE_SERVICE_VERSION) {
    clearHistory(session, hooks)
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
    await startStream(ctx, session, hooks)
  } catch (error) {
    scheduleReconnect(ctx, session, hooks)
    setStatus(ctx, session, {
      state: 'error',
      version,
      streamConnected: false,
      message: error instanceof Error ? error.message : String(error)
    })
  }
}

export async function runServiceAction(
  ctx: PluginMainContext,
  session: MonitorServiceSession,
  hooks: MonitorServiceHooks,
  action: 'install' | 'uninstall',
  password: string
): Promise<MonitorActionResult> {
  session.probeGeneration += 1
  closeStream(ctx, session)
  setStatus(ctx, session, {
    state: action === 'install' ? 'installing' : 'uninstalling',
    streamConnected: false,
    message: `${action === 'install' ? 'Installing' : 'Uninstalling'} WaSSH Service…`
  })
  const command = action === 'install' ? buildInstallCommand() : buildUninstallCommand()
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
  await probeService(ctx, session, hooks)
  return { ok: true }
}
