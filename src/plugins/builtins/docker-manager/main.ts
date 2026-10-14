import type { Duplex } from 'stream'
import type { PluginMainContext, PluginMainModule } from '@plugin-api/main'
import {
  DOCKER_CONFIRM_DESTRUCTIVE_DEFAULT,
  DOCKER_DEFAULT_LOG_TAIL_LINES,
  DOCKER_DEFAULT_REFRESH_INTERVAL_MS,
  DOCKER_DEFAULT_SHOW_STOPPED,
  DOCKER_DEFAULT_SOCKET_PATH,
  DOCKER_DEFAULT_STATS_INTERVAL_MS,
  DOCKER_MAX_LOG_CHARS,
  DOCKER_MAX_REFRESH_INTERVAL_MS,
  DOCKER_MAX_STATS_INTERVAL_MS,
  DOCKER_MIN_REFRESH_INTERVAL_MS,
  DOCKER_MIN_STATS_INTERVAL_MS,
  DOCKER_PROBE_TIMEOUT_MS
} from './defaults'
import { DockerClient } from './dockerClient'
import { detectInstallPlan, detectUpgradePlan, installEngine, upgradeEngine } from './engineInstall'
import {
  computeUpdateStatus,
  runContainerUpdate,
  updateAllContainers as runUpdateAll,
  type UpdateDeps,
  type UpdateOutcome
} from './updateService'
import {
  EMPTY_UPDATE_STATUS,
  isDockerContainerAction,
  isDockerRendererMessage,
  type DockerContainerDetailView,
  type DockerContainerView,
  type DockerEngineStatus,
  type DockerEnvSpec,
  type DockerImageView,
  type DockerMainMessage,
  type DockerNetworkSpec,
  type DockerNetworkView,
  type DockerPortSpec,
  type DockerRunRequest,
  type DockerStatsView,
  type DockerUpdateEvent,
  type DockerUpdateResult,
  type DockerUpdateStatus,
  type DockerVolumeSpec,
  type DockerVolumeView,
  type DockerFileSpec
} from './protocol'
import {
  DockerSocketError,
  describeSocketError,
  dockerTransportKind,
  isSocketMissingError,
  unsupportedTransportReason
} from './transport'
import { canRunHostCommands } from './hostCommand'

interface LogSession {
  id: string
  abort: () => void
}

interface StatsSession {
  id: string
  abort: () => void
}

interface ExecSession {
  containerId: string
  execId: string
  socket: Duplex | null
  closing: boolean
}

interface SessionState {
  stopped: boolean
  socketPath: string
  apiPrefix: string
  client: DockerClient | null
  engine: DockerEngineStatus
  timer: ReturnType<typeof setInterval> | null
  refreshing: boolean
  active: boolean
  logs: LogSession | null
  stats: StatsSession | null
  exec: ExecSession | null
  installing: boolean
  installLines: string[]
  /** Last container list received from the daemon. */
  lastContainers: DockerContainerView[]
  /** Which containers currently run an out-of-date image. */
  updates: DockerUpdateStatus
  /** Container ids with an update in flight. */
  updating: Set<string>
}

const sessionStates = new Map<string, SessionState>()

function instanceKey(ctx: PluginMainContext): string {
  return `${ctx.tabId}:${ctx.pluginId}`
}

function send(ctx: PluginMainContext, message: DockerMainMessage): void {
  ctx.sendToRenderer(message)
}

/* ------------------------------------------------------------------ settings */

function asString(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback
}

function asBoolean(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback
}

function clampNumber(value: unknown, fallback: number, min: number, max: number): number {
  const numeric = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(numeric)) {
    return fallback
  }
  return Math.min(max, Math.max(min, Math.round(numeric)))
}

interface DockerSettings {
  socketPath: string
  showStopped: boolean
  refreshIntervalMs: number
  statsIntervalMs: number
  logTailLines: number
  sudoPassword: string
  confirmDestructive: boolean
}

function readSettings(ctx: PluginMainContext): DockerSettings {
  const s = ctx.getSettings()
  const socketPath = asString(s.socketPath, DOCKER_DEFAULT_SOCKET_PATH).trim()
  return {
    socketPath: socketPath || DOCKER_DEFAULT_SOCKET_PATH,
    showStopped: asBoolean(s.showStopped, DOCKER_DEFAULT_SHOW_STOPPED),
    refreshIntervalMs: clampNumber(
      s.refreshIntervalMs,
      DOCKER_DEFAULT_REFRESH_INTERVAL_MS,
      DOCKER_MIN_REFRESH_INTERVAL_MS,
      DOCKER_MAX_REFRESH_INTERVAL_MS
    ),
    statsIntervalMs: clampNumber(
      s.statsIntervalMs,
      DOCKER_DEFAULT_STATS_INTERVAL_MS,
      DOCKER_MIN_STATS_INTERVAL_MS,
      DOCKER_MAX_STATS_INTERVAL_MS
    ),
    logTailLines: clampNumber(s.logTailLines, DOCKER_DEFAULT_LOG_TAIL_LINES, 10, 5000),
    sudoPassword: asString(s.sudoPassword, ''),
    confirmDestructive: asBoolean(s.confirmDestructive, DOCKER_CONFIRM_DESTRUCTIVE_DEFAULT)
  }
}

/* -------------------------------------------------------------------- engine */

function emptyEngine(ctx: PluginMainContext, socketPath: string): DockerEngineStatus {
  const kind = dockerTransportKind(ctx)
  if (kind === 'unsupported') {
    return {
      state: 'unsupported',
      sessionKind: 'unsupported',
      socketPath,
      message: unsupportedTransportReason(ctx)
    }
  }
  return { state: 'checking', sessionKind: kind, socketPath }
}

function makeClient(ctx: PluginMainContext, state: SessionState): DockerClient {
  const client = new DockerClient(ctx, { socketPath: state.socketPath, apiPrefix: state.apiPrefix })
  state.client = client
  return client
}

function setEngine(ctx: PluginMainContext, state: SessionState, engine: DockerEngineStatus): void {
  state.engine = engine
  send(ctx, { type: 'engine', status: engine })
}

/** Probe the daemon and negotiate an API version. */
async function probeEngine(ctx: PluginMainContext, state: SessionState): Promise<void> {
  const settings = readSettings(ctx)
  state.socketPath = settings.socketPath
  const kind = dockerTransportKind(ctx)

  if (kind === 'unsupported') {
    state.client = null
    stopTimer(state)
    setEngine(ctx, state, emptyEngine(ctx, settings.socketPath))
    return
  }

  setEngine(ctx, state, {
    state: 'checking',
    sessionKind: kind,
    socketPath: settings.socketPath
  })

  state.apiPrefix = ''
  const client = makeClient(ctx, state)

  try {
    const version = await withTimeout(client.version(), DOCKER_PROBE_TIMEOUT_MS)
    const apiVersion = version.ApiVersion ?? ''
    if (/^[0-9]+\.[0-9]+$/.test(apiVersion)) {
      state.apiPrefix = `/v${apiVersion}`
      makeClient(ctx, state)
    }

    let containersRunning: number | undefined
    let containersPaused: number | undefined
    let containersStopped: number | undefined
    let images: number | undefined
    try {
      const info = await withTimeout(makeClient(ctx, state).info(), DOCKER_PROBE_TIMEOUT_MS)
      containersRunning = info.ContainersRunning
      containersPaused = info.ContainersPaused
      containersStopped = info.ContainersStopped
      images = info.Images
    } catch {
      /* /info is optional detail */
    }

    setEngine(ctx, state, {
      state: 'ready',
      sessionKind: kind,
      socketPath: settings.socketPath,
      version: version.Version,
      apiVersion,
      os: version.Os,
      arch: version.Arch,
      containersRunning,
      containersPaused,
      containersStopped,
      images
    })

    armTimer(ctx, state)
  } catch (error) {
    state.client = null
    stopTimer(state)

    if (error instanceof DockerSocketError) {
      const missing = isSocketMissingError(error)
      setEngine(ctx, state, {
        state: error.code === 'EACCES' || error.code === 'EPERM' ? 'forbidden' : 'unavailable',
        sessionKind: kind,
        socketPath: settings.socketPath,
        message: describeSocketError(error),
        installable: missing && error.code !== 'EACCES' && error.code !== 'EPERM' && canRunHostCommands(ctx)
      })
      return
    }

    setEngine(ctx, state, {
      state: 'error',
      sessionKind: kind,
      socketPath: settings.socketPath,
      message: error instanceof Error ? error.message : String(error)
    })
  }
}

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_resolve, reject) => {
      setTimeout(() => reject(new Error('Docker API request timed out')), ms)
    })
  ])
}

/* ------------------------------------------------------------------ polling */

function stopTimer(state: SessionState): void {
  if (state.timer) {
    clearInterval(state.timer)
    state.timer = null
  }
}

function armTimer(ctx: PluginMainContext, state: SessionState): void {
  stopTimer(state)
  if (state.stopped || !state.active || state.engine.state !== 'ready') {
    return
  }
  const { refreshIntervalMs } = readSettings(ctx)
  state.timer = setInterval(() => {
    void refreshContainers(ctx, state)
  }, refreshIntervalMs)
}

async function refreshContainers(ctx: PluginMainContext, state: SessionState): Promise<void> {
  if (state.stopped || state.refreshing) {
    return
  }
  const client = state.client
  if (!client) {
    return
  }
  state.refreshing = true
  try {
    const containers = await client.listContainers(readSettings(ctx).showStopped)
    if (state.stopped) {
      return
    }
    state.lastContainers = containers
    send(ctx, { type: 'containers', containers, at: Date.now() })
  } catch (error) {
    if (state.stopped) {
      return
    }
    handleClientError(ctx, state, error)
  } finally {
    state.refreshing = false
  }
}

/** Surface a request failure, downgrading the engine state if it died. */
function handleClientError(ctx: PluginMainContext, state: SessionState, error: unknown): void {
  if (error instanceof DockerSocketError) {
    stopTimer(state)
    setEngine(ctx, state, {
      state: error.code === 'EACCES' || error.code === 'EPERM' ? 'forbidden' : 'unavailable',
      sessionKind: state.engine.sessionKind,
      socketPath: state.socketPath,
      message: describeSocketError(error),
      installable: isSocketMissingError(error) && canRunHostCommands(ctx)
    })
    return
  }
  const message = error instanceof Error ? error.message : String(error)
  send(ctx, { type: 'notice', message, kind: 'error' })
}

/* ------------------------------------------------------------- subscriptions */

function closeLogs(state: SessionState): void {
  if (state.logs) {
    state.logs.abort()
    state.logs = null
  }
}

function closeStats(state: SessionState): void {
  if (state.stats) {
    state.stats.abort()
    state.stats = null
  }
}

function closeExec(state: SessionState): void {
  const exec = state.exec
  state.exec = null
  if (!exec) {
    return
  }
  exec.closing = true
  if (exec.socket) {
    try {
      exec.socket.destroy()
    } catch {
      /* already gone */
    }
  }
}

/* ------------------------------------------------------------------ helpers */

function requireClient(state: SessionState): DockerClient {
  if (!state.client) {
    throw new Error('Docker is not available on this session')
  }
  return state.client
}

function requireReadyClient(state: SessionState): DockerClient {
  const client = requireClient(state)
  if (state.engine.state !== 'ready') {
    throw new Error(state.engine.message ?? 'Docker engine is not available')
  }
  return client
}

/** One-shot log fetch used by the plugin API and the UI's initial tail. */
function fetchLogs(
  client: DockerClient,
  id: string,
  tty: boolean,
  tail: number,
  timestamps: boolean
): Promise<string> {
  return new Promise((resolve, reject) => {
    let text = ''
    let settled = false
    const abort = client.streamLogs(
      id,
      tty,
      { tail, timestamps, follow: false },
      {
        onChunks: (chunks) => {
          for (const chunk of chunks) {
            text += chunk.text
            if (text.length > DOCKER_MAX_LOG_CHARS) {
              text = text.slice(text.length - DOCKER_MAX_LOG_CHARS)
            }
          }
        },
        onEnd: (error) => {
          if (settled) {
            return
          }
          settled = true
          if (error) {
            reject(error)
          } else {
            resolve(text)
          }
        }
      }
    )
    setTimeout(() => {
      if (!settled) {
        settled = true
        abort()
        resolve(text)
      }
    }, 20000)
  })
}

/** Whether the container's output is framed (non-TTY) or raw (TTY). */
async function containerTty(client: DockerClient, id: string): Promise<boolean> {
  try {
    const detail = await client.inspectContainer(id)
    return detail.tty
  } catch {
    return false
  }
}

async function ensureImage(
  ctx: PluginMainContext,
  client: DockerClient,
  image: string
): Promise<void> {
  try {
    await client.pullImage(image, (progress) => {
      send(ctx, { type: 'pull', progress })
    })
  } catch (error) {
    throw new Error(
      `Could not pull ${image}: ${error instanceof Error ? error.message : String(error)}`
    )
  }
}

function buildRunRequest(params: Record<string, unknown>): DockerRunRequest {
  const asSpecArray = <T>(value: unknown): T[] => (Array.isArray(value) ? (value as T[]) : [])
  const image = String(params.image ?? '').trim()
  return {
    image,
    tag: String(params.tag ?? 'latest'),
    name: String(params.name ?? '').trim(),
    command: String(params.command ?? ''),
    entrypoint: String(params.entrypoint ?? ''),
    detach: params.detach === undefined ? true : Boolean(params.detach),
    autoRemove: Boolean(params.autoRemove),
    restartPolicy: String(params.restartPolicy ?? 'no'),
    network: String(params.network ?? ''),
    ports: asSpecArray<DockerPortSpec>(params.ports),
    env: asSpecArray<DockerEnvSpec>(params.env),
    volumes: asSpecArray<DockerVolumeSpec>(params.volumes),
    labels: asSpecArray<DockerEnvSpec>(params.labels),
    advancedJson: String(params.advancedJson ?? '')
  }
}

/* ------------------------------------------------------------------ updates */

/** Assemble the callbacks the update service needs from this session. */
function updateDeps(ctx: PluginMainContext, state: SessionState): UpdateDeps | null {
  const client = state.client
  if (!client || state.engine.state !== 'ready') {
    return null
  }
  return {
    client,
    emit: (event) => send(ctx, { type: 'update', event }),
    onPullProgress: (progress) => send(ctx, { type: 'pull', progress }),
    refresh: () => refreshContainers(ctx, state)
  }
}

/** Recompute which containers run an image older than the registry's tag. */
async function checkForUpdates(
  ctx: PluginMainContext,
  state: SessionState
): Promise<DockerUpdateStatus> {
  const deps = updateDeps(ctx, state)
  if (!deps || state.stopped) {
    return state.updates
  }

  state.updates = { ...state.updates, checking: true }
  send(ctx, { type: 'updates', status: state.updates })

  const status = await computeUpdateStatus(deps, state.lastContainers)
  if (state.stopped) {
    return state.updates
  }
  state.updates = status
  send(ctx, { type: 'updates', status })
  return status
}

/** Update one container in place, tracking it as in-flight for this session. */
async function updateContainerById(
  ctx: PluginMainContext,
  state: SessionState,
  id: string
): Promise<UpdateOutcome> {
  const deps = updateDeps(ctx, state)
  if (!deps) {
    return { ok: false, error: state.engine.message ?? 'Docker engine is not available' }
  }
  if (state.updating.has(id)) {
    return { ok: false, error: 'This container is already being updated' }
  }

  state.updating.add(id)
  try {
    const outcome = await runContainerUpdate(deps, id)
    if (outcome.ok) {
      state.updates = {
        ...state.updates,
        staleContainerIds: state.updates.staleContainerIds.filter((candidate) => candidate !== id)
      }
      send(ctx, { type: 'updates', status: state.updates })
    }
    return outcome
  } finally {
    state.updating.delete(id)
  }
}

/** Check for stale images, then update every affected container. */
async function updateAllContainers(
  ctx: PluginMainContext,
  state: SessionState
): Promise<DockerUpdateResult> {
  const status = await checkForUpdates(ctx, state)
  const deps = updateDeps(ctx, state)
  if (!deps) {
    return { ok: false, error: state.engine.message ?? 'Docker engine is not available' }
  }
  return runUpdateAll(
    deps,
    state.lastContainers,
    status,
    (containerId) => state.updating.has(containerId),
    (containerId) => updateContainerById(ctx, state, containerId)
  )
}

/* ------------------------------------------------------------------- module */

export const dockerManagerMain: PluginMainModule = {
  async onActivate(ctx) {
    const settings = readSettings(ctx)
    const state: SessionState = {
      stopped: false,
      socketPath: settings.socketPath,
      apiPrefix: '',
      client: null,
      engine: emptyEngine(ctx, settings.socketPath),
      timer: null,
      refreshing: false,
      active: true,
      logs: null,
      stats: null,
      exec: null,
      installing: false,
      installLines: [],
      lastContainers: [],
      updates: { ...EMPTY_UPDATE_STATUS },
      updating: new Set<string>()
    }
    sessionStates.set(instanceKey(ctx), state)

    ctx.onDeactivateCleanup(() => {
      state.stopped = true
      stopTimer(state)
      closeLogs(state)
      closeStats(state)
      closeExec(state)
      sessionStates.delete(instanceKey(ctx))
    })

    await probeEngine(ctx, state)
    if (state.engine.state === 'ready') {
      await refreshContainers(ctx, state)
      // Registry checks are slow and can fail on private registries; run them
      // in the background so the panel is usable immediately.
      void checkForUpdates(ctx, state)
    }
  },

  async onDeactivate(ctx) {
    const state = sessionStates.get(instanceKey(ctx))
    if (!state) {
      return
    }
    state.stopped = true
    stopTimer(state)
    closeLogs(state)
    closeStats(state)
    closeExec(state)
    sessionStates.delete(instanceKey(ctx))
  },

  async onSessionStatus(ctx, event) {
    const state = sessionStates.get(instanceKey(ctx))
    if (!state || state.stopped) {
      return
    }
    if (event.status !== 'connected') {
      return
    }
    // A reconnect invalidates the daemon connection but not the panel state.
    closeLogs(state)
    closeStats(state)
    closeExec(state)
    await probeEngine(ctx, state)
    if (state.engine.state === 'ready') {
      await refreshContainers(ctx, state)
    }
  },

  async onMessage(ctx, payload) {
    const state = sessionStates.get(instanceKey(ctx))
    if (!state || state.stopped) {
      return { ok: false, error: 'Docker panel is not active' }
    }

    if (!isDockerRendererMessage(payload)) {
      return undefined
    }

    switch (payload.type) {
      case 'probe': {
        await probeEngine(ctx, state)
        if (state.engine.state === 'ready') {
          await refreshContainers(ctx, state)
        }
        return { ok: true }
      }

      case 'refresh': {
        await refreshContainers(ctx, state)
        return { ok: true }
      }

      case 'checkUpdates': {
        await checkForUpdates(ctx, state)
        return { ok: true }
      }

      case 'updateContainer': {
        const outcome = await updateContainerById(ctx, state, payload.id)
        return outcome.ok
          ? { ok: true, warnings: outcome.warnings }
          : { ok: false, error: outcome.error, warnings: outcome.warnings }
      }

      case 'updateAll': {
        return updateAllContainers(ctx, state)
      }

      case 'updateEngine': {
        if (state.installing) {
          return { ok: false, error: 'An engine operation is already running' }
        }
        state.installing = true
        try {
          const settings = readSettings(ctx)
          const plan = await detectUpgradePlan(ctx)
          send(ctx, { type: 'engineSetup', lines: [`Upgrading Docker via ${plan.label}`], done: false })
          const result = await upgradeEngine(ctx, plan, settings.sudoPassword, (line) => {
            send(ctx, { type: 'engineSetup', lines: [line], done: false })
          })
          if (!result.ok) {
            send(ctx, {
              type: 'engineSetup',
              lines: [],
              done: true,
              error: result.error ?? 'Update failed'
            })
            return { ok: false, error: result.error, command: plan.displayCommand }
          }
          send(ctx, { type: 'engineSetup', lines: ['Docker engine updated'], done: true })
          await probeEngine(ctx, state)
          if (state.engine.state === 'ready') {
            await refreshContainers(ctx, state)
          }
          return { ok: true }
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error)
          send(ctx, { type: 'engineSetup', lines: [], done: true, error: message })
          return { ok: false, error: message }
        } finally {
          state.installing = false
        }
      }

      case 'containerAction': {
        if (!isDockerContainerAction(payload.action)) {
          return { ok: false, error: 'Unknown container action' }
        }
        try {
          const client = requireReadyClient(state)
          await client.containerAction(payload.id, payload.action)
          await refreshContainers(ctx, state)
          return { ok: true }
        } catch (error) {
          handleClientError(ctx, state, error)
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'inspect': {
        try {
          const detail = await requireReadyClient(state).inspectContainer(payload.id)
          send(ctx, { type: 'detail', detail })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'logsOpen': {
        closeLogs(state)
        const id = payload.id
        const tail = clampNumber(payload.tail, DOCKER_DEFAULT_LOG_TAIL_LINES, 10, 5000)
        try {
          const client = requireReadyClient(state)
          const tty = await containerTty(client, id)
          const abort = client.streamLogs(
            id,
            tty,
            { tail, timestamps: payload.timestamps, follow: true },
            {
              onChunks: (chunks) => send(ctx, { type: 'logs', id, chunks }),
              onEnd: (error) => {
                if (state.logs?.id === id) {
                  state.logs = null
                }
                send(ctx, { type: 'logsEnd', id, error: error?.message })
              }
            }
          )
          state.logs = { id, abort }
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'logsClose': {
        closeLogs(state)
        return { ok: true }
      }

      case 'statsOpen': {
        closeStats(state)
        const id = payload.id
        try {
          const client = requireReadyClient(state)
          const abort = client.streamStats(id, {
            onSample: (stats: DockerStatsView) => send(ctx, { type: 'stats', id, stats }),
            onError: (error) => {
              if (state.stats?.id === id) {
                state.stats = null
              }
              send(ctx, { type: 'notice', message: error.message, kind: 'error' })
            }
          })
          state.stats = { id, abort }
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'statsClose': {
        closeStats(state)
        return { ok: true }
      }

      case 'listImages': {
        try {
          const images: DockerImageView[] = await requireReadyClient(state).listImages()
          send(ctx, { type: 'images', images })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'listVolumes': {
        try {
          const volumes: DockerVolumeView[] = await requireReadyClient(state).listVolumes()
          send(ctx, { type: 'volumes', volumes })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'listNetworks': {
        try {
          const networks: DockerNetworkView[] = await requireReadyClient(state).listNetworks()
          send(ctx, { type: 'networks', networks })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'pullImage': {
        try {
          const client = requireReadyClient(state)
          await ensureImage(ctx, client, payload.image)
          const images = await client.listImages()
          send(ctx, { type: 'images', images })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'removeImage': {
        try {
          const client = requireReadyClient(state)
          await client.removeImage(payload.id, payload.force)
          const images = await client.listImages()
          send(ctx, { type: 'images', images })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'createVolume': {
        try {
          const client = requireReadyClient(state)
          await client.createVolume(payload.spec as DockerFileSpec)
          send(ctx, { type: 'volumes', volumes: await client.listVolumes() })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'removeVolume': {
        try {
          const client = requireReadyClient(state)
          await client.removeVolume(payload.name, payload.force)
          send(ctx, { type: 'volumes', volumes: await client.listVolumes() })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'pruneVolumes': {
        try {
          const client = requireReadyClient(state)
          const result = await client.pruneVolumes()
          send(ctx, { type: 'volumes', volumes: await client.listVolumes() })
          return result
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'createNetwork': {
        try {
          const client = requireReadyClient(state)
          await client.createNetwork(payload.spec as DockerNetworkSpec)
          send(ctx, { type: 'networks', networks: await client.listNetworks() })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'removeNetwork': {
        try {
          const client = requireReadyClient(state)
          await client.removeNetwork(payload.id)
          send(ctx, { type: 'networks', networks: await client.listNetworks() })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'runContainer': {
        try {
          const client = requireReadyClient(state)
          const request = payload.request
          await ensureImage(ctx, client, request.image)
          const created = await client.createContainer(request)
          if (!created.Id) {
            return { ok: false, error: 'Docker did not return a container id' }
          }
          if (request.detach) {
            await client.startContainer(created.Id)
          }
          await refreshContainers(ctx, state)
          return { ok: true, id: created.Id, warnings: created.Warnings ?? undefined }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'execOpen': {
        closeExec(state)
        try {
          const client = requireReadyClient(state)
          const execId = await client.createExec(payload.id, payload.shell)
          const exec: ExecSession = { containerId: payload.id, execId, socket: null, closing: false }
          state.exec = exec
          client.startExec(execId, {
            onOpen: (socket) => {
              if (state.exec !== exec) {
                socket.destroy()
                return
              }
              exec.socket = socket
              send(ctx, { type: 'execOpened', id: payload.id })
              void client.resizeExec(execId, payload.cols, payload.rows)
              socket.on('data', (chunk: Buffer) => {
                send(ctx, { type: 'execData', data: chunk.toString('utf8') })
              })
              socket.on('error', (error: Error) => {
                send(ctx, { type: 'execClosed', error: error.message })
              })
              socket.on('close', () => {
                if (state.exec === exec) {
                  state.exec = null
                }
                if (!exec.closing) {
                  send(ctx, { type: 'execClosed' })
                }
              })
            },
            onError: (error) => {
              if (state.exec === exec) {
                state.exec = null
              }
              send(ctx, { type: 'execClosed', error: error.message })
            }
          })
          return { ok: true }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'execInput': {
        const socket = state.exec?.socket
        if (!socket) {
          return { ok: false, error: 'No container shell is open' }
        }
        socket.write(payload.data)
        return { ok: true }
      }

      case 'execResize': {
        const exec = state.exec
        if (exec) {
          void requireClient(state).resizeExec(exec.execId, payload.cols, payload.rows)
        }
        return { ok: true }
      }

      case 'execClose': {
        closeExec(state)
        return { ok: true }
      }

      case 'detectEngine': {
        try {
          const plan = await detectInstallPlan(ctx)
          return { ok: true, plan }
        } catch (error) {
          return { ok: false, error: error instanceof Error ? error.message : String(error) }
        }
      }

      case 'installEngine': {
        if (state.installing) {
          return { ok: false, error: 'An install is already running' }
        }
        state.installing = true
        try {
          const settings = readSettings(ctx)
          const plan = await detectInstallPlan(ctx)
          state.installLines = []
          send(ctx, { type: 'engineSetup', lines: [`Using ${plan.label}`], done: false })
          const result = await installEngine(ctx, plan, settings.sudoPassword, (line) => {
            state.installLines.push(line)
            send(ctx, { type: 'engineSetup', lines: [line], done: false })
          })
          if (!result.ok) {
            send(ctx, {
              type: 'engineSetup',
              lines: [],
              done: true,
              error: result.error ?? 'Install failed'
            })
            return { ok: false, error: result.error, command: plan.displayCommand }
          }
          send(ctx, { type: 'engineSetup', lines: ['Docker installed'], done: true })
          await probeEngine(ctx, state)
          if (state.engine.state === 'ready') {
            await refreshContainers(ctx, state)
          }
          return { ok: true }
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error)
          send(ctx, { type: 'engineSetup', lines: [], done: true, error: message })
          return { ok: false, error: message }
        } finally {
          state.installing = false
        }
      }

      case 'setActive': {
        state.active = payload.active
        if (state.active) {
          if (state.engine.state === 'checking' || state.engine.state === 'unsupported') {
            await probeEngine(ctx, state)
          }
          armTimer(ctx, state)
          if (state.engine.state === 'ready') {
            await refreshContainers(ctx, state)
          }
        } else {
          stopTimer(state)
          closeLogs(state)
          closeStats(state)
        }
        return { ok: true }
      }
    }
  },

  async onApiCall(ctx, method, params) {
    const state = sessionStates.get(instanceKey(ctx))
    if (!state || state.stopped) {
      throw new Error('Docker manager is not active on this tab')
    }
    const args = params && typeof params === 'object' ? (params as Record<string, unknown>) : {}

    switch (method) {
      case 'list_containers': {
        const client = requireReadyClient(state)
        const all = args.all === undefined ? true : Boolean(args.all)
        const containers: DockerContainerView[] = await client.listContainers(all)
        return containers
      }

      case 'inspect_container': {
        const id = String(args.id ?? '').trim()
        if (!id) {
          throw new Error('Container id is required')
        }
        const detail: DockerContainerDetailView = await requireReadyClient(state).inspectContainer(id)
        return detail
      }

      case 'get_container_logs': {
        const id = String(args.id ?? '').trim()
        if (!id) {
          throw new Error('Container id is required')
        }
        const client = requireReadyClient(state)
        const tail = clampNumber(args.tail, DOCKER_DEFAULT_LOG_TAIL_LINES, 1, 2000)
        const tty = await containerTty(client, id)
        return fetchLogs(client, id, tty, tail, Boolean(args.timestamps))
      }

      case 'get_container_stats': {
        const id = String(args.id ?? '').trim()
        if (!id) {
          throw new Error('Container id is required')
        }
        return requireReadyClient(state).statsOnce(id)
      }

      case 'list_images': {
        return requireReadyClient(state).listImages()
      }

      case 'list_volumes': {
        return requireReadyClient(state).listVolumes()
      }

      case 'list_networks': {
        return requireReadyClient(state).listNetworks()
      }

      case 'container_action': {
        const id = String(args.id ?? '').trim()
        const action = args.action
        if (!id) {
          throw new Error('Container id is required')
        }
        if (!isDockerContainerAction(action)) {
          throw new Error(`Unknown container action: ${String(action)}`)
        }
        const client = requireReadyClient(state)
        await client.containerAction(id, action)
        void refreshContainers(ctx, state)
        return { ok: true }
      }

      case 'remove_container': {
        const id = String(args.id ?? '').trim()
        if (!id) {
          throw new Error('Container id is required')
        }
        const client = requireReadyClient(state)
        await client.containerAction(id, 'remove')
        void refreshContainers(ctx, state)
        return { ok: true }
      }

      case 'pull_image': {
        const image = String(args.image ?? '').trim()
        if (!image) {
          throw new Error('Image reference is required')
        }
        await ensureImage(ctx, requireReadyClient(state), image)
        return { ok: true }
      }

      case 'remove_image': {
        const id = String(args.id ?? '').trim()
        if (!id) {
          throw new Error('Image id or reference is required')
        }
        await requireReadyClient(state).removeImage(id, Boolean(args.force))
        return { ok: true }
      }

      case 'update_container': {
        const id = String(args.id ?? '').trim()
        if (!id) {
          throw new Error('Container id is required')
        }
        const outcome = await updateContainerById(ctx, state, id)
        if (!outcome.ok) {
          throw new Error(outcome.error ?? 'Update failed')
        }
        return { ok: true, warnings: outcome.warnings }
      }

      case 'run_container': {
        const request = buildRunRequest(args)
        if (!request.image) {
          throw new Error('Image reference is required')
        }
        const client = requireReadyClient(state)
        await ensureImage(ctx, client, request.image)
        const created = await client.createContainer(request)
        if (!created.Id) {
          throw new Error('Docker did not return a container id')
        }
        if (request.detach) {
          await client.startContainer(created.Id)
        }
        void refreshContainers(ctx, state)
        return { ok: true, id: created.Id, warnings: created.Warnings ?? undefined }
      }

      default:
        throw new Error(`Unknown Docker API method: ${method}`)
    }
  }
}
