/**
 * Wire contract between the Docker manager view and its main-process module.
 * Environment-neutral: imported by both the node and the web TypeScript
 * projects, so it must not use Node or DOM APIs.
 */

/** Container lifecycle state reported by `GET /containers/json`. */
export type DockerContainerState =
  | 'created'
  | 'running'
  | 'paused'
  | 'restarting'
  | 'removing'
  | 'exited'
  | 'dead'
  | 'unknown'

/** Container action a user (or another plugin) can request. */
export type DockerContainerAction =
  | 'start'
  | 'stop'
  | 'restart'
  | 'pause'
  | 'unpause'
  | 'kill'
  | 'remove'

/** Which transport the daemon is reached over for this session. */
export type DockerSessionKind = 'ssh' | 'local' | 'unsupported'

export type DockerEngineState =
  | 'checking'
  | 'ready'
  | 'unavailable'
  | 'forbidden'
  | 'unsupported'
  | 'error'

export interface DockerEngineStatus {
  state: DockerEngineState
  sessionKind: DockerSessionKind
  socketPath: string
  version?: string
  apiVersion?: string
  os?: string
  arch?: string
  containersRunning?: number
  containersPaused?: number
  containersStopped?: number
  images?: number
  message?: string
  /** Whether the engine install wizard is offered (daemon missing). */
  installable?: boolean
}

export interface DockerPortView {
  hostIp: string
  hostPort: number | null
  containerPort: number
  protocol: string
}

export interface DockerMountView {
  type: string
  source: string
  destination: string
  mode: string
  rw: boolean
}

export interface DockerContainerView {
  id: string
  shortId: string
  name: string
  image: string
  imageId: string
  command: string
  state: DockerContainerState
  status: string
  created: number
  ports: DockerPortView[]
  mounts: DockerMountView[]
  networks: string[]
  labels: Record<string, string>
}

export interface DockerImageView {
  id: string
  shortId: string
  tags: string[]
  digests: string[]
  created: number
  size: number
  containers: number
}

export interface DockerVolumeView {
  name: string
  driver: string
  mountpoint: string
  scope: string
  createdAt: string
  size: number | null
  refCount: number | null
  labels: Record<string, string>
}

export interface DockerNetworkContainerView {
  id: string
  name: string
  ipv4: string
}

export interface DockerNetworkView {
  id: string
  shortId: string
  name: string
  driver: string
  scope: string
  internal: boolean
  attachable: boolean
  subnet: string
  gateway: string
  containers: DockerNetworkContainerView[]
}

export interface DockerStatsView {
  timestamp: number
  cpuPercent: number
  memoryUsed: number
  memoryLimit: number
  memoryPercent: number
  pids: number
  netRx: number
  netTx: number
  blockRead: number
  blockWrite: number
}

/** Normalized subset of `docker inspect` shown in the detail panel. */
export interface DockerContainerDetailView {
  id: string
  name: string
  image: string
  created: string
  status: string
  running: boolean
  paused: boolean
  restarting: boolean
  startedAt: string
  finishedAt: string
  exitCode: number | null
  pid: number | null
  restartCount: number
  platform: string
  command: string
  entrypoint: string
  workingDir: string
  user: string
  tty: boolean
  restartPolicy: string
  networkMode: string
  privileged: boolean
  autoRemove: boolean
  env: string[]
  binds: string[]
  portBindings: string[]
  networks: Array<{ name: string; ipAddress: string; gateway: string }>
  mounts: DockerMountView[]
  /** Full raw inspect JSON, pretty-printed, for the "Raw" tab. */
  raw: string
}

export interface DockerLogChunk {
  stream: 'stdout' | 'stderr' | 'raw'
  text: string
}

export interface DockerPullProgress {
  image: string
  status: string
  current: number | null
  total: number | null
  done: boolean
  error?: string
}

/** A single port mapping in the create-container form. */
export interface DockerPortSpec {
  hostPort: string
  containerPort: string
  protocol: string
}

/** An environment variable in the create-container form. */
export interface DockerEnvSpec {
  key: string
  value: string
}

/** A volume/bind entry in the create-container form. */
export interface DockerVolumeSpec {
  /** Bind a host path (`true`) or mount a named volume (`false`). */
  bind: boolean
  source: string
  destination: string
  readOnly: boolean
}

export interface DockerRunRequest {
  image: string
  tag: string
  name: string
  command: string
  entrypoint: string
  detach: boolean
  autoRemove: boolean
  restartPolicy: string
  network: string
  ports: DockerPortSpec[]
  env: DockerEnvSpec[]
  volumes: DockerVolumeSpec[]
  labels: DockerEnvSpec[]
  /** Raw `HostConfig` JSON merged over the generated config. */
  advancedJson: string
}

export interface DockerFileSpec {
  name: string
  driver: string
  labels: DockerEnvSpec[]
}

export interface DockerNetworkSpec {
  name: string
  driver: string
  subnet: string
  gateway: string
  internal: boolean
}

export type DockerRendererMessage =
  | { type: 'refresh' }
  | { type: 'probe' }
  | { type: 'setActive'; active: boolean }
  | { type: 'containerAction'; id: string; action: DockerContainerAction }
  | { type: 'checkUpdates' }
  | { type: 'updateContainer'; id: string }
  | { type: 'updateAll' }
  | { type: 'updateEngine' }
  | { type: 'inspect'; id: string }
  | { type: 'logsOpen'; id: string; tail: number; timestamps: boolean }
  | { type: 'logsClose' }
  | { type: 'statsOpen'; id: string }
  | { type: 'statsClose' }
  | { type: 'listImages' }
  | { type: 'listVolumes' }
  | { type: 'listNetworks' }
  | { type: 'pullImage'; image: string }
  | { type: 'removeImage'; id: string; force: boolean }
  | { type: 'createVolume'; spec: DockerFileSpec }
  | { type: 'removeVolume'; name: string; force: boolean }
  | { type: 'pruneVolumes' }
  | { type: 'createNetwork'; spec: DockerNetworkSpec }
  | { type: 'removeNetwork'; id: string }
  | { type: 'runContainer'; request: DockerRunRequest }
  | { type: 'execOpen'; id: string; shell: string; cols: number; rows: number }
  | { type: 'execInput'; data: string }
  | { type: 'execResize'; cols: number; rows: number }
  | { type: 'execClose' }
  | { type: 'detectEngine' }
  | { type: 'installEngine' }

export type DockerMainMessage =
  | { type: 'engine'; status: DockerEngineStatus }
  | { type: 'containers'; containers: DockerContainerView[]; at: number }
  | { type: 'images'; images: DockerImageView[] }
  | { type: 'volumes'; volumes: DockerVolumeView[] }
  | { type: 'networks'; networks: DockerNetworkView[] }
  | { type: 'detail'; detail: DockerContainerDetailView }
  | { type: 'logs'; id: string; chunks: DockerLogChunk[] }
  | { type: 'logsEnd'; id: string; error?: string }
  | { type: 'stats'; id: string; stats: DockerStatsView }
  | { type: 'pull'; progress: DockerPullProgress }
  | { type: 'updates'; status: DockerUpdateStatus }
  | { type: 'update'; event: DockerUpdateEvent }
  | { type: 'execOpened'; id: string }
  | { type: 'execData'; data: string }
  | { type: 'execClosed'; error?: string }
  | { type: 'engineSetup'; lines: string[]; done: boolean; error?: string }
  | { type: 'notice'; message: string; kind: 'info' | 'error' }
  | { type: 'busy'; busy: boolean }

export interface DockerActionResult {
  ok: boolean
  error?: string
}

export interface DockerRunResult extends DockerActionResult {
  id?: string
  warnings?: string[]
}

export interface DockerPruneResult extends DockerActionResult {
  reclaimed?: number
}

/** Progress phases for a container update (pull + recreate). */
export type DockerUpdatePhase =
  | 'pulling'
  | 'recreating'
  | 'starting'
  | 'done'
  | 'skipped'
  | 'failed'

export interface DockerUpdateEvent {
  id: string
  name: string
  image: string
  phase: DockerUpdatePhase
  message?: string
}

/** Which containers run an image older than the registry's current tag. */
export interface DockerUpdateStatus {
  checking: boolean
  at: number
  staleContainerIds: string[]
  checkedImages: string[]
  errors: Array<{ image: string; error: string }>
}

export interface DockerUpdateResult extends DockerActionResult {
  updated?: string[]
  skipped?: string[]
  failures?: Array<{ id: string; name: string; error: string }>
}

export const EMPTY_UPDATE_STATUS: DockerUpdateStatus = {
  checking: false,
  at: 0,
  staleContainerIds: [],
  checkedImages: [],
  errors: []
}

export interface DockerEngineSetupResult {
  ok: boolean
  error?: string
  /** Human-readable command the user may need to run manually. */
  command?: string
}

const CONTAINER_ACTIONS: string[] = [
  'start',
  'stop',
  'restart',
  'pause',
  'unpause',
  'kill',
  'remove'
]

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function messageType(payload: unknown): string | null {
  if (!isRecord(payload)) {
    return null
  }
  const type = payload.type
  return typeof type === 'string' ? type : null
}

const RENDERER_TYPES = new Set<string>([
  'refresh',
  'probe',
  'setActive',
  'containerAction',
  'checkUpdates',
  'updateContainer',
  'updateAll',
  'updateEngine',
  'inspect',
  'logsOpen',
  'logsClose',
  'statsOpen',
  'statsClose',
  'listImages',
  'listVolumes',
  'listNetworks',
  'pullImage',
  'removeImage',
  'createVolume',
  'removeVolume',
  'pruneVolumes',
  'createNetwork',
  'removeNetwork',
  'runContainer',
  'execOpen',
  'execInput',
  'execResize',
  'execClose',
  'detectEngine',
  'installEngine'
])

const MAIN_TYPES = new Set<string>([
  'engine',
  'containers',
  'images',
  'volumes',
  'networks',
  'detail',
  'logs',
  'logsEnd',
  'stats',
  'pull',
  'updates',
  'update',
  'execOpened',
  'execData',
  'execClosed',
  'engineSetup',
  'notice',
  'busy'
])

export function isDockerRendererMessage(payload: unknown): payload is DockerRendererMessage {
  const type = messageType(payload)
  return type !== null && RENDERER_TYPES.has(type)
}

export function isDockerMainMessage(payload: unknown): payload is DockerMainMessage {
  const type = messageType(payload)
  return type !== null && MAIN_TYPES.has(type)
}

export function isDockerContainerAction(value: unknown): value is DockerContainerAction {
  return typeof value === 'string' && CONTAINER_ACTIONS.includes(value)
}

/** Actions that destroy data or interrupt a running workload. */
export const DOCKER_DESTRUCTIVE_ACTIONS: DockerContainerAction[] = ['remove', 'kill']

export function isDestructiveContainerAction(action: DockerContainerAction): boolean {
  return DOCKER_DESTRUCTIVE_ACTIONS.includes(action)
}
