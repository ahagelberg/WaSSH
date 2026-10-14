import type { PluginMainContext } from '@plugin-api/main'
import { runHostCommand } from './hostCommand'

export type DockerPackageManager = 'apt' | 'dnf' | 'yum' | 'zypper' | 'pacman' | 'apk' | 'unknown'

export interface EngineInstallPlan {
  manager: DockerPackageManager
  /** Human-readable package manager / distribution label. */
  label: string
  /** The privileged command, without any sudo prefix. */
  installCommand: string
  /** Command shown to the user, including the sudo invocation. */
  displayCommand: string
}

/** Upgrade plan for an already-installed engine. */
export interface EngineUpgradePlan {
  manager: DockerPackageManager
  label: string
  /** The privileged command, without any sudo prefix. */
  upgradeCommand: string
  /** Command shown to the user, including the sudo invocation. */
  displayCommand: string
}

/** Wrap a string in single quotes for safe inclusion in a shell command. */
export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

function managerLabel(manager: DockerPackageManager): string {
  switch (manager) {
    case 'apt':
      return 'apt-get (Debian/Ubuntu)'
    case 'dnf':
      return 'dnf (Fedora/RHEL)'
    case 'yum':
      return 'yum (RHEL/CentOS)'
    case 'zypper':
      return 'zypper (openSUSE/SLES)'
    case 'pacman':
      return 'pacman (Arch)'
    case 'apk':
      return 'apk (Alpine)'
    default:
      return 'unknown package manager'
  }
}

/** Start the daemon after install; failures are not fatal (systemd may be absent). */
const POST_INSTALL = 'systemctl enable --now docker 2>/dev/null || true'

function installCommandFor(manager: DockerPackageManager): string {
  switch (manager) {
    case 'apt':
      return [
        'export DEBIAN_FRONTEND=noninteractive',
        'apt-get update',
        'apt-get install -y docker.io',
        POST_INSTALL
      ].join(' && ')
    case 'dnf':
      return ['dnf install -y moby-engine docker 2>/dev/null || dnf install -y moby-engine', POST_INSTALL].join(
        ' && '
      )
    case 'yum':
      return ['yum install -y docker', POST_INSTALL].join(' && ')
    case 'zypper':
      return ['zypper --non-interactive install docker', POST_INSTALL].join(' && ')
    case 'pacman':
      return ['pacman -Sy --noconfirm docker', POST_INSTALL].join(' && ')
    case 'apk':
      return ['apk add --no-cache docker', 'rc-update add docker default 2>/dev/null || true', 'service docker start 2>/dev/null || true'].join(
        ' && '
      )
    default:
      return ''
  }
}

/** Restart the daemon after an upgrade; failures are not fatal. */
const POST_UPGRADE = 'systemctl try-restart docker 2>/dev/null || true'

function upgradeCommandFor(manager: DockerPackageManager): string {
  switch (manager) {
    case 'apt':
      return [
        'export DEBIAN_FRONTEND=noninteractive',
        'apt-get update',
        'apt-get install -y --only-upgrade docker.io containerd runc || apt-get install -y --only-upgrade docker.io',
        POST_UPGRADE
      ].join(' && ')
    case 'dnf':
      return ['dnf upgrade -y moby-engine docker 2>/dev/null || dnf upgrade -y moby-engine', POST_UPGRADE].join(
        ' && '
      )
    case 'yum':
      return ['yum update -y docker', POST_UPGRADE].join(' && ')
    case 'zypper':
      return ['zypper --non-interactive update docker', POST_UPGRADE].join(' && ')
    case 'pacman':
      return ['pacman -Syu --noconfirm docker', POST_UPGRADE].join(' && ')
    case 'apk':
      return ['apk upgrade docker', 'rc-service docker restart 2>/dev/null || service docker restart 2>/dev/null || true'].join(
        ' && '
      )
    default:
      return ''
  }
}

/** `sudo -n` (passwordless) or `sudo -S` with the password piped on stdin. */
export function sudoPrefix(hasPassword: boolean): string {
  return hasPassword ? "sudo -S -p ''" : 'sudo -n'
}

/**
 * Probe the session host once to learn which package manager owns Docker.
 * Uses the distro id first, then falls back to whatever manager is installed.
 */
async function detectManager(
  ctx: PluginMainContext
): Promise<{ manager: DockerPackageManager; label: string }> {
  const probe = [
    '. /etc/os-release 2>/dev/null || true',
    'echo "OS_ID=${ID:-unknown}"',
    'echo "OS_LIKE=${ID_LIKE:-}"',
    'for m in apt-get dnf yum zypper pacman apk; do command -v "$m" >/dev/null 2>&1 && echo "HAS_$m=1"; done'
  ].join('; ')

  let output = ''
  try {
    output = await runHostCommand(ctx, probe, { timeoutMs: 15000 })
  } catch {
    output = ''
  }

  const id = (/OS_ID=(\S*)/.exec(output)?.[1] ?? '').toLowerCase()
  const like = (/OS_LIKE=([^\n]*)/.exec(output)?.[1] ?? '').toLowerCase()
  const has = (name: string): boolean => output.includes(`HAS_${name}=1`)

  let manager: DockerPackageManager = 'unknown'
  const family = `${id} ${like}`

  if (/ubuntu|debian|mint|pop|raspbian|kali|devuan|elementary|zorin|deepin/.test(family)) {
    manager = 'apt'
  } else if (/fedora/.test(family)) {
    manager = 'dnf'
  } else if (/rhel|centos|rocky|almalinux|oracle|amazon|scientific/.test(family)) {
    manager = has('dnf') ? 'dnf' : 'yum'
  } else if (/opensuse|suse|sles/.test(family)) {
    manager = 'zypper'
  } else if (/arch|manjaro|endeavour/.test(family)) {
    manager = 'pacman'
  } else if (/alpine/.test(family)) {
    manager = 'apk'
  } else if (has('apt-get')) {
    manager = 'apt'
  } else if (has('dnf')) {
    manager = 'dnf'
  } else if (has('yum')) {
    manager = 'yum'
  } else if (has('zypper')) {
    manager = 'zypper'
  } else if (has('pacman')) {
    manager = 'pacman'
  } else if (has('apk')) {
    manager = 'apk'
  }

  return { manager, label: managerLabel(manager) }
}

/**
 * Work out how Docker would be installed on the session host. Runs a single
 * probe command (using the distro id and available package managers) so the
 * plan can be shown to the user before anything privileged happens.
 */
export async function detectInstallPlan(ctx: PluginMainContext): Promise<EngineInstallPlan> {
  const { manager, label } = await detectManager(ctx)
  const installCommand = installCommandFor(manager)
  return {
    manager,
    label,
    installCommand,
    displayCommand: installCommand
      ? `${sudoPrefix(true)} ${installCommand}`
      : 'Docker must be installed manually on this distribution'
  }
}

/** Plan an in-place upgrade of an already-installed engine. */
export async function detectUpgradePlan(ctx: PluginMainContext): Promise<EngineUpgradePlan> {
  const { manager, label } = await detectManager(ctx)
  const upgradeCommand = upgradeCommandFor(manager)
  return {
    manager,
    label,
    upgradeCommand,
    displayCommand: upgradeCommand
      ? `${sudoPrefix(true)} ${upgradeCommand}`
      : 'Docker must be upgraded manually on this distribution'
  }
}

export interface InstallEngineResult {
  ok: boolean
  output: string
  error?: string
}

/**
 * Run the privileged install. The password (when configured) is piped to
 * `sudo -S` on stdin and never appears in the command line.
 */
export async function installEngine(
  ctx: PluginMainContext,
  plan: EngineInstallPlan,
  password: string,
  onLine?: (line: string) => void
): Promise<InstallEngineResult> {
  return runPrivileged(ctx, plan.installCommand, password, onLine, 'install')
}

/** Run the privileged upgrade for an already-installed engine. */
export async function upgradeEngine(
  ctx: PluginMainContext,
  plan: EngineUpgradePlan,
  password: string,
  onLine?: (line: string) => void
): Promise<InstallEngineResult> {
  return runPrivileged(ctx, plan.upgradeCommand, password, onLine, 'upgrade')
}

async function runPrivileged(
  ctx: PluginMainContext,
  command: string,
  password: string,
  onLine: ((line: string) => void) | undefined,
  action: 'install' | 'upgrade'
): Promise<InstallEngineResult> {
  if (!command) {
    return {
      ok: false,
      output: '',
      error: `No automatic ${action} command is available for this distribution`
    }
  }

  const hasPassword = password.length > 0
  const shellCommand = `${sudoPrefix(hasPassword)} sh -c ${shellQuote(command)}`

  let buffer = ''
  const emit = (chunk: string): void => {
    buffer += chunk
    for (const line of chunk.split('\n')) {
      const trimmed = line.trimEnd()
      if (trimmed) {
        onLine?.(trimmed)
      }
    }
  }

  try {
    const output = await runHostCommand(ctx, shellCommand, {
      stdin: hasPassword ? `${password}\n` : undefined,
      onData: emit
    })
    return { ok: true, output: output || buffer }
  } catch (error) {
    return {
      ok: false,
      output: buffer,
      error: error instanceof Error ? error.message : String(error)
    }
  }
}
