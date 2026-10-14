/** Default Docker daemon socket on the session host */
export const DOCKER_DEFAULT_SOCKET_PATH = '/var/run/docker.sock'

/** Container list auto-refresh interval (ms) */
export const DOCKER_DEFAULT_REFRESH_INTERVAL_MS = 5000

/** Container stats poll interval (ms) */
export const DOCKER_DEFAULT_STATS_INTERVAL_MS = 2000

/** Lines of log tail fetched when a log stream opens */
export const DOCKER_DEFAULT_LOG_TAIL_LINES = 200

/** Whether stopped containers are listed by default */
export const DOCKER_DEFAULT_SHOW_STOPPED = false

/** Whether destructive actions ask for confirmation by default */
export const DOCKER_CONFIRM_DESTRUCTIVE_DEFAULT = true

/** Settings schema bounds */
export const DOCKER_MIN_REFRESH_INTERVAL_MS = 1000
export const DOCKER_MAX_REFRESH_INTERVAL_MS = 60000
export const DOCKER_MIN_STATS_INTERVAL_MS = 500
export const DOCKER_MAX_STATS_INTERVAL_MS = 30000
export const DOCKER_MIN_LOG_TAIL_LINES = 10
export const DOCKER_MAX_LOG_TAIL_LINES = 5000

/** Per-request timeout for one-shot Docker API calls (ms) */
export const DOCKER_REQUEST_TIMEOUT_MS = 20000

/** Timeout for the engine probe when opening the panel (ms) */
export const DOCKER_PROBE_TIMEOUT_MS = 8000

/** Maximum characters of log output retained for one container (bounded buffer) */
export const DOCKER_MAX_LOG_CHARS = 400_000

/** Number of stat samples kept for the sparkline history */
export const DOCKER_STATS_HISTORY = 60

/** Maximum characters of a fully buffered JSON response (guards runaway output) */
export const DOCKER_MAX_JSON_BYTES = 4_000_000

/** Timeout for a host command used by engine detection/install (ms) */
export const DOCKER_HOST_COMMAND_TIMEOUT_MS = 180_000

/** Maximum buffer for host command output (bytes) */
export const DOCKER_HOST_COMMAND_MAX_BUFFER = 4_000_000

/** Selectable shells offered in the container exec panel */
export const DOCKER_SHELL_CHOICES = ['/bin/sh', '/bin/bash', 'sh', 'bash', 'ash', 'zsh']

/** Container actions exposed in the UI and the plugin API */
export const DOCKER_CONTAINER_ACTIONS = [
  'start',
  'stop',
  'restart',
  'pause',
  'unpause',
  'kill',
  'remove'
] as const
