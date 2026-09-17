#!/usr/bin/env python3
"""WaSSH Service - remote monitor daemon.

Samples /proc and /sys, keeps the history in a RAM ring and streams it to
attached clients over a unix socket. Nothing is persisted to disk; the only
file created is the socket in the systemd runtime directory (/run is tmpfs).

The wire format matches src/shared/monitorFrames.ts: length-prefixed binary
frames (magic 0x57, kind byte, big-endian u32 body length, body). The header
body is JSON; a sample body is a fixed-layout record.

Dependencies: python3 standard library only.

Run standalone for testing: python3 wassh-service.py [--selftest]
"""

import json
import os
import re
import selectors
import socket
import sys
import time
from collections import deque

VERSION = 6

# Mirror of REMOTE_SOCKET_PATH in remoteService.ts
SOCKET_PATH = '/run/wassh-service/stream.sock'

# Idle sampling cadence (seconds)
IDLE_INTERVAL = 10

# Streaming sampling cadence (seconds)
STREAM_INTERVAL = 1

# At most one ring entry per this many seconds; bounds RAM regardless of cadence
RING_GRID_SECONDS = 10

# Ring entries kept (60480 x 10 s = 7 d); an entry is one encoded frame
RING_CAPACITY = 7 * 24 * 3600 // RING_GRID_SECONDS

# Samples between disk-usage reads; statvfs is comparatively expensive
DF_EVERY = 6

# Samples between temperature reads (10 x 1 s while streaming)
TEMP_EVERY_STREAM = 10

# Samples between temperature reads while idle (10 x 10 s)
TEMP_EVERY_IDLE = 10

# Maximum interfaces recorded; bounds ladder size and frame length
MAX_INTERFACES = 8

# Bytes per disk sector (/proc/diskstats)
DISK_SECTOR_BYTES = 512

# Frame magic byte and kind discriminants, matching monitorFrames.ts
FRAME_MAGIC = 0x57
KIND_HEADER = 1
KIND_SAMPLE = 2

# Drop a client whose outbound backlog exceeds this many bytes (slow consumer)
CLIENT_BACKLOG_LIMIT = 16 * 1024 * 1024

# Whole disks in /proc/diskstats only; partitions double-count
WHOLE_DISK_RE = re.compile(r'^(?:sd[a-z]|vd[a-z]|xvd[a-z]|hd[a-z]|nvme\d+n\d+|mmcblk\d+)$')


def encode_uint(value, width):
    """Big-endian unsigned integer, clamped to the field width."""
    return max(0, min(2 ** (width * 8) - 1, int(value))).to_bytes(width, 'big')


def encode_int(value, width):
    """Big-endian two's-complement integer, clamped to the field width."""
    limit = 2 ** (width * 8 - 1)
    return max(-limit, min(limit - 1, int(value))).to_bytes(width, 'big', signed=True)


def frame(kind, body):
    return bytes((FRAME_MAGIC, kind)) + encode_uint(len(body), 4) + body


def sample_body(timestamp, cpu_tenths, mem_used_kib, mem_total_kib, disk_used_kib,
                disk_total_kib, temps, disk_read_bps, disk_write_bps, interfaces):
    body = bytearray()
    body += encode_uint(timestamp, 4)
    body += encode_uint(cpu_tenths, 2)
    body += encode_uint(mem_used_kib, 4)
    body += encode_uint(mem_total_kib, 4)
    body += encode_uint(disk_used_kib, 4)
    body += encode_uint(disk_total_kib, 4)
    body += encode_uint(len(temps), 2)
    for celsius_centi in temps:
        body += encode_int(celsius_centi, 2)
    body += encode_uint(disk_read_bps, 4)
    body += encode_uint(disk_write_bps, 4)
    body += encode_uint(len(interfaces), 2)
    for up, rx_bps, tx_bps in interfaces:
        body += encode_uint(1 if up else 0, 1)
        body += encode_uint(rx_bps, 4)
        body += encode_uint(tx_bps, 4)
    return bytes(body)


def header_frame(interfaces, zones):
    series = [
        {'id': 'cpu', 'label': 'CPU', 'unit': '%', 'kind': 'gauge', 'max': 100},
        {'id': 'mem:used', 'label': 'Memory used', 'unit': 'B', 'kind': 'gauge', 'max': None},
        {'id': 'mem:total', 'label': 'Memory total', 'unit': 'B', 'kind': 'gauge', 'max': None},
        {'id': 'disk:used', 'label': 'Disk used', 'unit': 'B', 'kind': 'gauge', 'max': None},
        {'id': 'disk:total', 'label': 'Disk total', 'unit': 'B', 'kind': 'gauge', 'max': None},
    ]
    for zone in zones:
        series.append({'id': f'temp:{zone}', 'label': f'Temp ({zone})',
                       'unit': '\u00b0C', 'kind': 'gauge', 'max': None})
    series.append({'id': 'diskio:read', 'label': 'Disk read', 'unit': 'B/s',
                   'kind': 'rate', 'max': None})
    series.append({'id': 'diskio:write', 'label': 'Disk write', 'unit': 'B/s',
                   'kind': 'rate', 'max': None})
    for name in interfaces:
        series.append({'id': f'net:{name}:rx', 'label': f'{name} RX',
                       'unit': 'B/s', 'kind': 'rate', 'max': None})
        series.append({'id': f'net:{name}:tx', 'label': f'{name} TX',
                       'unit': 'B/s', 'kind': 'rate', 'max': None})
        series.append({'id': f'iface:{name}:state', 'label': f'{name} state',
                       'unit': '', 'kind': 'state', 'max': 1})
    body = json.dumps({'series': series, 'temperatureZones': zones,
                       'interfaces': interfaces},
                      ensure_ascii=True, separators=(',', ':')).encode('ascii')
    return frame(KIND_HEADER, body)


def read_cpu():
    """Total and idle jiffies from the first cpu line in /proc/stat."""
    with open('/proc/stat', 'rb') as handle:
        parts = handle.readline().split()
    if not parts or parts[0] != b'cpu':
        return 0, 0
    values = [int(part) for part in parts[1:]]
    idle = (values[3] if len(values) > 3 else 0) + (values[4] if len(values) > 4 else 0)
    return sum(values), idle


def read_memory():
    total_kib = 0
    available_kib = 0
    with open('/proc/meminfo', 'rb') as handle:
        for raw in handle:
            parts = raw.split()
            if len(parts) < 2:
                continue
            if parts[0] == b'MemTotal:':
                total_kib = int(parts[1])
            elif parts[0] == b'MemAvailable:':
                available_kib = int(parts[1])
            if total_kib and available_kib:
                break
    return max(0, total_kib - available_kib), total_kib


def read_diskio():
    """Cumulative read/written sectors across whole disks only."""
    sectors_read = 0
    sectors_written = 0
    with open('/proc/diskstats', 'rb') as handle:
        for raw in handle:
            parts = raw.split()
            if len(parts) < 10 or not WHOLE_DISK_RE.match(parts[2].decode('ascii', 'replace')):
                continue
            sectors_read += int(parts[5])
            sectors_written += int(parts[9])
    return sectors_read, sectors_written


def read_net(interfaces):
    """Cumulative rx/tx bytes per interface, keyed by name."""
    counters = {}
    with open('/proc/net/dev', 'rb') as handle:
        for raw in handle:
            name, separator, rest = raw.decode('ascii', 'replace').partition(':')
            if not separator:
                continue
            name = name.strip()
            if name not in interfaces:
                continue
            fields = rest.split()
            if len(fields) >= 9:
                counters[name] = (int(fields[0]), int(fields[8]))
    return counters


def read_iface_states(interfaces):
    states = []
    for name in interfaces:
        try:
            with open(f'/sys/class/net/{name}/operstate', 'rb') as handle:
                states.append(handle.readline().strip() == b'up')
        except OSError:
            states.append(False)
    return states


def read_temps(zones):
    values = []
    for zone in zones:
        try:
            with open(f'/sys/class/thermal/{zone}/temp', 'rb') as handle:
                values.append(int(handle.readline().strip()))
        except (OSError, ValueError):
            values.append(0)
    return values


def read_disk_usage():
    stats = os.statvfs('/')
    total_kib = stats.f_blocks * stats.f_frsize // 1024
    used_kib = (stats.f_blocks - stats.f_bfree) * stats.f_frsize // 1024
    return used_kib, total_kib


def discover():
    """Interface names and thermal zones; fixed for the process lifetime."""
    interfaces = []
    for name in sorted(os.listdir('/sys/class/net')):
        if name == 'lo':
            continue
        interfaces.append(name)
        if len(interfaces) >= MAX_INTERFACES:
            break
    zones = []
    thermal = '/sys/class/thermal'
    if os.path.isdir(thermal):
        zones = sorted(name for name in os.listdir(thermal)
                       if name.startswith('thermal_zone'))
    return interfaces, zones


class Monitor:
    def __init__(self):
        self.interfaces, self.zones = discover()
        self.header_bytes = header_frame(self.interfaces, self.zones)
        self.ring = deque(maxlen=RING_CAPACITY)
        self.prev_cpu = None
        self.prev_disk = None
        self.prev_net = None
        self.prev_net_at = 0
        self.cpu_tenths = 0
        self.mem_used_kib = 0
        self.mem_total_kib = 0
        self.disk_used_kib = 0
        self.disk_total_kib = 0
        self.disk_read_bps = 0
        self.disk_write_bps = 0
        self.temps = [0] * len(self.zones)
        self.iface_states = [False] * len(self.interfaces)
        self.sample_count = 0
        self.last_ring_at = 0

    def sample(self, streaming):
        """Read one sample; returns (epoch seconds, encoded frame)."""
        now = int(time.time())
        self.sample_count += 1

        cpu_total, cpu_idle = read_cpu()
        if self.prev_cpu is not None:
            total_delta = cpu_total - self.prev_cpu[0]
            idle_delta = cpu_idle - self.prev_cpu[1]
            if total_delta > 0:
                self.cpu_tenths = max(0, min(1000, 1000 * (total_delta - idle_delta) // total_delta))
        self.prev_cpu = (cpu_total, cpu_idle)

        self.mem_used_kib, self.mem_total_kib = read_memory()

        disk_read_sectors, disk_write_sectors = read_diskio()
        net = read_net(self.interfaces)
        self.iface_states = read_iface_states(self.interfaces)

        rates = [(0, 0)] * len(self.interfaces)
        if self.prev_net is not None:
            elapsed = max(1, now - self.prev_net_at)
            self.disk_read_bps = max(0, (disk_read_sectors - self.prev_disk[0]) * DISK_SECTOR_BYTES // elapsed)
            self.disk_write_bps = max(0, (disk_write_sectors - self.prev_disk[1]) * DISK_SECTOR_BYTES // elapsed)
            for index, name in enumerate(self.interfaces):
                rx, tx = net.get(name, (0, 0))
                prev_rx, prev_tx = self.prev_net.get(name, (rx, tx))
                rates[index] = (max(0, (rx - prev_rx) // elapsed), max(0, (tx - prev_tx) // elapsed))
        self.prev_disk = (disk_read_sectors, disk_write_sectors)
        self.prev_net = net
        self.prev_net_at = now

        if self.sample_count % DF_EVERY == 1:
            self.disk_used_kib, self.disk_total_kib = read_disk_usage()
        temp_every = TEMP_EVERY_STREAM if streaming else TEMP_EVERY_IDLE
        if self.sample_count % temp_every == 1:
            self.temps = read_temps(self.zones)

        interfaces = []
        for index, _name in enumerate(self.interfaces):
            rx_bps, tx_bps = rates[index]
            interfaces.append((self.iface_states[index], rx_bps, tx_bps))

        body = sample_body(now, self.cpu_tenths, self.mem_used_kib, self.mem_total_kib,
                           self.disk_used_kib, self.disk_total_kib, self.temps,
                           self.disk_read_bps, self.disk_write_bps, interfaces)
        return now, frame(KIND_SAMPLE, body)


def open_listener():
    directory = os.path.dirname(SOCKET_PATH)
    os.makedirs(directory, exist_ok=True)
    if os.path.exists(SOCKET_PATH):
        os.unlink(SOCKET_PATH)
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(SOCKET_PATH)
    # Any local user may attach; the data (CPU/memory/disk) is not sensitive.
    os.chmod(SOCKET_PATH, 0o666)
    listener.listen(8)
    listener.setblocking(False)
    return listener


class Server:
    """Single-threaded select loop: one sampler, any number of clients."""

    def __init__(self, monitor):
        self.monitor = monitor
        self.selector = selectors.DefaultSelector()
        self.listener = open_listener()
        self.selector.register(self.listener, selectors.EVENT_READ)
        self.clients = {}
        self.next_sample_at = time.time()

    def run(self):
        print(f'wassh-service {VERSION} listening on {SOCKET_PATH} '
              f'({len(self.monitor.interfaces)} interfaces, '
              f'{len(self.monitor.zones)} thermal zones)', flush=True)
        while True:
            timeout = max(0.0, self.next_sample_at - time.time())
            for key, mask in self.selector.select(timeout):
                if key.fileobj is self.listener:
                    self.accept()
                else:
                    self.service(key.fileobj, mask)
            if time.time() >= self.next_sample_at:
                self.sample()

    def accept(self):
        try:
            client, _address = self.listener.accept()
        except OSError:
            return
        client.setblocking(False)
        backlog = bytearray(self.monitor.header_bytes)
        backlog += b''.join(self.monitor.ring)
        self.clients[client] = backlog
        self.selector.register(client, selectors.EVENT_READ | selectors.EVENT_WRITE)
        # A new client wants live samples now, not at the idle deadline.
        self.next_sample_at = min(self.next_sample_at, time.time() + STREAM_INTERVAL)
        print(f'client connected ({len(self.clients)} total, '
              f'{len(backlog)} B backlog)', flush=True)

    def service(self, client, mask):
        backlog = self.clients.get(client)
        if backlog is None:
            return
        if mask & selectors.EVENT_READ:
            try:
                # Clients only listen; any input is ignored, EOF closes.
                if not client.recv(4096):
                    self.close(client)
                    return
            except OSError:
                self.close(client)
                return
        if mask & selectors.EVENT_WRITE:
            try:
                sent = client.send(backlog)
            except OSError:
                self.close(client)
                return
            del backlog[:sent]
            if not backlog:
                self.selector.modify(client, selectors.EVENT_READ)

    def close(self, client):
        self.clients.pop(client, None)
        try:
            self.selector.unregister(client)
        except KeyError:
            pass
        try:
            client.close()
        except OSError:
            pass
        print(f'client disconnected ({len(self.clients)} total)', flush=True)

    def sample(self):
        streaming = bool(self.clients)
        _timestamp, encoded = self.monitor.sample(streaming)
        now = time.time()
        if now - self.monitor.last_ring_at >= RING_GRID_SECONDS:
            self.monitor.ring.append(encoded)
            self.monitor.last_ring_at = now
        for client, backlog in list(self.clients.items()):
            if not backlog:
                self.selector.modify(client, selectors.EVENT_READ | selectors.EVENT_WRITE)
            backlog += encoded
            if len(backlog) > CLIENT_BACKLOG_LIMIT:
                self.close(client)
        self.next_sample_at = now + (STREAM_INTERVAL if self.clients else IDLE_INTERVAL)


def selftest():
    """Emit one header and one synthetic sample frame (protocol conformance)."""
    body = sample_body(1700000000, 425, 1024, 2048, 512, 4096, [4750, -1025],
                       1234, 5678, [(True, 100, 200)])
    sys.stdout.buffer.write(header_frame(['eth0'], ['zone0']))
    sys.stdout.buffer.write(frame(KIND_SAMPLE, body))
    sys.stdout.buffer.flush()


def main():
    if len(sys.argv) > 1 and sys.argv[1] == '--selftest':
        selftest()
        return
    Server(Monitor()).run()


if __name__ == '__main__':
    main()
