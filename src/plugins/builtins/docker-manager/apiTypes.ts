/**
 * Subset of the Docker Engine API response shapes this plugin consumes.
 * Fields are optional where the daemon may omit them. Names mirror the API
 * (`PascalCase`) so they can be matched against `docker inspect` output and the
 * official docs without a translation layer.
 */

export interface DockerVersionResponse {
  Version?: string
  ApiVersion?: string
  MinAPIVersion?: string
  Os?: string
  Arch?: string
  KernelVersion?: string
  GoVersion?: string
}

export interface DockerInfoResponse {
  Containers?: number
  ContainersRunning?: number
  ContainersPaused?: number
  ContainersStopped?: number
  Images?: number
  ServerVersion?: string
  OperatingSystem?: string
  Driver?: string
  NCPU?: number
  MemTotal?: number
  Warnings?: string[] | null
}

export interface DockerPortMapping {
  IP?: string
  PrivatePort?: number
  PublicPort?: number
  Type?: string
}

export interface DockerMountPoint {
  Type?: string
  Name?: string
  Source?: string
  Destination?: string
  Driver?: string
  Mode?: string
  RW?: boolean
  Propagation?: string
}

export interface DockerContainerSummary {
  Id?: string
  Names?: string[]
  Image?: string
  ImageID?: string
  Command?: string
  Created?: number
  State?: string
  Status?: string
  Ports?: DockerPortMapping[] | null
  Labels?: Record<string, string> | null
  Mounts?: DockerMountPoint[] | null
  SizeRw?: number
  SizeRootFs?: number
  NetworkSettings?: {
    Networks?: Record<string, { IPAddress?: string; NetworkID?: string } | null> | null
  } | null
}

export interface DockerImageSummary {
  Id?: string
  ParentId?: string
  RepoTags?: string[] | null
  RepoDigests?: string[] | null
  Created?: number
  Size?: number
  VirtualSize?: number
  SharedSize?: number
  Containers?: number
  Labels?: Record<string, string> | null
}

export interface DockerVolumeUsage {
  Size?: number
  RefCount?: number
}

export interface DockerVolume {
  Name?: string
  Driver?: string
  Mountpoint?: string
  CreatedAt?: string
  Scope?: string
  Labels?: Record<string, string> | null
  Options?: Record<string, string> | null
  UsageData?: DockerVolumeUsage | null
}

export interface DockerVolumeListResponse {
  Volumes?: DockerVolume[] | null
  Warnings?: string[] | null
}

export interface DockerAddressPool {
  Subnet?: string
  Gateway?: string
  IPRange?: string
}

export interface DockerEndpoint {
  Name?: string
  Id?: string
  MacAddress?: string
  IPv4Address?: string
  IPv6Address?: string
  NetworkID?: string
  Aliases?: string[] | null
}

export interface DockerNetwork {
  Id?: string
  Name?: string
  Driver?: string
  Scope?: string
  Internal?: boolean
  Attachable?: boolean
  Ingress?: boolean
  Created?: string
  EnableIPv6?: boolean
  Labels?: Record<string, string> | null
  Containers?: Record<string, DockerEndpoint> | null
  IPAM?: { Config?: DockerAddressPool[] | null } | null
}

export interface DockerStatsResponse {
  read?: string
  preread?: string
  pids_stats?: { current?: number }
  blkio_stats?: {
    io_service_bytes_recursive?: Array<{ op?: string; value?: number }> | null
  } | null
  networks?: Record<string, { rx_bytes?: number; tx_bytes?: number } | null> | null
  memory_stats?: {
    usage?: number
    limit?: number
    stats?: Record<string, number> | null
  } | null
  cpu_stats?: {
    cpu_usage?: { total_usage?: number; percpu_usage?: number[] | null; usage_in_kernelmode?: number; usage_in_usermode?: number }
    system_cpu_usage?: number
    online_cpus?: number
    throttling_data?: {
      periods?: number
      throttled_periods?: number
      throttled_time?: number
    }
  } | null
  precpu_stats?: {
    cpu_usage?: { total_usage?: number }
    system_cpu_usage?: number
  } | null
}

export interface DockerContainerStateInfo {
  Status?: string
  Running?: boolean
  Paused?: boolean
  Restarting?: boolean
  OOMKilled?: boolean
  Dead?: boolean
  Pid?: number
  ExitCode?: number
  Error?: string
  StartedAt?: string
  FinishedAt?: string
  Health?: { Status?: string; FailingStreak?: number } | null
}

export interface DockerContainerConfig {
  Hostname?: string
  Image?: string
  Env?: string[] | null
  Cmd?: string[] | null
  Entrypoint?: string[] | null
  WorkingDir?: string
  User?: string
  Tty?: boolean
  OpenStdin?: boolean
  ExposedPorts?: Record<string, unknown> | null
  Labels?: Record<string, string> | null
}

export interface DockerHostConfig {
  RestartPolicy?: { Name?: string; MaximumRetryCount?: number } | null
  NetworkMode?: string
  Binds?: string[] | null
  PortBindings?: Record<string, Array<{ HostIp?: string; HostPort?: string }>> | null
  Privileged?: boolean
  AutoRemove?: boolean
  Memory?: number
  NanoCpus?: number
}

export interface DockerNetworkSettings {
  IPAddress?: string
  Networks?: Record<string, { IPAddress?: string; NetworkID?: string; Gateway?: string } | null> | null
  Ports?: Record<string, Array<{ HostIp?: string; HostPort?: string }> | null> | null
}

export interface DockerContainerInspectResponse {
  Id?: string
  Name?: string
  Created?: string
  Path?: string
  Args?: string[] | null
  Image?: string
  State?: DockerContainerStateInfo | null
  Config?: DockerContainerConfig | null
  HostConfig?: DockerHostConfig | null
  NetworkSettings?: DockerNetworkSettings | null
  Mounts?: DockerMountPoint[] | null
  RestartCount?: number
  Platform?: string
}

/** One JSON object from `POST /images/create` (pull progress). */
export interface DockerPullProgressLine {
  status?: string
  id?: string
  progress?: string
  progressDetail?: { current?: number; total?: number }
  error?: string
  errorDetail?: { message?: string }
}

/** One JSON object from `POST /containers/create`. */
export interface DockerCreateContainerResponse {
  Id?: string
  Warnings?: string[] | null
}

export interface DockerImageInspectResponse {
  Id?: string
  RepoTags?: string[] | null
  RepoDigests?: string[] | null
  Created?: string
  Size?: number
  Architecture?: string
  Os?: string
}

/** `GET /distribution/{name}/json` — the registry's current manifest digest. */
export interface DockerDistributionResponse {
  Descriptor?: { digest?: string; mediaType?: string; size?: number }
}

export interface DockerErrorResponse {
  message?: string
}
