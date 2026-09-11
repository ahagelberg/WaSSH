import { DAEMON_MONITOR_MAX_PING_TARGETS } from './defaults'

export const REMOTE_SERVICE_VERSION = 2
export const REMOTE_OPERATION_SUCCESS = '__WASSH_SERVICE_OK__'
export const REMOTE_BASE_PATH = '/usr/local/lib/wassh-service'
export const REMOTE_EXECUTABLE_PATH = `${REMOTE_BASE_PATH}/wassh-service`
export const REMOTE_CONFIG_PATH = '/etc/wassh-service.conf'
export const REMOTE_STATE_PATH = '/var/lib/wassh-service'
export const REMOTE_RECORDS_PATH = `${REMOTE_STATE_PATH}/records.tsv`
export const REMOTE_STARTED_AT_PATH = `${REMOTE_STATE_PATH}/started_at`
export const REMOTE_SYSTEMD_UNIT_NAME = 'wassh-service.service'
export const REMOTE_SYSTEMD_UNIT_PATH = `/etc/systemd/system/${REMOTE_SYSTEMD_UNIT_NAME}`

const TARGET_PATTERN =
  /^(?=.{1,253}$)(?:[A-Za-z0-9](?:[A-Za-z0-9.-]{0,251}[A-Za-z0-9])?|(?:[A-Fa-f0-9]{0,4}:){2,7}[A-Fa-f0-9]{0,4})$/
const MAX_EPOCH_SECONDS = 8_640_000_000_000

function quoteShell(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

function validatedTargets(targets: string[]): string[] {
  if (targets.length > DAEMON_MONITOR_MAX_PING_TARGETS) {
    throw new Error(`At most ${DAEMON_MONITOR_MAX_PING_TARGETS} ping targets are supported`)
  }
  return targets.map((target) => {
    const normalized = target.trim()
    if (!TARGET_PATTERN.test(normalized) || normalized.includes('..')) {
      throw new Error(`Invalid ping target: ${target}`)
    }
    return normalized
  })
}

function sudoScriptCommand(script: string): string {
  return `IFS= read -r __wassh_password; printf '%s\\n' "$__wassh_password" | sudo -S -p '' -- sh -c ${quoteShell(script)} && printf '%s\\n' ${quoteShell(REMOTE_OPERATION_SUCCESS)}`
}

export const REMOTE_SERVICE_PAYLOAD = `#!/bin/sh
set -eu

VERSION=${REMOTE_SERVICE_VERSION}
INTERVAL=10
RETENTION_SECONDS=604800
MAX_BYTES=10485760
PRUNE_SAMPLES=60
STATE_DIR=${REMOTE_STATE_PATH}
RECORDS=${REMOTE_RECORDS_PATH}

record() {
  printf '%s\\t%s' "$VERSION" "$(date +%s)" >> "$RECORDS"
  for field in "$@"; do printf '\\t%s' "$field" >> "$RECORDS"; done
  printf '\\n' >> "$RECORDS"
}

prune() {
  cutoff=$(($(date +%s) - RETENTION_SECONDS))
  awk -F '\\t' -v cutoff="$cutoff" '$2 >= cutoff' "$RECORDS" > "$RECORDS.new"
  mv "$RECORDS.new" "$RECORDS"
  size=$(wc -c < "$RECORDS")
  if [ "$size" -gt "$MAX_BYTES" ]; then
    tail -c "$MAX_BYTES" "$RECORDS" | sed '1d' > "$RECORDS.new"
    mv "$RECORDS.new" "$RECORDS"
  fi
}

read_cpu() {
  awk '/^cpu / { total=0; for (i=2; i<=NF; i++) total+=$i; print total, $5+$6; exit }' /proc/stat
}

state_transition() {
  kind=$1 name=$2 state=$3 file=$4
  previous=$(cat "$file" 2>/dev/null || true)
  if [ -n "$previous" ] && [ "$previous" != "$state" ]; then
    record event "$kind" "$name" "$state"
  fi
  printf '%s\\n' "$state" > "$file"
}

mkdir -p "$STATE_DIR/interfaces"
rm -rf "$STATE_DIR/pings"
mkdir "$STATE_DIR/pings"
touch "$RECORDS"
# Persist the epoch when monitoring first began, surviving service restarts
# (a fresh install wipes STATE_DIR, so this resets on reinstall). Clients use
# it as the left boundary of "known" state, instead of assuming history
# exists before monitoring actually started.
STARTED_FILE="${REMOTE_STARTED_AT_PATH}"
[ -f "$STARTED_FILE" ] || date +%s > "$STARTED_FILE"
set -- $(read_cpu)
previous_total=$1
previous_idle=$2
sample_count=0
sleep_seconds=$INTERVAL

while :; do
  sleep "$sleep_seconds"
  iteration_started=$(date +%s)
  set -- $(read_cpu)
  total=$1 idle=$2
  total_delta=$((total - previous_total))
  idle_delta=$((idle - previous_idle))
  cpu_tenths=0
  if [ "$total_delta" -gt 0 ]; then
    cpu_tenths=$((1000 * (total_delta - idle_delta) / total_delta))
  fi
  previous_total=$total
  previous_idle=$idle
  set -- $(awk '/^MemTotal:/ { total=$2 } /^MemAvailable:/ { available=$2 } END { print total+0, total-available }' /proc/meminfo)
  memory_total=$1 memory_used=$2
  set -- $(df -Pk / | awk 'NR == 2 { print $2, $3 }')
  disk_total=$1 disk_used=$2
  record sample "$cpu_tenths" "$memory_used" "$memory_total" "$disk_used" "$disk_total"

  seen_dir="$STATE_DIR/interfaces-seen"
  rm -rf "$seen_dir"
  mkdir "$seen_dir"
  for interface_path in /sys/class/net/*; do
    [ -e "$interface_path" ] || continue
    interface=\${interface_path##*/}
    [ "$interface" = lo ] && continue
    state=down
    [ "$(cat "$interface_path/operstate" 2>/dev/null || true)" = up ] && state=up
    state_transition interface "$interface" "$state" "$STATE_DIR/interfaces/$interface"
    : > "$seen_dir/$interface"
  done
  for state_file in "$STATE_DIR/interfaces"/*; do
    [ -f "$state_file" ] || continue
    interface=\${state_file##*/}
    [ -f "$seen_dir/$interface" ] || state_transition interface "$interface" down "$state_file"
  done

  target_index=0
  results_dir="$STATE_DIR/ping-results"
  rm -rf "$results_dir"
  mkdir "$results_dir"
  while IFS= read -r target; do
    [ -n "$target" ] || continue
    target_index=$((target_index + 1))
    (if ping -c 1 -W 2 "$target" >/dev/null 2>&1; then printf 'up\\n'; else printf 'down\\n'; fi) > "$results_dir/$target_index" &
  done < ${REMOTE_CONFIG_PATH}
  wait
  target_index=0
  while IFS= read -r target; do
    [ -n "$target" ] || continue
    target_index=$((target_index + 1))
    state=$(cat "$results_dir/$target_index")
    target_file="$STATE_DIR/pings/$target_index"
    previous=$(sed -n '2p' "$target_file" 2>/dev/null || true)
    if [ -n "$previous" ] && [ "$previous" != "$state" ]; then
      record event ping "$target" "$state"
    fi
    printf '%s\\n%s\\n' "$target" "$state" > "$target_file"
  done < ${REMOTE_CONFIG_PATH}

  sample_count=$((sample_count + 1))
  if [ "$sample_count" -ge "$PRUNE_SAMPLES" ]; then
    prune
    sample_count=0
  fi
  elapsed=$(($(date +%s) - iteration_started))
  sleep_seconds=$((INTERVAL - elapsed))
  [ "$sleep_seconds" -gt 0 ] || sleep_seconds=1
done
`

export const REMOTE_SYSTEMD_UNIT_PAYLOAD = `[Unit]
Description=WaSSH Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${REMOTE_EXECUTABLE_PATH}
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
`

export function buildInstallCommand(pingTargets: string[]): string {
  const config = validatedTargets(pingTargets).join('\n')
  const script = `set -eu
install -d -m 0755 ${REMOTE_BASE_PATH}
install -d -m 0755 ${REMOTE_STATE_PATH}
cat > ${REMOTE_EXECUTABLE_PATH} <<'WASSH_SERVICE'
${REMOTE_SERVICE_PAYLOAD}WASSH_SERVICE
chmod 0755 ${REMOTE_EXECUTABLE_PATH}
cat > ${REMOTE_SYSTEMD_UNIT_PATH} <<'WASSH_UNIT'
${REMOTE_SYSTEMD_UNIT_PAYLOAD}WASSH_UNIT
cat > ${REMOTE_CONFIG_PATH} <<'WASSH_CONFIG'
${config}
WASSH_CONFIG
chmod 0644 ${REMOTE_CONFIG_PATH} ${REMOTE_SYSTEMD_UNIT_PATH}
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
rm -rf ${REMOTE_BASE_PATH} ${REMOTE_STATE_PATH}
rm -f ${REMOTE_CONFIG_PATH}`
  return sudoScriptCommand(script)
}

export function buildProbeCommand(): string {
  return `if [ "$(uname -s 2>/dev/null)" != Linux ] || ! command -v systemctl >/dev/null 2>&1; then printf 'unsupported\\n'; elif [ ! -x ${REMOTE_EXECUTABLE_PATH} ] || [ ! -f ${REMOTE_SYSTEMD_UNIT_PATH} ]; then printf 'missing\\n'; else version=$(sed -n 's/^VERSION=//p' ${REMOTE_EXECUTABLE_PATH} | head -n 1); state=$(systemctl is-active ${REMOTE_SYSTEMD_UNIT_NAME} 2>/dev/null || true); printf 'present\\t%s\\t%s\\n' "\${version:-0}" "$state"; fi`
}

export function buildStreamCommand(sinceEpochSeconds: number): string {
  if (
    !Number.isSafeInteger(sinceEpochSeconds) ||
    sinceEpochSeconds < 0 ||
    sinceEpochSeconds > MAX_EPOCH_SECONDS
  ) {
    throw new Error('Stream epoch must be a non-negative integer')
  }
  // Stamp the initial snapshot with max(since, monitoring start time): if
  // monitoring began after the requested "since", the state is only known
  // from when monitoring started, not further back, so the client can
  // correctly grey out the earlier, genuinely unknown period.
  const current = `since=${sinceEpochSeconds}; started=$(cat ${REMOTE_STARTED_AT_PATH} 2>/dev/null || date +%s); anchor=$since; [ "$started" -gt "$anchor" ] && anchor=$started; for file in ${REMOTE_STATE_PATH}/interfaces/*; do [ -f "$file" ] || continue; name=\${file##*/}; state=$(cat "$file"); printf '${REMOTE_SERVICE_VERSION}\\t%s\\tcurrent\\tinterface\\t%s\\t%s\\n' "$anchor" "$name" "$state"; done; for file in ${REMOTE_STATE_PATH}/pings/*; do [ -f "$file" ] || continue; name=$(sed -n '1p' "$file"); state=$(sed -n '2p' "$file"); printf '${REMOTE_SERVICE_VERSION}\\t%s\\tcurrent\\tping\\t%s\\t%s\\n' "$anchor" "$name" "$state"; done`
  return `{ ${current}; exec tail -n +1 -F ${REMOTE_RECORDS_PATH}; }`
}
