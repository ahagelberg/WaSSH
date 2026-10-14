import { useCallback, useEffect, useState, type ReactElement } from 'react'
import { PluginButton } from '@plugin-api/renderer'
import {
  isDockerMainMessage,
  type DockerImageView,
  type DockerNetworkView,
  type DockerPullProgress,
  type DockerVolumeView
} from './protocol'
import { formatBytes, formatEpochMs } from './viewUtils'

type Notify = (notice: { message: string; kind: 'info' | 'error' }) => void

interface PanelProps {
  tabId: string
  pluginId: string
  ready: boolean
  notify: Notify
  confirmDestructive: boolean
}

function usePluginChannel(
  tabId: string,
  pluginId: string
): {
  send: (payload: Parameters<typeof window.wassh.sendPluginMessage>[2]) => Promise<unknown>
  subscribe: (handler: (payload: unknown) => void) => () => void
} {
  const send = useCallback(
    (payload: Parameters<typeof window.wassh.sendPluginMessage>[2]): Promise<unknown> =>
      window.wassh.sendPluginMessage(tabId, pluginId, payload),
    [tabId, pluginId]
  )
  const subscribe = useCallback(
    (handler: (payload: unknown) => void): (() => void) =>
      window.wassh.onPluginMessage((event) => {
        if (event.tabId !== tabId || event.pluginId !== pluginId) {
          return
        }
        handler(event.payload)
      }),
    [tabId, pluginId]
  )
  return { send, subscribe }
}

/** Small reusable confirmation used by the resource panels. */
function PanelConfirm({
  title,
  message,
  confirmLabel,
  onCancel,
  onConfirm
}: {
  title: string
  message: string
  confirmLabel: string
  onCancel: () => void
  onConfirm: () => void
}): ReactElement {
  return (
    <div className="plugin-ui-modal-backdrop" role="presentation" onClick={onCancel}>
      <div
        className="plugin-ui-modal docker-modal"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onClick={(event) => event.stopPropagation()}
      >
        <h2 className="plugin-ui-heading">{title}</h2>
        <p>{message}</p>
        <div className="plugin-ui-actions docker-modal-actions">
          <PluginButton compact onClick={onCancel}>
            Cancel
          </PluginButton>
          <PluginButton variant="danger" compact onClick={onConfirm}>
            {confirmLabel}
          </PluginButton>
        </div>
      </div>
    </div>
  )
}

function resultError(result: unknown): string | null {
  const typed = result as { ok?: boolean; error?: string } | undefined
  if (typed && typed.ok === false) {
    return typed.error ?? 'Operation failed'
  }
  return null
}

/* ------------------------------------------------------------------- images */

export function ImagesPanel({
  tabId,
  pluginId,
  ready,
  notify,
  confirmDestructive
}: PanelProps): ReactElement {
  const { send, subscribe } = usePluginChannel(tabId, pluginId)
  const [images, setImages] = useState<DockerImageView[]>([])
  const [pullRef, setPullRef] = useState('')
  const [progress, setProgress] = useState<DockerPullProgress | null>(null)
  const [pulling, setPulling] = useState(false)
  const [confirmId, setConfirmId] = useState<string | null>(null)

  useEffect(() => {
    if (!ready) {
      return
    }
    void send({ type: 'listImages' })
  }, [ready, send])

  useEffect(() => {
    return subscribe((payload) => {
      if (!isDockerMainMessage(payload)) {
        return
      }
      if (payload.type === 'images') {
        setImages(payload.images)
        return
      }
      if (payload.type === 'pull') {
        setProgress(payload.progress)
        if (payload.progress.done) {
          setPulling(false)
          notify({ message: `Pulled ${payload.progress.image}`, kind: 'info' })
        }
      }
    })
  }, [subscribe, notify])

  const pull = useCallback(async (): Promise<void> => {
    const reference = pullRef.trim()
    if (!reference) {
      return
    }
    setPulling(true)
    setProgress(null)
    const error = resultError(await send({ type: 'pullImage', image: reference }))
    setPulling(false)
    if (error) {
      notify({ message: error, kind: 'error' })
    }
  }, [notify, pullRef, send])

  const remove = useCallback(
    async (id: string, force: boolean): Promise<void> => {
      const error = resultError(await send({ type: 'removeImage', id, force }))
      if (error) {
        notify({ message: error, kind: 'error' })
      }
    },
    [notify, send]
  )

  if (!ready) {
    return <div className="plugin-ui-empty">Docker is not available</div>
  }

  return (
    <div className="docker-panel">
      <div className="plugin-ui-toolbar">
        <input
          className="plugin-ui-input docker-filter"
          value={pullRef}
          placeholder="nginx:latest"
          aria-label="Image to pull"
          onChange={(event) => setPullRef(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              void pull()
            }
          }}
        />
        <PluginButton variant="primary" compact disabled={!pullRef.trim() || pulling} onClick={() => void pull()}>
          {pulling ? 'Pulling…' : 'Pull'}
        </PluginButton>
        <div className="plugin-ui-spacer" />
        <PluginButton compact onClick={() => void send({ type: 'listImages' })}>
          Refresh
        </PluginButton>
      </div>

      {progress && !progress.done ? (
        <div className="plugin-ui-progress docker-pull">
          <span className="docker-pull-status">{progress.status || 'Pulling…'}</span>
          {progress.current !== null && progress.total ? (
            <>
              <progress value={progress.current} max={progress.total} />
              <span className="plugin-ui-muted">
                {formatBytes(progress.current)} / {formatBytes(progress.total)}
              </span>
            </>
          ) : null}
        </div>
      ) : null}

      {images.length === 0 ? (
        <div className="plugin-ui-empty">No images</div>
      ) : (
        <div className="plugin-ui-table-wrap">
          <table className="plugin-ui-table">
            <thead>
              <tr>
                <th>Repository</th>
                <th>Tag</th>
                <th>Image ID</th>
                <th>Created</th>
                <th>Size</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {images.map((image) => {
                const reference = image.tags[0] ?? image.id
                return (
                  <tr key={image.id}>
                    <td className="docker-mono">{reference.split(':')[0]}</td>
                    <td className="docker-mono">{image.tags.length > 1 ? `${image.tags.length} tags` : reference.split(':')[1] ?? '—'}</td>
                    <td className="docker-mono plugin-ui-muted">{image.shortId}</td>
                    <td className="plugin-ui-muted">{formatEpochMs(image.created)}</td>
                    <td className="plugin-ui-muted">{formatBytes(image.size)}</td>
                    <td>
                      <PluginButton
                        variant="danger"
                        compact
                        onClick={() => {
                          if (confirmDestructive) {
                            setConfirmId(reference)
                          } else {
                            void remove(reference, false)
                          }
                        }}
                      >
                        Delete
                      </PluginButton>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}

      {confirmId ? (
        <PanelConfirm
          title="Delete image"
          message={`Delete image "${confirmId}"? This cannot be undone.`}
          confirmLabel="Delete"
          onCancel={() => setConfirmId(null)}
          onConfirm={() => {
            const id = confirmId
            setConfirmId(null)
            void remove(id, false)
          }}
        />
      ) : null}
    </div>
  )
}

/* ------------------------------------------------------------------ volumes */

export function VolumesPanel({
  tabId,
  pluginId,
  ready,
  notify,
  confirmDestructive
}: PanelProps): ReactElement {
  const { send, subscribe } = usePluginChannel(tabId, pluginId)
  const [volumes, setVolumes] = useState<DockerVolumeView[]>([])
  const [name, setName] = useState('')
  const [driver, setDriver] = useState('')
  const [confirmName, setConfirmName] = useState<string | null>(null)
  const [confirmPrune, setConfirmPrune] = useState(false)

  useEffect(() => {
    if (!ready) {
      return
    }
    void send({ type: 'listVolumes' })
  }, [ready, send])

  useEffect(() => {
    return subscribe((payload) => {
      if (isDockerMainMessage(payload) && payload.type === 'volumes') {
        setVolumes(payload.volumes)
      }
    })
  }, [subscribe])

  const create = useCallback(async (): Promise<void> => {
    const error = resultError(
      await send({ type: 'createVolume', spec: { name: name.trim(), driver: driver.trim(), labels: [] } })
    )
    if (error) {
      notify({ message: error, kind: 'error' })
      return
    }
    setName('')
    notify({ message: `Created volume ${name.trim() || '(auto)'}`, kind: 'info' })
  }, [driver, name, notify, send])

  const remove = useCallback(
    async (volumeName: string): Promise<void> => {
      const error = resultError(await send({ type: 'removeVolume', name: volumeName, force: false }))
      if (error) {
        notify({ message: error, kind: 'error' })
      }
    },
    [notify, send]
  )

  const prune = useCallback(async (): Promise<void> => {
    const result = (await send({ type: 'pruneVolumes' })) as
      | { ok?: boolean; reclaimed?: number; error?: string }
      | undefined
    if (result && result.ok === false) {
      notify({ message: result.error ?? 'Prune failed', kind: 'error' })
      return
    }
    notify({ message: `Reclaimed ${formatBytes(result?.reclaimed ?? 0)}`, kind: 'info' })
  }, [notify, send])

  if (!ready) {
    return <div className="plugin-ui-empty">Docker is not available</div>
  }

  return (
    <div className="docker-panel">
      <div className="plugin-ui-toolbar">
        <input
          className="plugin-ui-input docker-filter"
          value={name}
          placeholder="volume name"
          aria-label="Volume name"
          onChange={(event) => setName(event.target.value)}
        />
        <input
          className="plugin-ui-input docker-filter"
          value={driver}
          placeholder="driver (local)"
          aria-label="Volume driver"
          onChange={(event) => setDriver(event.target.value)}
        />
        <PluginButton variant="primary" compact onClick={() => void create()}>
          Create
        </PluginButton>
        <div className="plugin-ui-spacer" />
        <PluginButton
          variant="danger"
          compact
          onClick={() => (confirmDestructive ? setConfirmPrune(true) : void prune())}
        >
          Prune unused
        </PluginButton>
        <PluginButton compact onClick={() => void send({ type: 'listVolumes' })}>
          Refresh
        </PluginButton>
      </div>

      {volumes.length === 0 ? (
        <div className="plugin-ui-empty">No volumes</div>
      ) : (
        <div className="plugin-ui-table-wrap">
          <table className="plugin-ui-table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Driver</th>
                <th>Scope</th>
                <th>Size</th>
                <th>In use</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {volumes.map((volume) => (
                <tr key={volume.name}>
                  <td className="docker-mono">{volume.name}</td>
                  <td className="plugin-ui-muted">{volume.driver}</td>
                  <td className="plugin-ui-muted">{volume.scope}</td>
                  <td className="plugin-ui-muted">{volume.size === null ? '—' : formatBytes(volume.size)}</td>
                  <td className="plugin-ui-muted">{volume.refCount ?? '—'}</td>
                  <td>
                    <PluginButton
                      variant="danger"
                      compact
                      onClick={() => {
                        if (confirmDestructive) {
                          setConfirmName(volume.name)
                        } else {
                          void remove(volume.name)
                        }
                      }}
                    >
                      Delete
                    </PluginButton>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {confirmName ? (
        <PanelConfirm
          title="Delete volume"
          message={`Delete volume "${confirmName}"? Its data is lost and this cannot be undone.`}
          confirmLabel="Delete"
          onCancel={() => setConfirmName(null)}
          onConfirm={() => {
            const target = confirmName
            setConfirmName(null)
            void remove(target)
          }}
        />
      ) : null}

      {confirmPrune ? (
        <PanelConfirm
          title="Prune volumes"
          message="Delete every volume that is not used by a container? This cannot be undone."
          confirmLabel="Prune"
          onCancel={() => setConfirmPrune(false)}
          onConfirm={() => {
            setConfirmPrune(false)
            void prune()
          }}
        />
      ) : null}
    </div>
  )
}

/* ----------------------------------------------------------------- networks */

export function NetworksPanel({
  tabId,
  pluginId,
  ready,
  notify,
  confirmDestructive
}: PanelProps): ReactElement {
  const { send, subscribe } = usePluginChannel(tabId, pluginId)
  const [networks, setNetworks] = useState<DockerNetworkView[]>([])
  const [name, setName] = useState('')
  const [driver, setDriver] = useState('bridge')
  const [subnet, setSubnet] = useState('')
  const [gateway, setGateway] = useState('')
  const [internal, setInternal] = useState(false)
  const [confirmId, setConfirmId] = useState<string | null>(null)

  useEffect(() => {
    if (!ready) {
      return
    }
    void send({ type: 'listNetworks' })
  }, [ready, send])

  useEffect(() => {
    return subscribe((payload) => {
      if (isDockerMainMessage(payload) && payload.type === 'networks') {
        setNetworks(payload.networks)
      }
    })
  }, [subscribe])

  const create = useCallback(async (): Promise<void> => {
    if (!name.trim()) {
      return
    }
    const error = resultError(
      await send({
        type: 'createNetwork',
        spec: { name: name.trim(), driver, subnet: subnet.trim(), gateway: gateway.trim(), internal }
      })
    )
    if (error) {
      notify({ message: error, kind: 'error' })
      return
    }
    notify({ message: `Created network ${name.trim()}`, kind: 'info' })
    setName('')
    setSubnet('')
    setGateway('')
  }, [driver, gateway, internal, name, notify, send, subnet])

  const remove = useCallback(
    async (id: string): Promise<void> => {
      const error = resultError(await send({ type: 'removeNetwork', id }))
      if (error) {
        notify({ message: error, kind: 'error' })
      }
    },
    [notify, send]
  )

  if (!ready) {
    return <div className="plugin-ui-empty">Docker is not available</div>
  }

  return (
    <div className="docker-panel">
      <div className="plugin-ui-toolbar docker-toolbar-wrap">
        <input
          className="plugin-ui-input docker-filter"
          value={name}
          placeholder="network name"
          aria-label="Network name"
          onChange={(event) => setName(event.target.value)}
        />
        <select
          className="plugin-ui-select"
          value={driver}
          aria-label="Network driver"
          onChange={(event) => setDriver(event.target.value)}
        >
          <option value="bridge">bridge</option>
          <option value="overlay">overlay</option>
          <option value="macvlan">macvlan</option>
          <option value="none">none</option>
        </select>
        <input
          className="plugin-ui-input docker-filter"
          value={subnet}
          placeholder="subnet (optional)"
          aria-label="Subnet"
          onChange={(event) => setSubnet(event.target.value)}
        />
        <input
          className="plugin-ui-input docker-filter"
          value={gateway}
          placeholder="gateway (optional)"
          aria-label="Gateway"
          onChange={(event) => setGateway(event.target.value)}
        />
        <label className="docker-checkbox">
          <input type="checkbox" checked={internal} onChange={(event) => setInternal(event.target.checked)} />
          <span>Internal</span>
        </label>
        <PluginButton variant="primary" compact disabled={!name.trim()} onClick={() => void create()}>
          Create
        </PluginButton>
        <div className="plugin-ui-spacer" />
        <PluginButton compact onClick={() => void send({ type: 'listNetworks' })}>
          Refresh
        </PluginButton>
      </div>

      {networks.length === 0 ? (
        <div className="plugin-ui-empty">No networks</div>
      ) : (
        <div className="plugin-ui-table-wrap">
          <table className="plugin-ui-table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Driver</th>
                <th>Scope</th>
                <th>Subnet</th>
                <th>Containers</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {networks.map((network) => (
                <tr key={network.id}>
                  <td className="docker-mono">{network.name}</td>
                  <td className="plugin-ui-muted">
                    {network.driver}
                    {network.internal ? ' (internal)' : ''}
                  </td>
                  <td className="plugin-ui-muted">{network.scope}</td>
                  <td className="docker-mono plugin-ui-muted">{network.subnet || '—'}</td>
                  <td className="plugin-ui-muted">
                    {network.containers.length === 0
                      ? '—'
                      : network.containers.map((container) => container.name).join(', ')}
                  </td>
                  <td>
                    <PluginButton
                      variant="danger"
                      compact
                      onClick={() => {
                        if (confirmDestructive) {
                          setConfirmId(network.id)
                        } else {
                          void remove(network.id)
                        }
                      }}
                    >
                      Delete
                    </PluginButton>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {confirmId ? (
        <PanelConfirm
          title="Delete network"
          message="Delete this network? Connected containers lose connectivity on it."
          confirmLabel="Delete"
          onCancel={() => setConfirmId(null)}
          onConfirm={() => {
            const id = confirmId
            setConfirmId(null)
            void remove(id)
          }}
        />
      ) : null}
    </div>
  )
}
