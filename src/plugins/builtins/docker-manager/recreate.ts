import type {
  DockerContainerInspectResponse,
  DockerEndpoint,
  DockerMountPoint
} from './apiTypes'

export interface RecreatePlan {
  name: string
  /** Image reference to pull before the container is recreated. */
  image: string
  /** Body for `POST /containers/create`. */
  body: Record<string, unknown>
  /** Non-fatal notes surfaced to the user before the update runs. */
  warnings: string[]
}

/** Network modes that need no `NetworkingConfig` endpoints. */
const BUILT_IN_NETWORK_MODES = new Set(['default', 'bridge', 'host', 'none'])

/**
 * `HostConfig` fields that are container *inputs* and therefore safe to replay.
 * Deliberately excluded: `NetworkMode` is handled with the network endpoints,
 * and resolved/output-only fields are dropped.
 */
const HOST_CONFIG_KEYS = [
  'Binds',
  'PortBindings',
  'RestartPolicy',
  'Privileged',
  'AutoRemove',
  'CapAdd',
  'CapDrop',
  'Devices',
  'Tmpfs',
  'ReadonlyRootfs',
  'SecurityOpt',
  'Sysctls',
  'Ulimits',
  'Dns',
  'DnsOptions',
  'DnsSearch',
  'ExtraHosts',
  'LogConfig',
  'Init',
  'PidMode',
  'IpcMode',
  'UTSMode',
  'UsernsMode',
  'CgroupnsMode',
  'CgroupParent',
  'ShmSize',
  'OomKillDisable',
  'Memory',
  'MemorySwap',
  'MemoryReservation',
  'NanoCpus',
  'CpuShares',
  'CpuQuota',
  'CpuPeriod',
  'CpusetCpus',
  'CpusetMems',
  'GroupAdd',
  'DeviceCgroupRules',
  'Runtime',
  'Isolation',
  'MaskedPaths',
  'ReadonlyPaths',
  'StopTimeout'
] as const

/** `Config` fields that are container inputs and therefore safe to replay. */
const CONFIG_KEYS = [
  'Image',
  'Cmd',
  'Entrypoint',
  'Env',
  'WorkingDir',
  'User',
  'Tty',
  'OpenStdin',
  'StdinOnce',
  'Labels',
  'ExposedPorts',
  'Volumes',
  'Healthcheck',
  'StopSignal',
  'StopTimeout',
  'Shell'
] as const

const COMPOSE_PROJECT_LABEL = 'com.docker.compose.project'

/** Aliases the daemon generates per container; replaying them would be wrong. */
function isGeneratedAlias(alias: string, containerId: string, containerName: string): boolean {
  return (
    alias === containerId ||
    alias === containerId.slice(0, 12) ||
    alias === containerName ||
    /^[0-9a-f]{12,}$/.test(alias)
  )
}

/** Rebuild a create body from an inspected container. */
export function buildRecreatePlan(raw: DockerContainerInspectResponse): RecreatePlan {
  const warnings: string[] = []
  const config = raw.Config ?? {}
  const hostConfig = raw.HostConfig ?? {}
  const containerId = raw.Id ?? ''
  const name = (raw.Name ?? '').replace(/^\//, '')

  const image = (config.Image ?? raw.Image ?? '').trim()
  if (!image) {
    throw new Error('This container has no image reference to update')
  }
  const reference = image.includes(':') || image.includes('@') || image.includes('/') ? image : `${image}:latest`

  const labels = config.Labels ?? null
  if (labels && labels[COMPOSE_PROJECT_LABEL]) {
    warnings.push(
      `This container belongs to Compose project "${labels[COMPOSE_PROJECT_LABEL]}". Recreating it here takes it out of Compose's control; "docker compose up -d" is usually better.`
    )
  }

  const body: Record<string, unknown> = {}
  for (const key of CONFIG_KEYS) {
    const value = (config as Record<string, unknown>)[key]
    if (value !== undefined && value !== null && value !== '') {
      body[key] = value
    }
  }
  body.Image = reference

  const nextHostConfig: Record<string, unknown> = {}
  for (const key of HOST_CONFIG_KEYS) {
    const value = (hostConfig as Record<string, unknown>)[key]
    if (value !== undefined && value !== null && value !== '') {
      nextHostConfig[key] = value
    }
  }

  // Volumes: the original create flags when available, otherwise rebuild the
  // Binds list from the resolved mounts (covers Compose-created containers).
  const binds = Array.isArray(hostConfig.Binds) ? hostConfig.Binds.filter(Boolean) : []
  if (binds.length > 0) {
    nextHostConfig.Binds = binds
  } else {
    const derived = bindsFromMounts(raw.Mounts, warnings)
    if (derived.length > 0) {
      nextHostConfig.Binds = derived
    }
  }
  warnAboutUnattachedVolumes(raw.Mounts, nextHostConfig.Binds, warnings)

  // Ports: the daemon splits these across Config.ExposedPorts and
  // HostConfig.PortBindings; both need to come back.
  const portBindings = hostConfig.PortBindings ?? null
  const exposedPorts: Record<string, unknown> = { ...(config.ExposedPorts ?? {}) }
  if (portBindings) {
    for (const key of Object.keys(portBindings)) {
      exposedPorts[key] = exposedPorts[key] ?? {}
    }
  }
  if (Object.keys(exposedPorts).length > 0) {
    body.ExposedPorts = exposedPorts
  }

  const networkMode = hostConfig.NetworkMode ?? ''
  if (networkMode.startsWith('container:')) {
    warnings.push(
      `This container shares another container's network namespace (${networkMode}); the new container will need that container to still exist.`
    )
  }
  if (networkMode) {
    nextHostConfig.NetworkMode = networkMode
  }

  // A restart policy and AutoRemove are mutually exclusive.
  if (nextHostConfig.AutoRemove === true) {
    nextHostConfig.RestartPolicy = { Name: 'no' }
  }

  body.HostConfig = nextHostConfig

  if (networkMode && !BUILT_IN_NETWORK_MODES.has(networkMode) && !networkMode.startsWith('container:')) {
    const endpoints = endpointsFor(raw.NetworkSettings?.Networks ?? null, containerId, name)
    if (Object.keys(endpoints).length > 0) {
      body.NetworkingConfig = { EndpointsConfig: endpoints }
    }
  }

  return { name, image: reference, body, warnings }
}

/** Binds rebuilt from resolved mounts, for containers created without `-v` flags. */
function bindsFromMounts(
  mounts: DockerMountPoint[] | null | undefined,
  warnings: string[]
): string[] {
  if (!Array.isArray(mounts)) {
    return []
  }
  const binds: string[] = []
  for (const mount of mounts) {
    const destination = mount.Destination ?? ''
    if (!destination) {
      continue
    }
    const mode = mount.RW === false ? ':ro' : ''
    if (mount.Type === 'volume') {
      const source = mount.Name ?? mount.Source ?? ''
      if (source) {
        binds.push(`${source}:${destination}${mode}`)
      }
      continue
    }
    if (mount.Type === 'bind') {
      const source = mount.Source ?? ''
      if (source) {
        binds.push(`${source}:${destination}${mode}`)
      }
      continue
    }
    if (mount.Type === 'tmpfs') {
      // Replayed through HostConfig.Tmpfs.
      continue
    }
    warnings.push(`Mount type "${mount.Type}" at ${destination} could not be replayed`)
  }
  return binds
}

/** Network endpoints to rejoin, without the daemon-generated aliases. */
function endpointsFor(
  networks: Record<string, DockerEndpoint | null> | null,
  containerId: string,
  containerName: string
): Record<string, unknown> {
  if (!networks) {
    return {}
  }
  const out: Record<string, unknown> = {}
  for (const [networkName, endpoint] of Object.entries(networks)) {
    if (BUILT_IN_NETWORK_MODES.has(networkName) || !endpoint) {
      continue
    }
    const aliases = (endpoint.Aliases ?? []).filter(
      (alias) => alias && !isGeneratedAlias(alias, containerId, containerName)
    )
    out[networkName] = aliases.length > 0 ? { Aliases: aliases } : {}
  }
  return out
}

/** Warn about volume mounts the replayed binds do not reference. */
function warnAboutUnattachedVolumes(
  mounts: DockerMountPoint[] | null | undefined,
  binds: unknown,
  warnings: string[]
): void {
  if (!Array.isArray(mounts)) {
    return
  }
  const sources = new Set<string>()
  if (Array.isArray(binds)) {
    for (const bind of binds) {
      if (typeof bind === 'string') {
        sources.add(bind.split(':')[0])
      }
    }
  }
  for (const mount of mounts) {
    if (mount.Type !== 'volume') {
      continue
    }
    const name = mount.Name ?? mount.Source ?? ''
    if (name && !sources.has(name)) {
      warnings.push(
        `Anonymous volume at ${mount.Destination} is not reattached; it is preserved on the host but the new container will not use it.`
      )
    }
  }
}

/** Unique backup name for the container being replaced. */
export function backupName(name: string, containerId: string, now = Date.now()): string {
  const base = name || containerId.slice(0, 12)
  const stamp = new Date(now).toISOString().replace(/[-:.TZ]/g, '').slice(0, 14)
  return `${base}-wassh-old-${stamp}`
}
