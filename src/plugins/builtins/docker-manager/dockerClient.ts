import type { Duplex } from 'stream'
import type { PluginMainContext } from '@plugin-api/main'
import { DOCKER_REQUEST_TIMEOUT_MS } from './defaults'
import type {
  DockerContainerInspectResponse,
  DockerContainerSummary,
  DockerCreateContainerResponse,
  DockerDistributionResponse,
  DockerImageInspectResponse,
  DockerImageSummary,
  DockerInfoResponse,
  DockerNetwork,
  DockerVersionResponse,
  DockerVolume,
  DockerVolumeListResponse
} from './apiTypes'
import {
  dockerRequest,
  dockerStreamRequest,
  dockerUpgradeRequest,
  DockerApiError,
  type DockerHttpRequest
} from './dockerHttp'
import type {
  DockerContainerAction,
  DockerContainerDetailView,
  DockerContainerState,
  DockerContainerView,
  DockerImageView,
  DockerLogChunk,
  DockerMountView,
  DockerNetworkSpec,
  DockerNetworkView,
  DockerPortView,
  DockerPruneResult,
  DockerPullProgress,
  DockerRunRequest,
  DockerStatsView,
  DockerVolumeView,
  DockerFileSpec
} from './protocol'
import { openDockerSocket } from './transport'
import {
  DockerLogDemuxer,
  JsonLineSplitter,
  donePullEvent,
  emptyStats,
  normalizeStats,
  parsePullLine
} from './dockerStreams'

/** Timeout for endpoints that legitimately take a while (pull, prune). */
const LONG_REQUEST_TIMEOUT_MS = 10 * 60 * 1000

export interface DockerClientConfig {
  socketPath: string
  /** Version prefix such as `/v1.45`, or '' when the daemon predates negotiation. */
  apiPrefix: string
}

const CONTAINER_STATES: DockerContainerState[] = [
  'created',
  'running',
  'paused',
  'restarting',
  'removing',
  'exited',
  'dead'
]

function normalizeState(value: string | undefined): DockerContainerState {
  if (!value) {
    return 'unknown'
  }
  const lower = value.toLowerCase()
  return (CONTAINER_STATES.find((state) => state === lower) ?? 'unknown') as DockerContainerState
}

function shortId(id: string): string {
  return id.replace(/^sha256:/, '').slice(0, 12)
}

function parseJson<T>(body: string): T {
  return (body ? JSON.parse(body) : undefined) as T
}

function toPorts(summary: DockerContainerSummary): DockerPortView[] {
  const ports = summary.Ports
  if (!Array.isArray(ports)) {
    return []
  }
  const out: DockerPortView[] = []
  for (const port of ports) {
    if (!port || typeof port.PrivatePort !== 'number') {
      continue
    }
    out.push({
      hostIp: port.IP ?? '',
      hostPort: typeof port.PublicPort === 'number' ? port.PublicPort : null,
      containerPort: port.PrivatePort,
      protocol: port.Type ?? 'tcp'
    })
  }
  return out
}

export function toMountViews(
  mounts: Array<{ Type?: string; Source?: string; Destination?: string; Mode?: string; RW?: boolean }> | null | undefined
): DockerMountView[] {
  if (!Array.isArray(mounts)) {
    return []
  }
  return mounts.map((mount) => ({
    type: mount.Type ?? '',
    source: mount.Source ?? '',
    destination: mount.Destination ?? '',
    mode: mount.Mode ?? '',
    rw: mount.RW !== false
  }))
}

export function toContainerView(summary: DockerContainerSummary): DockerContainerView {
  const id = summary.Id ?? ''
  const names = summary.Names ?? []
  const networks = summary.NetworkSettings?.Networks ?? null
  return {
    id,
    shortId: shortId(id),
    name: (names[0] ?? '').replace(/^\//, ''),
    image: summary.Image ?? '',
    imageId: summary.ImageID ?? '',
    command: summary.Command ?? '',
    state: normalizeState(summary.State),
    status: summary.Status ?? '',
    created: typeof summary.Created === 'number' ? summary.Created * 1000 : 0,
    ports: toPorts(summary),
    mounts: toMountViews(summary.Mounts),
    networks: networks ? Object.keys(networks) : [],
    labels: summary.Labels ?? {}
  }
}

export function toImageView(image: DockerImageSummary): DockerImageView {
  const id = image.Id ?? ''
  return {
    id,
    shortId: shortId(id),
    tags: (image.RepoTags ?? []).filter((tag) => tag && tag !== '<none>:<none>'),
    digests: (image.RepoDigests ?? []).filter(Boolean),
    created: typeof image.Created === 'number' ? image.Created * 1000 : 0,
    size: image.Size ?? image.VirtualSize ?? 0,
    containers: image.Containers ?? 0
  }
}

export function toVolumeView(volume: DockerVolume): DockerVolumeView {
  return {
    name: volume.Name ?? '',
    driver: volume.Driver ?? '',
    mountpoint: volume.Mountpoint ?? '',
    scope: volume.Scope ?? '',
    createdAt: volume.CreatedAt ?? '',
    size: typeof volume.UsageData?.Size === 'number' ? volume.UsageData.Size : null,
    refCount: typeof volume.UsageData?.RefCount === 'number' ? volume.UsageData.RefCount : null,
    labels: volume.Labels ?? {}
  }
}

export function toNetworkView(network: DockerNetwork): DockerNetworkView {
  const id = network.Id ?? ''
  const containers = network.Containers ?? null
  return {
    id,
    shortId: shortId(id),
    name: network.Name ?? '',
    driver: network.Driver ?? '',
    scope: network.Scope ?? '',
    internal: Boolean(network.Internal),
    attachable: Boolean(network.Attachable),
    subnet: network.IPAM?.Config?.[0]?.Subnet ?? '',
    gateway: network.IPAM?.Config?.[0]?.Gateway ?? '',
    containers: containers
      ? Object.entries(containers).map(([containerId, endpoint]) => ({
          id: containerId,
          name: endpoint?.Name ?? shortId(containerId),
          ipv4: (endpoint?.IPv4Address ?? '').split('/')[0]
        }))
      : []
  }
}

function toDetailView(raw: DockerContainerInspectResponse, rawJson: string): DockerContainerDetailView {
  const state = raw.State ?? {}
  const config = raw.Config ?? {}
  const hostConfig = raw.HostConfig ?? {}
  const networkSettings = raw.NetworkSettings ?? {}
  const networks = networkSettings.Networks ?? null

  const portBindings: string[] = []
  const bindings = hostConfig.PortBindings ?? null
  if (bindings) {
    for (const [containerPort, hosts] of Object.entries(bindings)) {
      for (const host of hosts ?? []) {
        portBindings.push(`${host.HostIp || '0.0.0.0'}:${host.HostPort || ''} -> ${containerPort}`)
      }
    }
  }

  return {
    id: raw.Id ?? '',
    name: (raw.Name ?? '').replace(/^\//, ''),
    image: raw.Image ?? config.Image ?? '',
    created: raw.Created ?? '',
    status: state.Status ?? '',
    running: Boolean(state.Running),
    paused: Boolean(state.Paused),
    restarting: Boolean(state.Restarting),
    startedAt: state.StartedAt ?? '',
    finishedAt: state.FinishedAt ?? '',
    exitCode: typeof state.ExitCode === 'number' ? state.ExitCode : null,
    pid: typeof state.Pid === 'number' ? state.Pid : null,
    restartCount: raw.RestartCount ?? 0,
    platform: raw.Platform ?? '',
    command: [...(config.Entrypoint ?? []), ...(config.Cmd ?? [])].join(' ') || `${raw.Path ?? ''} ${(raw.Args ?? []).join(' ')}`.trim(),
    entrypoint: (config.Entrypoint ?? []).join(' '),
    workingDir: config.WorkingDir ?? '',
    user: config.User ?? '',
    tty: Boolean(config.Tty),
    restartPolicy: hostConfig.RestartPolicy?.Name ?? '',
    networkMode: hostConfig.NetworkMode ?? '',
    privileged: Boolean(hostConfig.Privileged),
    autoRemove: Boolean(hostConfig.AutoRemove),
    env: config.Env ?? [],
    binds: hostConfig.Binds ?? [],
    portBindings,
    networks: networks
      ? Object.entries(networks).map(([name, entry]) => ({
          name,
          ipAddress: entry?.IPAddress ?? '',
          gateway: entry?.Gateway ?? ''
        }))
      : [],
    mounts: toMountViews(raw.Mounts),
    raw: rawJson
  }
}

/** Map a container action to its endpoint, path, and query. */
function actionEndpoint(
  id: string,
  action: DockerContainerAction
): { method: string; path: string; query?: Record<string, string | number | boolean> } {
  const base = `/containers/${encodeURIComponent(id)}`
  switch (action) {
    case 'start':
      return { method: 'POST', path: `${base}/start` }
    case 'stop':
      return { method: 'POST', path: `${base}/stop`, query: { t: 10 } }
    case 'restart':
      return { method: 'POST', path: `${base}/restart`, query: { t: 10 } }
    case 'pause':
      return { method: 'POST', path: `${base}/pause` }
    case 'unpause':
      return { method: 'POST', path: `${base}/unpause` }
    case 'kill':
      return { method: 'POST', path: `${base}/kill` }
    case 'remove':
      return { method: 'DELETE', path: base, query: { force: true, v: true } }
  }
}

export interface DockerLogOptions {
  tail: number
  timestamps: boolean
  follow: boolean
}

export interface DockerStatsHandlers {
  onSample: (stats: DockerStatsView) => void
  onError: (error: Error) => void
}

export interface DockerExecHandlers {
  onOpen: (socket: Duplex) => void
  onError: (error: Error) => void
}

export class DockerClient {
  constructor(
    private readonly ctx: PluginMainContext,
    private readonly config: DockerClientConfig
  ) {}

  get socketPath(): string {
    return this.config.socketPath
  }

  private versioned(path: string): string {
    return `${this.config.apiPrefix}${path}`
  }

  private async call<T>(req: DockerHttpRequest & { path: string }): Promise<T> {
    const socket = await openDockerSocket(this.ctx, this.config.socketPath)
    const response = await dockerRequest(socket, {
      ...req,
      path: this.versioned(req.path),
      timeoutMs: req.timeoutMs ?? DOCKER_REQUEST_TIMEOUT_MS
    })
    return parseJson<T>(response.body)
  }

  /** Endpoints that report "already in that state" with 304 are not failures. */
  private async run(path: string, query?: Record<string, string | number | boolean>): Promise<void> {
    try {
      await this.call<void>({ method: 'POST', path, query })
    } catch (error) {
      if (error instanceof DockerApiError && error.status === 304) {
        return
      }
      throw error
    }
  }

  async ping(): Promise<boolean> {
    const socket = await openDockerSocket(this.ctx, this.config.socketPath)
    const response = await dockerRequest(socket, {
      method: 'HEAD',
      path: '/_ping',
      timeoutMs: 5000
    })
    return response.status === 200
  }

  version(): Promise<DockerVersionResponse> {
    return this.call<DockerVersionResponse>({ method: 'GET', path: '/version' })
  }

  info(): Promise<DockerInfoResponse> {
    return this.call<DockerInfoResponse>({ method: 'GET', path: '/info' })
  }

  async listContainers(all: boolean): Promise<DockerContainerView[]> {
    const summaries = await this.call<DockerContainerSummary[]>({
      method: 'GET',
      path: '/containers/json',
      query: { all: all, size: false }
    })
    return (summaries ?? []).map(toContainerView)
  }

  async inspectContainer(id: string): Promise<DockerContainerDetailView> {
    const socket = await openDockerSocket(this.ctx, this.config.socketPath)
    const response = await dockerRequest(socket, {
      method: 'GET',
      path: this.versioned(`/containers/${encodeURIComponent(id)}/json`),
      timeoutMs: DOCKER_REQUEST_TIMEOUT_MS
    })
    const raw = parseJson<DockerContainerInspectResponse>(response.body)
    let pretty = response.body
    try {
      pretty = JSON.stringify(JSON.parse(response.body), null, 2)
    } catch {
      /* leave as-is */
    }
    return toDetailView(raw, pretty)
  }

  containerAction(id: string, action: DockerContainerAction): Promise<void> {
    const endpoint = actionEndpoint(id, action)
    return this.run(endpoint.path, endpoint.query)
  }

  async statsOnce(id: string): Promise<DockerStatsView> {
    try {
      const raw = await this.call<Record<string, unknown>>({
        method: 'GET',
        path: `/containers/${encodeURIComponent(id)}/stats`,
        query: { stream: false, 'one-shot': true },
        timeoutMs: DOCKER_REQUEST_TIMEOUT_MS
      })
      return normalizeStats(raw as never)
    } catch {
      return emptyStats()
    }
  }

  /**
   * Stream container logs. Returns an abort function. `tty` selects raw vs
   * multiplexed framing.
   */
  streamLogs(
    id: string,
    tty: boolean,
    options: DockerLogOptions,
    handlers: {
      onChunks: (chunks: DockerLogChunk[]) => void
      onEnd: (error?: Error) => void
    }
  ): () => void {
    let abort: (() => void) | null = null
    let cancelled = false

    const demuxer = new DockerLogDemuxer(tty)

    void openDockerSocket(this.ctx, this.config.socketPath)
      .then((socket) => {
        if (cancelled) {
          socket.destroy()
          return
        }
        abort = dockerStreamRequest(
          socket,
          {
            method: 'GET',
            path: this.versioned(`/containers/${encodeURIComponent(id)}/logs`),
            query: {
              stdout: true,
              stderr: true,
              tail: options.tail,
              timestamps: options.timestamps,
              follow: options.follow
            },
            stream: true
          },
          {
            onResponse: () => undefined,
            onData: (chunk) => {
              const chunks = demuxer.push(chunk)
              if (chunks.length > 0) {
                handlers.onChunks(chunks)
              }
            },
            onEnd: (error) => {
              const rest = demuxer.flush()
              if (rest.length > 0) {
                handlers.onChunks(rest)
              }
              handlers.onEnd(error)
            }
          }
        )
      })
      .catch((error: unknown) => {
        handlers.onEnd(error instanceof Error ? error : new Error(String(error)))
      })

    return () => {
      cancelled = true
      abort?.()
    }
  }

  /** Stream container stats samples. Returns an abort function. */
  streamStats(id: string, handlers: DockerStatsHandlers): () => void {
    let abort: (() => void) | null = null
    let cancelled = false
    const splitter = new JsonLineSplitter()

    void openDockerSocket(this.ctx, this.config.socketPath)
      .then((socket) => {
        if (cancelled) {
          socket.destroy()
          return
        }
        abort = dockerStreamRequest(
          socket,
          {
            method: 'GET',
            path: this.versioned(`/containers/${encodeURIComponent(id)}/stats`),
            query: { stream: true },
            stream: true
          },
          {
            onResponse: () => undefined,
            onData: (chunk) => {
              for (const line of splitter.push(chunk)) {
                try {
                  handlers.onSample(normalizeStats(JSON.parse(line) as never))
                } catch {
                  /* ignore malformed sample */
                }
              }
            },
            onEnd: (error) => {
              if (error) {
                handlers.onError(error)
              }
            }
          }
        )
      })
      .catch((error: unknown) => {
        handlers.onError(error instanceof Error ? error : new Error(String(error)))
      })

    return () => {
      cancelled = true
      abort?.()
    }
  }

  async listImages(): Promise<DockerImageView[]> {
    const images = await this.call<DockerImageSummary[]>({ method: 'GET', path: '/images/json' })
    return (images ?? []).map(toImageView)
  }

  /** Pull an image, reporting progress. Resolves when the pull finishes. */
  pullImage(image: string, onProgress: (progress: DockerPullProgress) => void): Promise<void> {
    const [fromImage, tag] = splitImageRef(image)
    const splitter = new JsonLineSplitter()

    return new Promise<void>((resolve, reject) => {
      let settled = false
      const finish = (error?: Error): void => {
        if (settled) {
          return
        }
        settled = true
        if (error) {
          reject(error)
        } else {
          onProgress(donePullEvent(image))
          resolve()
        }
      }

      void openDockerSocket(this.ctx, this.config.socketPath)
        .then((socket) => {
          dockerStreamRequest(
            socket,
            {
              method: 'POST',
              path: this.versioned('/images/create'),
              query: { fromImage, tag },
              stream: true
            },
            {
              onResponse: () => undefined,
              onData: (chunk) => {
                for (const line of splitter.push(chunk)) {
                  const event = parsePullLine(line, image)
                  if (!event) {
                    continue
                  }
                  if (event.error) {
                    finish(new Error(event.error))
                    return
                  }
                  onProgress({
                    image,
                    status: event.status,
                    current: event.current,
                    total: event.total,
                    done: false
                  })
                }
              },
              onEnd: (error) => finish(error)
            }
          )
        })
        .catch((error: unknown) => finish(error instanceof Error ? error : new Error(String(error))))
    })
  }

  removeImage(id: string, force: boolean): Promise<void> {
    return this.call<void>({
      method: 'DELETE',
      path: `/images/${encodeURIComponent(id)}`,
      query: { force: force }
    })
  }

  async listVolumes(): Promise<DockerVolumeView[]> {
    const response = await this.call<DockerVolumeListResponse>({ method: 'GET', path: '/volumes' })
    return (response?.Volumes ?? []).map(toVolumeView)
  }

  createVolume(spec: DockerFileSpec): Promise<void> {
    return this.call<void>({
      method: 'POST',
      path: '/volumes/create',
      body: JSON.stringify({
        Name: spec.name || undefined,
        Driver: spec.driver || undefined,
        Labels: labelsFrom(spec.labels)
      })
    })
  }

  removeVolume(name: string, force: boolean): Promise<void> {
    return this.call<void>({
      method: 'DELETE',
      path: `/volumes/${encodeURIComponent(name)}`,
      query: { force: force }
    })
  }

  async pruneVolumes(): Promise<DockerPruneResult> {
    const response = await this.call<{ VolumesDeleted?: string[] | null; SpaceReclaimed?: number }>({
      method: 'POST',
      path: '/volumes/prune',
      timeoutMs: LONG_REQUEST_TIMEOUT_MS
    })
    return { ok: true, reclaimed: response?.SpaceReclaimed ?? 0 }
  }

  async listNetworks(): Promise<DockerNetworkView[]> {
    const networks = await this.call<DockerNetwork[]>({ method: 'GET', path: '/networks' })
    return (networks ?? []).map(toNetworkView)
  }

  createNetwork(spec: DockerNetworkSpec): Promise<void> {
    const ipam =
      spec.subnet || spec.gateway
        ? { Driver: 'default', Config: [{ Subnet: spec.subnet || undefined, Gateway: spec.gateway || undefined }] }
        : undefined
    return this.call<void>({
      method: 'POST',
      path: '/networks/create',
      body: JSON.stringify({
        Name: spec.name,
        Driver: spec.driver || undefined,
        Internal: spec.internal,
        IPAM: ipam
      })
    })
  }

  removeNetwork(id: string): Promise<void> {
    return this.call<void>({ method: 'DELETE', path: `/networks/${encodeURIComponent(id)}` })
  }

  async createContainer(request: DockerRunRequest): Promise<DockerCreateContainerResponse> {
    const body = buildCreateBody(request)
    const created = await this.call<DockerCreateContainerResponse>({
      method: 'POST',
      path: '/containers/create',
      query: { name: request.name || undefined },
      body: JSON.stringify(body)
    })
    return created ?? {}
  }

  startContainer(id: string): Promise<void> {
    return this.run(`/containers/${encodeURIComponent(id)}/start`)
  }

  renameContainer(id: string, name: string): Promise<void> {
    return this.call<void>({
      method: 'POST',
      path: `/containers/${encodeURIComponent(id)}/rename`,
      query: { name }
    })
  }

  removeContainer(id: string, force: boolean, removeVolumes = false): Promise<void> {
    return this.call<void>({
      method: 'DELETE',
      path: `/containers/${encodeURIComponent(id)}`,
      query: { force, v: removeVolumes }
    })
  }

  /** Create a container from a pre-built body (used by update/recreate). */
  async createContainerRaw(
    body: Record<string, unknown>,
    name?: string
  ): Promise<DockerCreateContainerResponse> {
    const created = await this.call<DockerCreateContainerResponse>({
      method: 'POST',
      path: '/containers/create',
      query: { name: name || undefined },
      body: JSON.stringify(body)
    })
    return created ?? {}
  }

  /** Raw inspect payload, for callers that need the structured Config/HostConfig. */
  async inspectContainerRaw(id: string): Promise<DockerContainerInspectResponse> {
    return this.call<DockerContainerInspectResponse>({
      method: 'GET',
      path: `/containers/${encodeURIComponent(id)}/json`
    })
  }

  /** Local image metadata, including the digests recorded at pull time. */
  imageInspect(reference: string): Promise<DockerImageInspectResponse> {
    return this.call<DockerImageInspectResponse>({
      method: 'GET',
      path: `/images/${encodeURIComponent(reference)}/json`
    })
  }

  /**
   * The registry's current manifest digest for a reference. Returns null when
   * the daemon does not support the endpoint; throws when the registry refuses
   * (offline, auth, or the image is local-only).
   */
  async distributionDigest(reference: string): Promise<string | null> {
    const response = await this.call<DockerDistributionResponse>({
      method: 'GET',
      path: `/distribution/${encodeURIComponent(reference)}/json`,
      timeoutMs: 15000
    })
    return response?.Descriptor?.digest ?? null
  }

  async createExec(id: string, command: string): Promise<string> {
    const response = await this.call<{ Id?: string }>({
      method: 'POST',
      path: `/containers/${encodeURIComponent(id)}/exec`,
      body: JSON.stringify({
        AttachStdin: true,
        AttachStdout: true,
        AttachStderr: true,
        Tty: true,
        Cmd: splitCommand(command)
      })
    })
    if (!response?.Id) {
      throw new Error('Docker did not return an exec id')
    }
    return response.Id
  }

  /** Start an exec session and hand back the hijacked byte stream. */
  startExec(execId: string, handlers: DockerExecHandlers): void {
    void openDockerSocket(this.ctx, this.config.socketPath)
      .then((socket) => {
        dockerUpgradeRequest(
          socket,
          {
            method: 'POST',
            path: this.versioned(`/exec/${encodeURIComponent(execId)}/start`),
            headers: { Connection: 'Upgrade', Upgrade: 'tcp' },
            body: JSON.stringify({ Detach: false, Tty: true }),
            timeoutMs: 30000
          },
          {
            onUpgrade: (upgraded) => handlers.onOpen(upgraded),
            onResponse: (status, body) => handlers.onError(new DockerApiError(status, body)),
            onError: (error) => handlers.onError(error)
          }
        )
      })
      .catch((error: unknown) => {
        handlers.onError(error instanceof Error ? error : new Error(String(error)))
      })
  }

  /** Best-effort terminal resize for an active exec session. */
  async resizeExec(execId: string, cols: number, rows: number): Promise<void> {
    try {
      await this.call<void>({
        method: 'POST',
        path: `/exec/${encodeURIComponent(execId)}/resize`,
        query: { h: rows, w: cols },
        timeoutMs: 5000
      })
    } catch {
      /* resize is advisory; a closed session or a daemon without the endpoint is fine */
    }
  }
}

/** Split `nginx:1.25` into `['nginx','1.25']`, defaulting the tag to `latest`. */
export function splitImageRef(image: string): [string, string] {
  const trimmed = image.trim()
  const digestIndex = trimmed.indexOf('@')
  if (digestIndex >= 0) {
    return [trimmed.slice(0, digestIndex), trimmed.slice(digestIndex + 1)]
  }
  const lastColon = trimmed.lastIndexOf(':')
  const lastSlash = trimmed.lastIndexOf('/')
  if (lastColon > lastSlash && lastColon >= 0) {
    return [trimmed.slice(0, lastColon), trimmed.slice(lastColon + 1)]
  }
  return [trimmed, 'latest']
}

export function labelsFrom(entries: Array<{ key: string; value: string }>): Record<string, string> {
  const labels: Record<string, string> = {}
  for (const entry of entries) {
    const key = entry.key.trim()
    if (key) {
      labels[key] = entry.value
    }
  }
  return labels
}

/** Split a command line, honouring simple single/double quoted segments. */
export function splitCommand(command: string): string[] {
  const trimmed = command.trim()
  if (!trimmed) {
    return []
  }
  const parts: string[] = []
  let current = ''
  let quote: '"' | "'" | null = null
  for (let i = 0; i < trimmed.length; i += 1) {
    const char = trimmed[i]
    if (quote) {
      if (char === quote) {
        quote = null
      } else {
        current += char
      }
      continue
    }
    if (char === '"' || char === "'") {
      quote = char
      continue
    }
    if (char === ' ') {
      if (current) {
        parts.push(current)
        current = ''
      }
      continue
    }
    current += char
  }
  if (current) {
    parts.push(current)
  }
  return parts
}

export interface DockerCreateBody {
  [key: string]: unknown
  HostConfig: Record<string, unknown>
}

/** Build the `POST /containers/create` body from a run request. */
export function buildCreateBody(request: DockerRunRequest): DockerCreateBody {
  const [fromImage, tag] = splitImageRef(request.image)
  const image = request.image.includes(':') || request.image.includes('@') ? request.image : `${fromImage}:${tag}`

  const exposedPorts: Record<string, unknown> = {}
  const portBindings: Record<string, Array<{ HostIp: string; HostPort: string }>> = {}
  for (const port of request.ports) {
    const containerPort = Number(port.containerPort)
    if (!Number.isFinite(containerPort) || containerPort <= 0 || containerPort > 65535) {
      continue
    }
    const protocol = port.protocol === 'udp' ? 'udp' : 'tcp'
    const key = `${containerPort}/${protocol}`
    exposedPorts[key] = {}
    portBindings[key] = [{ HostIp: '', HostPort: port.hostPort.trim() }]
  }

  const env = request.env
    .filter((entry) => entry.key.trim())
    .map((entry) => `${entry.key.trim()}=${entry.value}`)

  const binds = request.volumes
    .filter((entry) => entry.source.trim() && entry.destination.trim())
    .map((entry) => `${entry.source.trim()}:${entry.destination.trim()}${entry.readOnly ? ':ro' : ''}`)

  const labels = labelsFrom(request.labels)

  const hostConfig: Record<string, unknown> = {
    RestartPolicy: { Name: request.restartPolicy || 'no' },
    AutoRemove: request.autoRemove
  }
  if (binds.length > 0) {
    hostConfig.Binds = binds
  }
  if (Object.keys(portBindings).length > 0) {
    hostConfig.PortBindings = portBindings
  }
  if (request.network.trim()) {
    hostConfig.NetworkMode = request.network.trim()
  }

  const body: DockerCreateBody = {
    Image: image,
    HostConfig: hostConfig
  }

  if (request.command.trim()) {
    body.Cmd = splitCommand(request.command)
  }
  if (request.entrypoint.trim()) {
    body.Entrypoint = splitCommand(request.entrypoint)
  }
  if (env.length > 0) {
    body.Env = env
  }
  if (Object.keys(exposedPorts).length > 0) {
    body.ExposedPorts = exposedPorts
  }
  if (Object.keys(labels).length > 0) {
    body.Labels = labels
  }

  const advanced = request.advancedJson.trim()
  if (advanced) {
    const parsed = JSON.parse(advanced) as Record<string, unknown>
    if (parsed && typeof parsed === 'object') {
      const nested = parsed.HostConfig
      if (nested && typeof nested === 'object') {
        Object.assign(hostConfig, nested as Record<string, unknown>)
      } else {
        Object.assign(hostConfig, parsed)
      }
    }
  }

  return body
}

export type { DockerLogChunk }
