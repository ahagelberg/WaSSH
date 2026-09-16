import type { PluginMainContext } from '@plugin-api/main'
import { MonitorFrameDecoder, type MonitorSample, type MonitorStreamHeader } from '@shared/monitorFrames'
import { MonitorLadder } from '@shared/monitorLadder'
import {
  MONITOR_SERVICE_INITIAL_STATUS,
  type MonitorActionResult,
  type MonitorMainMessage,
  type MonitorServiceStatus
} from './serviceProtocol'
import {
  buildInstallCommand,
  buildProbeCommand,
  buildStreamCommand,
  buildUninstallCommand,
  REMOTE_OPERATION_SUCCESS,
  REMOTE_SERVICE_VERSION
} from './remoteService'

export interface MonitorServiceSession {
  status: MonitorServiceStatus
  /** Series set announced by the daemon stream, null until the header arrives */
  header: MonitorStreamHeader | null
  /** Ladder fed by the daemon stream while it is connected */
  ladder: MonitorLadder | null
  streamId: string | null
  decoder: MonitorFrameDecoder
  probeGeneration: number
  streamGeneration: number
  disposed: boolean
}

export interface MonitorServiceHooks {
  /** Called with every decoded sample, plus the ladder it was folded into. */
  onSample: (sample: MonitorSample, ladder: MonitorLadder, header: MonitorStreamHeader) => void
  /** Called once the daemon's series set is known. */
  onHeader: (header: MonitorStreamHeader) => void
  /** Called when the daemon stream disconnects (including deliberate closes). */
  onStreamClosed: (error?: string) => void
}

export function createServiceSession(): MonitorServiceSession {
  return {
    status: { ...MONITOR_SERVICE_INITIAL_STATUS },
    header: null,
    ladder: null,
    streamId: null,
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

export function closeStream(ctx: PluginMainContext, session: MonitorServiceSession): void {
  if (!session.streamId) {
    return
  }
  const streamId = session.streamId
  session.streamId = null
  ctx.closeSideConnection(streamId)
}

/** Fold one decoded sample into the ladder and forward it to the view. */
function consumeSample(
  ctx: PluginMainContext,
  session: MonitorServiceSession,
  sample: MonitorSample,
  hooks: MonitorServiceHooks
): void {
  const header = session.header
  const ladder = session.ladder
  if (!header || !ladder) {
    return
  }
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
  session.header = null
  session.ladder = null

  const streamId = await ctx.openSideConnection({
    kind: 'ssh-exec',
    command: buildStreamCommand()
  })
  if (session.disposed || generation !== session.streamGeneration) {
    ctx.closeSideConnection(streamId)
    return
  }
  session.streamId = streamId
  const offData = ctx.onSideData(streamId, (data) => {
    for (const frame of session.decoder.push(Buffer.from(data, 'binary'))) {
      if (frame.kind === 1) {
        session.header = frame.header
        session.ladder = new MonitorLadder(frame.header.series)
        hooks.onHeader(frame.header)
        continue
      }
      consumeSample(ctx, session, frame.sample, hooks)
    }
  })
  const offClosed = ctx.onSideClosed(streamId, (error) => {
    offData()
    offClosed()
    if (session.streamId === streamId) {
      session.streamId = null
    }
    if (session.disposed || generation !== session.streamGeneration) {
      return
    }
    setStatus(ctx, session, {
      ...session.status,
      streamConnected: false,
      message: error || 'WaSSH Service stream closed'
    })
    hooks.onStreamClosed(error)
  })
  setStatus(ctx, session, { ...session.status, streamConnected: true, message: undefined })
}

export async function probeService(
  ctx: PluginMainContext,
  session: MonitorServiceSession,
  hooks: MonitorServiceHooks
): Promise<void> {
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
    await startStream(ctx, session, hooks)
  } catch (error) {
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
