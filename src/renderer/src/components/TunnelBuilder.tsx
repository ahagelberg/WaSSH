import { useState } from 'react'
import {
  DEFAULT_TUNNEL_LISTEN_HOST,
  TUNNEL_PORT_MAX,
  TUNNEL_PORT_MIN,
  TUNNEL_TYPE_DYNAMIC,
  TUNNEL_TYPE_LOCAL,
  TUNNEL_TYPE_REMOTE,
  type SshTunnel,
  type TunnelType
} from '@shared/types'
import { emptyTunnel } from '@shared/connection'

/** Longest valid port is 65535, so cap typed digits at five */
const PORT_INPUT_MAX_DIGITS = 5

interface TunnelTypeMeta {
  type: TunnelType
  badge: string
  title: string
  /** One/two-line plain-language meaning of the type */
  description: string
  /** Payload flow direction drawn between the two machines */
  arrow: string
}

/** Tunnel types offered by the builder, in a logical reading order */
const TUNNEL_TYPE_META: TunnelTypeMeta[] = [
  {
    type: TUNNEL_TYPE_LOCAL,
    badge: 'L',
    title: 'Local forward',
    description:
      'Expose a service reachable from the SSH server on a port of this PC. Apps connect to the local port; traffic rides the tunnel to the server side.',
    arrow: '→'
  },
  {
    type: TUNNEL_TYPE_REMOTE,
    badge: 'R',
    title: 'Remote forward',
    description:
      'Expose a service running on this PC through a port bound on the SSH server. Connections arriving on the server are pulled back through the tunnel.',
    arrow: '←'
  },
  {
    type: TUNNEL_TYPE_DYNAMIC,
    badge: 'D',
    title: 'Dynamic SOCKS5',
    description:
      'Start a SOCKS5 proxy on this PC. Apps pointed at the proxy can reach any host the SSH server can reach, chosen per connection.',
    arrow: '⇄'
  }
]

interface TunnelDraft {
  type: TunnelType
  listenHost: string
  listenPort: string
  destHost: string
  destPort: string
}

function parsePort(raw: string): number | null {
  if (!/^\d+$/.test(raw)) {
    return null
  }
  const value = Number(raw)
  if (value < TUNNEL_PORT_MIN || value > TUNNEL_PORT_MAX) {
    return null
  }
  return value
}

function portDigits(raw: string): string {
  return raw.replace(/[^0-9]/g, '').slice(0, PORT_INPUT_MAX_DIGITS)
}

function addressText(host: string, port: string): string {
  return `${host.trim() || 'host'}:${port || 'port'}`
}

interface DiagramChipProps {
  role: string
  host: string
  port: string
  ready: boolean
  /** Fixed label instead of a host:port pair (dynamic tunnels) */
  fixed?: string
}

function DiagramChip({ role, host, port, ready, fixed }: DiagramChipProps) {
  return (
    <span className={`tunnel-diagram-chip${ready ? ' ready' : ' pending'}`}>
      {role ? <span className="tunnel-diagram-chip-role">{role}</span> : null}
      <span className="tunnel-diagram-chip-addr">{fixed ?? addressText(host, port)}</span>
    </span>
  )
}

function TunnelDiagram({
  draft,
  sshEndpoint,
  listenReady,
  destReady
}: {
  draft: TunnelDraft
  sshEndpoint: string
  listenReady: boolean
  destReady: boolean
}) {
  const meta = TUNNEL_TYPE_META.find((m) => m.type === draft.type) ?? TUNNEL_TYPE_META[0]
  const pcChip: DiagramChipProps =
    draft.type === TUNNEL_TYPE_REMOTE
      ? { role: 'deliver to', host: draft.destHost, port: draft.destPort, ready: destReady }
      : {
          role: 'listen on',
          host: draft.listenHost,
          port: draft.listenPort,
          ready: listenReady
        }
  const serverChip: DiagramChipProps =
    draft.type === TUNNEL_TYPE_DYNAMIC
      ? { role: '', host: '', port: '', ready: true, fixed: 'any host:port' }
      : draft.type === TUNNEL_TYPE_REMOTE
        ? {
            role: 'listen on',
            host: draft.listenHost,
            port: draft.listenPort,
            ready: listenReady
          }
        : {
            role: 'connect to',
            host: draft.destHost,
            port: draft.destPort,
            ready: destReady
          }
  return (
    <div className="tunnel-diagram" data-type={draft.type}>
      <div className="tunnel-diagram-side">
        <span className="tunnel-diagram-node">This PC</span>
        <DiagramChip {...pcChip} />
      </div>
      <div className="tunnel-diagram-mid">
        <span className="tunnel-diagram-pipe">
          <span className="tunnel-diagram-arrow" aria-hidden="true">
            {meta.arrow}
          </span>
        </span>
        <span className="tunnel-diagram-mid-label">SSH tunnel</span>
        <span className="tunnel-diagram-mid-sub" title={sshEndpoint}>
          {sshEndpoint}
        </span>
      </div>
      <div className="tunnel-diagram-side">
        <span className="tunnel-diagram-node">SSH server</span>
        <DiagramChip {...serverChip} />
      </div>
    </div>
  )
}

function HostPortField({
  label,
  hint,
  host,
  port,
  onHostChange,
  onPortChange
}: {
  label: string
  hint: string
  host: string
  port: string
  onHostChange: (host: string) => void
  onPortChange: (port: string) => void
}) {
  return (
    <div className="tunnel-field">
      <span className="tunnel-field-label">{label}</span>
      <span className="tunnel-field-inputs">
        <input
          type="text"
          value={host}
          placeholder="host"
          spellCheck={false}
          aria-label={`${label} host`}
          onChange={(e) => onHostChange(e.target.value)}
        />
        <span className="tunnel-field-colon">:</span>
        <input
          className="tunnel-field-port"
          type="text"
          inputMode="numeric"
          value={port}
          placeholder="port"
          maxLength={PORT_INPUT_MAX_DIGITS}
          aria-label={`${label} port`}
          onChange={(e) => onPortChange(portDigits(e.target.value))}
        />
      </span>
      <span className="tunnel-field-hint">{hint}</span>
    </div>
  )
}

interface Props {
  /** SSH endpoint rendered on the tunnel, e.g. user@host:22 */
  sshEndpoint: string
  /** Tunnels already configured for the host, used to flag listen collisions */
  existing: SshTunnel[]
  onAdd: (tunnel: SshTunnel) => void
  onCancel: () => void
}

export default function TunnelBuilder({ sshEndpoint, existing, onAdd, onCancel }: Props) {
  const [type, setType] = useState<TunnelType>(TUNNEL_TYPE_LOCAL)
  const [listenHost, setListenHost] = useState(DEFAULT_TUNNEL_LISTEN_HOST)
  const [listenPort, setListenPort] = useState('')
  const [destHost, setDestHost] = useState('')
  const [destPort, setDestPort] = useState('')

  const isLocal = type === TUNNEL_TYPE_LOCAL
  const isRemote = type === TUNNEL_TYPE_REMOTE
  const showDest = type !== TUNNEL_TYPE_DYNAMIC
  const meta = TUNNEL_TYPE_META.find((m) => m.type === type) ?? TUNNEL_TYPE_META[0]

  const trimmedListenHost = listenHost.trim()
  const trimmedDestHost = destHost.trim()
  const listenPortValue = parsePort(listenPort)
  const destPortValue = parsePort(destPort)
  const listenReady = trimmedListenHost !== '' && listenPortValue !== null
  const destReady = trimmedDestHost !== '' && destPortValue !== null

  const listenConflict =
    listenReady &&
    existing.some(
      (t) =>
        // Local/dynamic bind on this PC; remote binds on the server — only the
        // same side can actually collide.
        (type === TUNNEL_TYPE_REMOTE
          ? t.type === TUNNEL_TYPE_REMOTE
          : t.type !== TUNNEL_TYPE_REMOTE) &&
        t.listenHost.trim().toLowerCase() === trimmedListenHost.toLowerCase() &&
        t.listenPort === listenPortValue
    )

  const errors: string[] = []
  if (listenPort === '') {
    errors.push('Enter a listen port.')
  } else if (listenPortValue === null) {
    errors.push('Listen port must be between 1 and 65535.')
  } else if (listenConflict) {
    errors.push(`${trimmedListenHost}:${listenPortValue} is already used by another tunnel.`)
  }
  if (showDest) {
    if (!trimmedDestHost) {
      errors.push('Enter a destination host.')
    } else if (destPort === '') {
      errors.push('Enter a destination port.')
    } else if (destPortValue === null) {
      errors.push('Destination port must be between 1 and 65535.')
    }
  }

  const canAdd = errors.length === 0

  const handleAdd = (): void => {
    if (!canAdd) {
      return
    }
    onAdd({
      ...emptyTunnel(),
      type,
      listenHost: trimmedListenHost,
      listenPort: listenPortValue ?? TUNNEL_PORT_MIN,
      destHost: showDest ? trimmedDestHost : '',
      destPort: showDest ? destPortValue ?? 0 : 0
    })
  }

  const summary = showDest
    ? `${addressText(listenHost, listenPort)} ${meta.arrow} ${addressText(destHost, destPort)}`
    : `SOCKS5 ${addressText(listenHost, listenPort)}`

  const draft: TunnelDraft = { type, listenHost, listenPort, destHost, destPort }

  return (
    <div className="tunnel-builder">
      <div className="tunnel-builder-head">
        <strong>Add a tunnel</strong>
        <button type="button" className="tunnel-builder-close" aria-label="Close" onClick={onCancel}>
          ✕
        </button>
      </div>

      <div className="tunnel-type-row" role="radiogroup" aria-label="Tunnel type">
        {TUNNEL_TYPE_META.map((option) => (
          <label
            key={option.type}
            className={`tunnel-type-option${option.type === type ? ' selected' : ''}`}
          >
            <input
              type="radio"
              name="tunnel-type"
              checked={option.type === type}
              onChange={() => setType(option.type)}
            />
            <span className="tunnel-type-option-top">
              <span className={`tunnel-badge ${option.badge}`}>{option.badge}</span>
              <strong>{option.title}</strong>
            </span>
            <small>{option.description}</small>
          </label>
        ))}
      </div>

      <div className="tunnel-builder-body">
        <div className="tunnel-fields">
          {isLocal ? (
            <>
              <HostPortField
                label="Listen on this PC"
                hint="Opens this local port; apps connect here and their traffic rides the tunnel."
                host={listenHost}
                port={listenPort}
                onHostChange={setListenHost}
                onPortChange={setListenPort}
              />
              <HostPortField
                label="Forward to (server side)"
                hint="Host and port reachable from the SSH server — another machine on its network, a service, …"
                host={destHost}
                port={destPort}
                onHostChange={setDestHost}
                onPortChange={setDestPort}
              />
            </>
          ) : isRemote ? (
            <>
              <HostPortField
                label="Server listens on"
                hint="Port bound on the SSH server. Keep 127.0.0.1 for a private bind or use 0.0.0.0 to expose it on the server's network."
                host={listenHost}
                port={listenPort}
                onHostChange={setListenHost}
                onPortChange={setListenPort}
              />
              <HostPortField
                label="Deliver to (this PC)"
                hint="Local service that receives the connections, e.g. 127.0.0.1:3000."
                host={destHost}
                port={destPort}
                onHostChange={setDestHost}
                onPortChange={setDestPort}
              />
            </>
          ) : (
            <HostPortField
              label="SOCKS5 proxy on this PC"
              hint="Point any app (browser, git, …) at this proxy; each connection picks its destination on the server side."
              host={listenHost}
              port={listenPort}
              onHostChange={setListenHost}
              onPortChange={setListenPort}
            />
          )}
        </div>

        <TunnelDiagram
          draft={draft}
          sshEndpoint={sshEndpoint}
          listenReady={listenReady}
          destReady={destReady}
        />
      </div>

      <div className="tunnel-builder-footer">
        <div className="tunnel-builder-note">
          <span className="tunnel-builder-summary">
            <span className={`tunnel-badge ${meta.badge}`}>{meta.badge}</span>
            <span>{summary}</span>
          </span>
          {errors.map((error) => (
            <span key={error} className="tunnel-builder-error">
              {error}
            </span>
          ))}
        </div>
        <div className="tunnel-builder-actions">
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
          <button type="button" className="primary" disabled={!canAdd} onClick={handleAdd}>
            Add tunnel
          </button>
        </div>
      </div>
    </div>
  )
}

