import { readFileSync } from 'node:fs'
import daemonPath from './wassh-service.py?asset'

export const REMOTE_SERVICE_VERSION = 6
export const REMOTE_OPERATION_SUCCESS = '__WASSH_SERVICE_OK__'
export const REMOTE_BASE_PATH = '/usr/local/lib/wassh-service'
export const REMOTE_EXECUTABLE_PATH = `${REMOTE_BASE_PATH}/wassh-service`
/** Unix socket the daemon streams on; mirrored in wassh-service.py */
export const REMOTE_SOCKET_PATH = '/run/wassh-service/stream.sock'
export const REMOTE_SYSTEMD_UNIT_NAME = 'wassh-service.service'
export const REMOTE_SYSTEMD_UNIT_PATH = `/etc/systemd/system/${REMOTE_SYSTEMD_UNIT_NAME}`

function quoteShell(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

function sudoScriptCommand(script: string): string {
  // Deployed scripts must be LF-only; the working tree may use CRLF on Windows
  // (a `\r` in the shebang makes the service fail to start).
  const target = script.replace(/\r\n/g, '\n')
  return `IFS= read -r __wassh_password; printf '%s\\n' "$__wassh_password" | sudo -S -p '' -- sh -c ${quoteShell(target)} && printf '%s\\n' ${quoteShell(REMOTE_OPERATION_SUCCESS)}`
}

/** Daemon source (separate file, shipped as an asset); read at deploy time. */
function daemonSource(): string {
  return readFileSync(daemonPath, 'utf8')
}

const SYSTEMD_UNIT_PAYLOAD = `[Unit]
Description=WaSSH Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${REMOTE_EXECUTABLE_PATH}
RuntimeDirectory=wassh-service
RuntimeDirectoryMode=0755
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
`

export function buildInstallCommand(): string {
  const script = `set -eu
if ! command -v python3 >/dev/null 2>&1; then
  echo 'WaSSH Service requires python3 on the remote host' >&2
  exit 1
fi
install -d -m 0755 ${REMOTE_BASE_PATH}
cat > ${REMOTE_EXECUTABLE_PATH} <<'WASSH_SERVICE_PY'
${daemonSource()}WASSH_SERVICE_PY
chmod 0755 ${REMOTE_EXECUTABLE_PATH}
cat > ${REMOTE_SYSTEMD_UNIT_PATH} <<'WASSH_UNIT'
${SYSTEMD_UNIT_PAYLOAD}WASSH_UNIT
chmod 0644 ${REMOTE_SYSTEMD_UNIT_PATH}
systemctl daemon-reload
systemctl enable ${REMOTE_SYSTEMD_UNIT_NAME}
systemctl restart ${REMOTE_SYSTEMD_UNIT_NAME}`
  return sudoScriptCommand(script)
}

export function buildUninstallCommand(): string {
  const script = `set -eu
systemctl disable --now ${REMOTE_SYSTEMD_UNIT_NAME} 2>/dev/null || true
rm -f ${REMOTE_SYSTEMD_UNIT_PATH}
rm -f /etc/systemd/system/multi-user.target.wants/${REMOTE_SYSTEMD_UNIT_NAME}
systemctl daemon-reload
systemctl reset-failed ${REMOTE_SYSTEMD_UNIT_NAME} 2>/dev/null || true
rm -rf ${REMOTE_BASE_PATH}`
  return sudoScriptCommand(script)
}

export function buildProbeCommand(): string {
  return `if [ "$(uname -s 2>/dev/null)" != Linux ] || ! command -v systemctl >/dev/null 2>&1; then printf 'unsupported\\n'; elif ! command -v python3 >/dev/null 2>&1; then printf 'nopython\\n'; elif [ ! -x ${REMOTE_EXECUTABLE_PATH} ] || [ ! -f ${REMOTE_SYSTEMD_UNIT_PATH} ]; then printf 'missing\\n'; else version=$(sed -n 's/^VERSION *= *//p' ${REMOTE_EXECUTABLE_PATH} | head -n 1); state=$(systemctl is-active ${REMOTE_SYSTEMD_UNIT_NAME} 2>/dev/null || true); printf 'present\\t%s\\t%s\\n' "\${version:-0}" "$state"; fi`
}
