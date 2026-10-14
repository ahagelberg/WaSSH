import { useCallback, useEffect, useMemo, useState, type ReactElement } from 'react'
import { PluginButton, PluginField } from '@plugin-api/renderer'
import {
  isDockerMainMessage,
  type DockerEnvSpec,
  type DockerNetworkView,
  type DockerPortSpec,
  type DockerRunRequest,
  type DockerVolumeSpec
} from './protocol'
import { jsonParseError } from './viewUtils'

interface Props {
  tabId: string
  pluginId: string
  initialImage?: string
  onClose: () => void
  onCreated: (message: string) => void
}

const RESTART_POLICIES = ['no', 'on-failure', 'always', 'unless-stopped']

const EMPTY_PORT: DockerPortSpec = { hostPort: '', containerPort: '', protocol: 'tcp' }
const EMPTY_ENV: DockerEnvSpec = { key: '', value: '' }
const EMPTY_VOLUME: DockerVolumeSpec = { bind: true, source: '', destination: '', readOnly: false }

export default function CreateContainerDialog({
  tabId,
  pluginId,
  initialImage = '',
  onClose,
  onCreated
}: Props): ReactElement {
  const [image, setImage] = useState(initialImage)
  const [name, setName] = useState('')
  const [command, setCommand] = useState('')
  const [entrypoint, setEntrypoint] = useState('')
  const [detach, setDetach] = useState(true)
  const [autoRemove, setAutoRemove] = useState(false)
  const [restartPolicy, setRestartPolicy] = useState('no')
  const [network, setNetwork] = useState('')
  const [ports, setPorts] = useState<DockerPortSpec[]>([{ ...EMPTY_PORT }])
  const [env, setEnv] = useState<DockerEnvSpec[]>([{ ...EMPTY_ENV }])
  const [volumes, setVolumes] = useState<DockerVolumeSpec[]>([{ ...EMPTY_VOLUME }])
  const [labels, setLabels] = useState<DockerEnvSpec[]>([{ ...EMPTY_ENV }])
  const [advancedJson, setAdvancedJson] = useState('')
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [images, setImages] = useState<string[]>([])
  const [networks, setNetworks] = useState<DockerNetworkView[]>([])
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const send = useCallback(
    (payload: Parameters<typeof window.wassh.sendPluginMessage>[2]): Promise<unknown> =>
      window.wassh.sendPluginMessage(tabId, pluginId, payload),
    [tabId, pluginId]
  )

  useEffect(() => {
    void send({ type: 'listImages' })
    void send({ type: 'listNetworks' })
  }, [send])

  useEffect(() => {
    return window.wassh.onPluginMessage((event) => {
      if (event.tabId !== tabId || event.pluginId !== pluginId) {
        return
      }
      if (!isDockerMainMessage(event.payload)) {
        return
      }
      if (event.payload.type === 'images') {
        const tags = event.payload.images.flatMap((entry) => entry.tags)
        setImages(Array.from(new Set(tags)).sort())
        return
      }
      if (event.payload.type === 'networks') {
        setNetworks(event.payload.networks)
      }
    })
  }, [tabId, pluginId])

  const advancedError = useMemo(() => jsonParseError(advancedJson), [advancedJson])
  const canSubmit = image.trim().length > 0 && !advancedError && !submitting

  const submit = useCallback(async (): Promise<void> => {
    const request: DockerRunRequest = {
      image: image.trim(),
      tag: 'latest',
      name: name.trim(),
      command,
      entrypoint,
      detach,
      autoRemove,
      restartPolicy,
      network,
      ports: ports.filter((port) => port.containerPort.trim()),
      env: env.filter((entry) => entry.key.trim()),
      volumes: volumes.filter((volume) => volume.source.trim() && volume.destination.trim()),
      labels: labels.filter((entry) => entry.key.trim()),
      advancedJson
    }

    setSubmitting(true)
    setError(null)
    try {
      const result = (await send({ type: 'runContainer', request })) as
        | { ok?: boolean; error?: string; id?: string }
        | undefined
      if (result && result.ok === false) {
        setError(result.error ?? 'Could not create the container')
        return
      }
      onCreated(`Started ${request.name || request.image}`)
    } finally {
      setSubmitting(false)
    }
  }, [advancedJson, autoRemove, command, detach, entrypoint, env, image, labels, name, network, onCreated, ports, restartPolicy, send, volumes])

  return (
    <div className="plugin-ui-modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className="plugin-ui-modal docker-modal docker-create"
        role="dialog"
        aria-modal="true"
        aria-label="New container"
        onClick={(event) => event.stopPropagation()}
      >
        <h2 className="plugin-ui-heading">New container</h2>

        <div className="docker-create-body">
          <PluginField label="Image" hint="Pulled automatically when it is not present locally.">
            <input
              className="plugin-ui-input"
              list="docker-image-tags"
              value={image}
              placeholder="nginx:latest"
              autoFocus
              onChange={(event) => setImage(event.target.value)}
            />
            <datalist id="docker-image-tags">
              {images.map((tag) => (
                <option key={tag} value={tag} />
              ))}
            </datalist>
          </PluginField>

          <div className="docker-grid-2">
            <PluginField label="Name" hint="Optional container name.">
              <input
                className="plugin-ui-input"
                value={name}
                placeholder="my-nginx"
                onChange={(event) => setName(event.target.value)}
              />
            </PluginField>
            <PluginField label="Restart policy">
              <select
                className="plugin-ui-select"
                value={restartPolicy}
                onChange={(event) => setRestartPolicy(event.target.value)}
              >
                {RESTART_POLICIES.map((policy) => (
                  <option key={policy} value={policy}>
                    {policy}
                  </option>
                ))}
              </select>
            </PluginField>
          </div>

          <div className="docker-grid-2">
            <PluginField label="Network" hint="Leave empty for the default bridge network.">
              <input
                className="plugin-ui-input"
                list="docker-networks"
                value={network}
                placeholder="bridge"
                onChange={(event) => setNetwork(event.target.value)}
              />
              <datalist id="docker-networks">
                {networks.map((entry) => (
                  <option key={entry.id} value={entry.name} />
                ))}
              </datalist>
            </PluginField>
            <PluginField label="Command" hint="Overrides the image CMD.">
              <input
                className="plugin-ui-input"
                value={command}
                placeholder="nginx -g 'daemon off;'"
                onChange={(event) => setCommand(event.target.value)}
              />
            </PluginField>
          </div>

          <PluginField label="Entrypoint" hint="Overrides the image ENTRYPOINT.">
            <input
              className="plugin-ui-input"
              value={entrypoint}
              onChange={(event) => setEntrypoint(event.target.value)}
            />
          </PluginField>

          <div className="docker-flags">
            <label className="docker-checkbox">
              <input type="checkbox" checked={detach} onChange={(event) => setDetach(event.target.checked)} />
              <span>Start detached</span>
            </label>
            <label className="docker-checkbox">
              <input
                type="checkbox"
                checked={autoRemove}
                onChange={(event) => setAutoRemove(event.target.checked)}
              />
              <span>Auto-remove when stopped</span>
            </label>
          </div>

          <RowEditor
            title="Ports"
            hint="Host port → container port. Leave the host port empty to publish on a random port."
            rows={ports}
            onChange={setPorts}
            blank={{ ...EMPTY_PORT }}
            render={(row, update) => (
              <>
                <input
                  className="plugin-ui-input docker-cell"
                  value={row.hostPort}
                  placeholder="8080"
                  aria-label="Host port"
                  onChange={(event) => update({ ...row, hostPort: event.target.value })}
                />
                <input
                  className="plugin-ui-input docker-cell"
                  value={row.containerPort}
                  placeholder="80"
                  aria-label="Container port"
                  onChange={(event) => update({ ...row, containerPort: event.target.value })}
                />
                <select
                  className="plugin-ui-select docker-cell"
                  value={row.protocol}
                  aria-label="Protocol"
                  onChange={(event) => update({ ...row, protocol: event.target.value })}
                >
                  <option value="tcp">tcp</option>
                  <option value="udp">udp</option>
                </select>
              </>
            )}
          />

          <RowEditor
            title="Environment"
            hint="Variables passed to the container."
            rows={env}
            onChange={setEnv}
            blank={{ ...EMPTY_ENV }}
            render={(row, update) => (
              <>
                <input
                  className="plugin-ui-input docker-cell"
                  value={row.key}
                  placeholder="KEY"
                  aria-label="Variable name"
                  onChange={(event) => update({ ...row, key: event.target.value })}
                />
                <input
                  className="plugin-ui-input docker-cell docker-cell-wide"
                  value={row.value}
                  placeholder="value"
                  aria-label="Variable value"
                  onChange={(event) => update({ ...row, value: event.target.value })}
                />
              </>
            )}
          />

          <RowEditor
            title="Volumes"
            hint="Bind a host path or mount a named volume."
            rows={volumes}
            onChange={setVolumes}
            blank={{ ...EMPTY_VOLUME }}
            render={(row, update) => (
              <>
                <select
                  className="plugin-ui-select docker-cell"
                  value={row.bind ? 'bind' : 'volume'}
                  aria-label="Mount type"
                  onChange={(event) => update({ ...row, bind: event.target.value === 'bind' })}
                >
                  <option value="bind">Bind</option>
                  <option value="volume">Volume</option>
                </select>
                <input
                  className="plugin-ui-input docker-cell"
                  value={row.source}
                  placeholder={row.bind ? '/host/path' : 'volume-name'}
                  aria-label="Source"
                  onChange={(event) => update({ ...row, source: event.target.value })}
                />
                <input
                  className="plugin-ui-input docker-cell"
                  value={row.destination}
                  placeholder="/container/path"
                  aria-label="Destination"
                  onChange={(event) => update({ ...row, destination: event.target.value })}
                />
                <label className="docker-checkbox docker-cell">
                  <input
                    type="checkbox"
                    checked={row.readOnly}
                    onChange={(event) => update({ ...row, readOnly: event.target.checked })}
                  />
                  <span>ro</span>
                </label>
              </>
            )}
          />

          <RowEditor
            title="Labels"
            rows={labels}
            onChange={setLabels}
            blank={{ ...EMPTY_ENV }}
            render={(row, update) => (
              <>
                <input
                  className="plugin-ui-input docker-cell"
                  value={row.key}
                  placeholder="key"
                  aria-label="Label key"
                  onChange={(event) => update({ ...row, key: event.target.value })}
                />
                <input
                  className="plugin-ui-input docker-cell docker-cell-wide"
                  value={row.value}
                  placeholder="value"
                  aria-label="Label value"
                  onChange={(event) => update({ ...row, value: event.target.value })}
                />
              </>
            )}
          />

          <details className="docker-raw" open={showAdvanced} onToggle={(event) => setShowAdvanced((event.target as HTMLDetailsElement).open)}>
            <summary>Advanced JSON</summary>
            <PluginField
              label="HostConfig overrides"
              hint="Merged over the generated HostConfig. Pass a full HostConfig object, or an object with a HostConfig key."
            >
              <textarea
                className="plugin-ui-textarea docker-json-input"
                rows={6}
                value={advancedJson}
                spellCheck={false}
                placeholder={'{\n  "CapAdd": ["NET_ADMIN"],\n  "Privileged": false\n}'}
                onChange={(event) => setAdvancedJson(event.target.value)}
              />
            </PluginField>
            {advancedError ? <div className="plugin-ui-error">{advancedError}</div> : null}
          </details>

          {error ? <div className="plugin-ui-error">{error}</div> : null}
        </div>

        <div className="plugin-ui-actions docker-modal-actions">
          <PluginButton compact onClick={onClose} disabled={submitting}>
            Cancel
          </PluginButton>
          <PluginButton
            variant="primary"
            compact
            disabled={!canSubmit}
            onClick={() => void submit()}
          >
            {submitting ? 'Creating…' : 'Pull & run'}
          </PluginButton>
        </div>
      </div>
    </div>
  )
}

/** Repeatable list of structured rows with add/remove controls. */
function RowEditor<T>({
  title,
  hint,
  rows,
  onChange,
  blank,
  render
}: {
  title: string
  hint?: string
  rows: T[]
  onChange: (rows: T[]) => void
  blank: T
  render: (row: T, update: (next: T) => void) => ReactElement
}): ReactElement {
  return (
    <div className="docker-row-editor">
      <div className="docker-row-editor-head">
        <span className="plugin-ui-label">{title}</span>
        <div className="plugin-ui-spacer" />
        <PluginButton compact onClick={() => onChange([...rows, { ...blank }])}>
          Add
        </PluginButton>
      </div>
      {hint ? <span className="plugin-ui-hint">{hint}</span> : null}
      {rows.map((row, index) => (
        <div className="docker-row-editor-row" key={index}>
          {render(row, (next) => onChange(rows.map((candidate, at) => (at === index ? next : candidate))))}
          <PluginButton
            variant="danger"
            compact
            aria-label={`Remove ${title} row`}
            disabled={rows.length === 1}
            onClick={() => onChange(rows.filter((_candidate, at) => at !== index))}
          >
            ×
          </PluginButton>
        </div>
      ))}
    </div>
  )
}
