export const REMOTE_SERVICE_VERSION = 4
export const REMOTE_OPERATION_SUCCESS = '__WASSH_SERVICE_OK__'
export const REMOTE_BASE_PATH = '/usr/local/lib/wassh-service'
export const REMOTE_EXECUTABLE_PATH = `${REMOTE_BASE_PATH}/wassh-service`
export const REMOTE_STATE_PATH = '/var/lib/wassh-service'
export const REMOTE_SYSTEMD_UNIT_NAME = 'wassh-service.service'
export const REMOTE_SYSTEMD_UNIT_PATH = `/etc/systemd/system/${REMOTE_SYSTEMD_UNIT_NAME}`

/** Idle sampling cadence (seconds) */
const INTERVAL_IDLE = 10

/** Streaming sampling cadence (seconds) */
const INTERVAL_STREAM = 1

/** Samples between `df` reads; disk usage changes slowly */
const DF_EVERY = 6

/** Samples between temperature reads while streaming (always 1/10 Hz) */
const TEMP_EVERY_STREAM = 10

/** Maximum interfaces recorded; bounds ladder size and frame length */
const MAX_INTERFACES = 8

/** Frame magic byte, matching `monitorFrames.ts` */
const FRAME_MAGIC = 0x57

/** Frame kind discriminants, matching `monitorFrames.ts` */
const FRAME_KIND_HEADER = 1
const FRAME_KIND_SAMPLE = 2

/** Bytes per disk sector (`/proc/diskstats`) */
const DISK_SECTOR_BYTES = 512

function quoteShell(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

function sudoScriptCommand(script: string): string {
  return `IFS= read -r __wassh_password; printf '%s\\n' "$__wassh_password" | sudo -S -p '' -- sh -c ${quoteShell(script)} && printf '%s\\n' ${quoteShell(REMOTE_OPERATION_SUCCESS)}`
}

/**
 * Remote daemon. Samples `/proc` and `/sys` with shell builtins only - the
 * single external command is `df`, read once per `DF_EVERY` samples. Samples
 * feed an in-RAM multi-resolution ladder; while a client is attached the
 * ladder is also streamed as length-prefixed binary frames.
 */
export const REMOTE_SERVICE_PAYLOAD = `#!/bin/sh
set -eu

VERSION=${REMOTE_SERVICE_VERSION}
INTERVAL_IDLE=${INTERVAL_IDLE}
INTERVAL_STREAM=${INTERVAL_STREAM}
DF_EVERY=${DF_EVERY}
TEMP_EVERY_STREAM=${TEMP_EVERY_STREAM}
MAX_INTERFACES=${MAX_INTERFACES}
DISK_SECTOR_BYTES=${DISK_SECTOR_BYTES}
STATE_DIR=${REMOTE_STATE_PATH}
MAGIC=${FRAME_MAGIC}
KIND_HEADER=${FRAME_KIND_HEADER}
KIND_SAMPLE=${FRAME_KIND_SAMPLE}

# ---------------------------------------------------------------- utilities

# Write an unsigned integer as big-endian octal escapes of the given width.
be() {
  value=$1 width=$2
  shift 2
  shift_amount=$((width * 8))
  while [ "$shift_amount" -gt 0 ]; do
    shift_amount=$((shift_amount - 8))
    printf '\\\\%03o' $(((value >> shift_amount) & 255))
  done
}

# Emit one frame: magic, kind, 4-byte big-endian length, then the body file.
frame() {
  kind=$1 body=$2
  length=$(wc -c < "$body")
  printf '\\\\%03o\\\\%03o' "$MAGIC" "$kind"
  be "$length" 4
  cat "$body"
}

# ------------------------------------------------------------ sample state

# Previous cumulative counters (0 = not yet primed).
prev_cpu_total=0
prev_cpu_idle=0
prev_disk_read=0
prev_disk_write=0
prev_net_at=0

# Latest sampled values, carried across samples where a metric is not re-read.
disk_used_kib=0
disk_total_kib=0
cpu_tenths=0
mem_used_kib=0
mem_total_kib=0
disk_read_bps=0
disk_write_bps=0
temp_count=0
iface_count=0
sample_count=0

# Interface and thermal zone names, fixed for the lifetime of the process.
iface_list=""
temp_zones=""

# Sum of the first cpu line in /proc/stat, split into total and idle jiffies.
read_cpu() {
  read -r cpu_line < /proc/stat || return 0
  set -- $cpu_line
  shift
  total=0 idle=0 index=0
  for value in "$@"; do
    total=$((total + value))
    if [ "$index" -eq 3 ] || [ "$index" -eq 4 ]; then
      idle=$((idle + value))
    fi
    index=$((index + 1))
  done
  cpu_total=$total
  cpu_idle=$idle
}

read_memory() {
  mem_total_kib=0
  mem_available_kib=0
  while read -r key value _rest; do
    case $key in
      MemTotal:) mem_total_kib=$value ;;
      MemAvailable:) mem_available_kib=$value ;;
    esac
    if [ "$mem_total_kib" -gt 0 ] && [ "$mem_available_kib" -gt 0 ]; then
      break
    fi
  done < /proc/meminfo
  mem_used_kib=$((mem_total_kib - mem_available_kib))
}

# Sum read/written sectors across whole disks only (partitions double-count).
read_diskio() {
  total_read=0
  total_write=0
  while read -r _major _minor name rest; do
    case $name in
      sd[a-z]|vd[a-z]|xvd[a-z]|hd[a-z]|nvme[0-9]n[0-9]|mmcblk[0-9]) ;;
      *) continue ;;
    esac
    set -- $rest
    sectors_read=$3
    sectors_written=$7
    total_read=$((total_read + sectors_read))
    total_write=$((total_write + sectors_written))
  done < /proc/diskstats
  disk_read_sectors=$total_read
  disk_write_sectors=$total_write
}

# Per-interface cumulative rx/tx bytes, stored in the fixed iface_list order.
read_net() {
  while read -r name rest; do
    case $name in
      *:) name=\${name%:} ;;
      *) continue ;;
    esac
    [ -z "$name" ] && continue
    position=0 matched=0
    for known in $iface_list; do
      if [ "$known" = "$name" ]; then
        matched=1
        break
      fi
      position=$((position + 1))
    done
    [ "$matched" -eq 1 ] || continue
    set -- $rest
    rx=$1
    shift 8
    tx=$1
    eval "net_rx_$position=\\$rx"
    eval "net_tx_$position=\\$tx"
  done < /proc/net/dev
}

read_temps() {
  temp_count=0
  for zone in $temp_zones; do
    value=""
    read -r value < "/sys/class/thermal/$zone/temp" 2>/dev/null || value=""
    [ -z "$value" ] && value=0
    eval "temp_$temp_count=\\$value"
    temp_count=$((temp_count + 1))
  done
}

read_disk_usage() {
  set -- $(df -Pk / 2>/dev/null | tail -n 1)
  disk_total_kib=\${2:-0}
  disk_used_kib=\${3:-0}
}

read_iface_states() {
  index=0
  for name in $iface_list; do
    state=0
    operstate=""
    read -r operstate < "/sys/class/net/$name/operstate" 2>/dev/null || operstate=""
    [ "$operstate" = up ] && state=1
    eval "iface_state_$index=\\$state"
    index=$((index + 1))
  done
}

# --------------------------------------------------------------- discovery

discover() {
  iface_list=""
  iface_count=0
  for path in /sys/class/net/*; do
    [ -e "$path" ] || continue
    name=\${path##*/}
    [ "$name" = lo ] && continue
    iface_list="$iface_list $name"
    iface_count=$((iface_count + 1))
    [ "$iface_count" -ge "$MAX_INTERFACES" ] && break
  done
  iface_list=\${iface_list# }

  temp_zones=""
  for path in /sys/class/thermal/thermal_zone*; do
    [ -e "$path" ] || continue
    temp_zones="$temp_zones \${path##*/}"
  done
  temp_zones=\${temp_zones# }
}

# ------------------------------------------------------------------ stream

stream_header() {
  body="$STATE_DIR/header.json"
  {
    printf '{"series":['
    printf '{"id":"cpu","label":"CPU","unit":"%%","kind":"gauge","max":100},'
    printf '{"id":"mem:used","label":"Memory used","unit":"B","kind":"gauge","max":null},'
    printf '{"id":"mem:total","label":"Memory total","unit":"B","kind":"gauge","max":null},'
    printf '{"id":"disk:used","label":"Disk used","unit":"B","kind":"gauge","max":null},'
    printf '{"id":"disk:total","label":"Disk total","unit":"B","kind":"gauge","max":null}'
    for zone in $temp_zones; do
      printf ',{"id":"temp:%s","label":"Temp (%s)","unit":"°C","kind":"gauge","max":null}' "$zone" "$zone"
    done
    printf ',{"id":"diskio:read","label":"Disk read","unit":"B/s","kind":"rate","max":null}'
    printf ',{"id":"diskio:write","label":"Disk write","unit":"B/s","kind":"rate","max":null}'
    for name in $iface_list; do
      printf ',{"id":"net:%s:rx","label":"%s RX","unit":"B/s","kind":"rate","max":null}' "$name" "$name"
      printf ',{"id":"net:%s:tx","label":"%s TX","unit":"B/s","kind":"rate","max":null}' "$name" "$name"
      printf ',{"id":"iface:%s:state","label":"%s state","unit":"","kind":"state","max":1}' "$name" "$name"
    done
    printf '],"temperatureZones":['
    separator=
    for zone in $temp_zones; do
      printf '%s"%s"' "$separator" "$zone"
      separator=,
    done
    printf '],"interfaces":['
    separator=
    for name in $iface_list; do
      printf '%s"%s"' "$separator" "$name"
      separator=,
    done
    printf ']}'
  } > "$body"
  frame "$KIND_HEADER" "$body"
}

stream_sample() {
  body="$STATE_DIR/sample.bin"
  {
    be "$timestamp" 4
    be "$cpu_tenths" 2
    be "$mem_used_kib" 4
    be "$mem_total_kib" 4
    be "$disk_used_kib" 4
    be "$disk_total_kib" 4
    be "$temp_count" 2
    index=0
    while [ "$index" -lt "$temp_count" ]; do
      eval "value=\\$temp_$index"
      be "$value" 2
      index=$((index + 1))
    done
    be "$disk_read_bps" 4
    be "$disk_write_bps" 4
    be "$iface_count" 2
    index=0
    while [ "$index" -lt "$iface_count" ]; do
      eval "state=\\$iface_state_$index"
      eval "rx=\\$net_rx_bps_$index"
      eval "tx=\\$net_tx_bps_$index"
      be "$state" 1
      be "$rx" 4
      be "$tx" 4
      index=$((index + 1))
    done
  } > "$body"
  frame "$KIND_SAMPLE" "$body"
}

# -------------------------------------------------------------------- main

discover
mkdir -p "$STATE_DIR"

# --stream starts in streaming mode: 1 Hz sampling with the header emitted
# immediately. Without it the daemon samples at the idle cadence and stays
# silent until a client attaches.
streaming=0
interval=$INTERVAL_IDLE
temp_every=$INTERVAL_IDLE
if [ "\${1:-}" = "--stream" ]; then
  streaming=1
  interval=$INTERVAL_STREAM
  temp_every=$TEMP_EVERY_STREAM
  stream_header
fi

while :; do
  sleep "$interval"
  timestamp=$(date +%s)

  read_cpu
  if [ "$prev_cpu_total" -gt 0 ]; then
    total_delta=$((cpu_total - prev_cpu_total))
    idle_delta=$((cpu_idle - prev_cpu_idle))
    if [ "$total_delta" -gt 0 ]; then
      cpu_tenths=$((1000 * (total_delta - idle_delta) / total_delta))
    fi
  fi
  prev_cpu_total=$cpu_total
  prev_cpu_idle=$cpu_idle

  read_memory
  read_diskio
  read_net
  read_iface_states

  if [ "$prev_net_at" -gt 0 ]; then
    elapsed=$((timestamp - prev_net_at))
    [ "$elapsed" -gt 0 ] || elapsed=1
    disk_read_bps=$(( (disk_read_sectors - prev_disk_read) * DISK_SECTOR_BYTES / elapsed ))
    disk_write_bps=$(( (disk_write_sectors - prev_disk_write) * DISK_SECTOR_BYTES / elapsed ))
    index=0
    while [ "$index" -lt "$iface_count" ]; do
      eval "rx=\\$net_rx_$index"
      eval "tx=\\$net_tx_$index"
      eval "prev_rx=\\$prev_net_rx_$index"
      eval "prev_tx=\\$prev_net_tx_$index"
      eval "net_rx_bps_$index=\\$(( (rx - prev_rx) / elapsed ))"
      eval "net_tx_bps_$index=\\$(( (tx - prev_tx) / elapsed ))"
      index=$((index + 1))
    done
  fi
  prev_disk_read=$disk_read_sectors
  prev_disk_write=$disk_write_sectors
  index=0
  while [ "$index" -lt "$iface_count" ]; do
    eval "prev_net_rx_$index=\\$net_rx_$index"
    eval "prev_net_tx_$index=\\$net_tx_$index"
    index=$((index + 1))
  done
  prev_net_at=$timestamp

  sample_count=$((sample_count + 1))
  if [ $((sample_count % DF_EVERY)) -eq 1 ]; then
    read_disk_usage
  fi
  if [ $((sample_count % temp_every)) -eq 1 ]; then
    read_temps
  fi

  if [ "$streaming" -eq 1 ]; then
    if stream_sample; then
      :
    else
      streaming=0
      interval=$INTERVAL_IDLE
      temp_every=$INTERVAL_IDLE
    fi
  fi
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

export function buildInstallCommand(): string {
  const script = `set -eu
install -d -m 0755 ${REMOTE_BASE_PATH}
install -d -m 0755 ${REMOTE_STATE_PATH}
cat > ${REMOTE_EXECUTABLE_PATH} <<'WASSH_SERVICE'
${REMOTE_SERVICE_PAYLOAD}WASSH_SERVICE
chmod 0755 ${REMOTE_EXECUTABLE_PATH}
cat > ${REMOTE_SYSTEMD_UNIT_PATH} <<'WASSH_UNIT'
${REMOTE_SYSTEMD_UNIT_PAYLOAD}WASSH_UNIT
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
rm -rf ${REMOTE_BASE_PATH} ${REMOTE_STATE_PATH}`
  return sudoScriptCommand(script)
}

export function buildProbeCommand(): string {
  return `if [ "$(uname -s 2>/dev/null)" != Linux ] || ! command -v systemctl >/dev/null 2>&1; then printf 'unsupported\\n'; elif [ ! -x ${REMOTE_EXECUTABLE_PATH} ] || [ ! -f ${REMOTE_SYSTEMD_UNIT_PATH} ]; then printf 'missing\\n'; else version=$(sed -n 's/^VERSION=//p' ${REMOTE_EXECUTABLE_PATH} | head -n 1); state=$(systemctl is-active ${REMOTE_SYSTEMD_UNIT_NAME} 2>/dev/null || true); printf 'present\\t%s\\t%s\\n' "\${version:-0}" "$state"; fi`
}

/**
 * Start the daemon in streaming mode. It switches to the 1 Hz cadence while
 * this connection is open and reverts when a write fails.
 */
export function buildStreamCommand(): string {
  return `${REMOTE_EXECUTABLE_PATH} --stream`
}
