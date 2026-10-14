import { useCallback, useEffect, useMemo, useState, type ReactElement } from 'react'
import { PluginButton } from '@plugin-api/renderer'
import type { PluginViewProps } from '@plugin-api/renderer'
import {
  EMPTY_UPDATE_STATUS,
  isDockerMainMessage,
  isDestructiveContainerAction,
  type DockerContainerAction,
  type DockerContainerView,
  type DockerEngineStatus,
  type DockerMainMessage,
  type DockerUpdateEvent,
  type DockerUpdateStatus
} from './protocol'
import { containerLabel, formatPorts, sortContainers, stateLabel } from './viewUtils'
import ContainerDetail from './ContainerDetail'
import ContainerShell from './ContainerShell'
import CreateContainerDialog from './CreateContainerDialog'
import EngineDialog from './EngineDialog'
import { ImagesPanel, NetworksPanel, VolumesPanel } from './ResourcePanels'
import './styles.css'

type ResourceTab = 'containers' | 'images' | 'volumes' | 'networks'

const TABS: Array<{ id: ResourceTab; label: string }> = [
  { id: 'containers', label: 'Containers' },
  { id: 'images', label: 'Images' },
  { id: 'volumes', label: 'Volumes' },
  { id: 'networks', label: 'Networks' }
]

/** Actions offered directly on a container row. */
const ROW_ACTIONS: Array<{ action: DockerContainerAction; label: string; title: string }> = [
  { action: 'start', label: 'Start', title: 'Start container' },
  { action: 'stop', label: 'Stop', title: 'Stop container' },
  { action: 'restart', label: 'Restart', title: 'Restart container' },
  { action: 'pause', label: 'Pause', title: 'Pause container' },
  { action: 'unpause', label: 'Resume', title: 'Resume container' },
  { action: 'kill', label: 'Kill', title: 'Kill container' },
  { action: 'remove', label: 'Remove', title: 'Remove container' }
]

/** Which actions make sense for a container's current state. */
function applicableActions(container: DockerContainerView): DockerContainerAction[] {
  switch (container.state) {
    case 'running':
      return ['stop', 'restart', 'pause', 'kill', 'remove']
    case 'paused':
      return ['unpause', 'stop', 'kill', 'remove']
    case 'restarting':
      return ['stop', 'kill', 'remove']
    default:
      return ['start', 'remove']
  }
}

interface PendingAction {
  container: DockerContainerView
  action: DockerContainerAction
}

export default function DockerManagerView({
  tabId,
  pluginId,
  active,
  settings,
  onSettingsPatch
}: PluginViewProps): ReactElement {
  const [engine, setEngine] = useState<DockerEngineStatus | null>(null)
  const [containers, setContainers] = useState<DockerContainerView[]>([])
  const [lastUpdated, setLastUpdated] = useState(0)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<{ message: string; kind: 'info' | 'error' } | null>(null)
  const [tab, setTab] = useState<ResourceTab>('containers')
  const [filter, setFilter] = useState('')
  const [detail, setDetail] = useState<DockerContainerView | null>(null)
  const [shell, setShell] = useState<DockerContainerView | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [createImage, setCreateImage] = useState('')
  const [updates, setUpdates] = useState<DockerUpdateStatus>(EMPTY_UPDATE_STATUS)
  const [updateEvents, setUpdateEvents] = useState<Record<string, DockerUpdateEvent>>({})
  const [engineOpen, setEngineOpen] = useState(false)
  const [pending, setPending] = useState<PendingAction | null>(null)
  const [installLines, setInstallLines] = useState<string[]>([])
  const [installing, setInstalling] = useState(false)

  const confirmDestructive = settings.confirmDestructive !== false
  const showStopped = settings.showStopped === true

  const send = useCallback(
    (payload: Parameters<typeof window.wassh.sendPluginMessage>[2]): Promise<unknown> =>
      window.wassh.sendPluginMessage(tabId, pluginId, payload),
    [tabId, pluginId]
  )

  /* ------------------------------------------------------------ messaging */

  useEffect(() => {
    return window.wassh.onPluginMessage((event) => {
      if (event.tabId !== tabId || event.pluginId !== pluginId) {
        return
      }
      if (!isDockerMainMessage(event.payload)) {
        return
      }
      const message: DockerMainMessage = event.payload
      switch (message.type) {
        case 'engine':
          setEngine(message.status)
          break
        case 'containers':
          setContainers(message.containers)
          setLastUpdated(message.at)
          break
        case 'updates':
          setUpdates(message.status)
          break
        case 'update':
          setUpdateEvents((previous) => ({ ...previous, [message.event.id]: message.event }))
          if (message.event.phase === 'failed') {
            setNotice({ message: `Update failed for ${message.event.name}: ${message.event.message ?? ''}`, kind: 'error' })
          }
          if (message.event.phase === 'done') {
            setNotice({ message: `Updated ${message.event.name}`, kind: 'info' })
          }
          break
        case 'notice':
          setNotice({ message: message.message, kind: message.kind })
          break
        case 'busy':
          setBusy(message.busy)
          break
        case 'engineSetup':
          setInstalling(!message.done)
          if (message.lines.length > 0) {
            setInstallLines((prev) => [...prev.slice(-200), ...message.lines])
          }
          if (message.done && message.error) {
            setNotice({ message: message.error, kind: 'error' })
          }
          if (message.done && !message.error) {
            setNotice({ message: 'Docker engine installed', kind: 'info' })
          }
          break
        default:
          break
      }
    })
  }, [tabId, pluginId])

  useEffect(() => {
    if (!notice) {
      return
    }
    const timer = setTimeout(() => setNotice(null), 6000)
    return () => clearTimeout(timer)
  }, [notice])

  // Polling lives in the main process; the view only says whether it is visible.
  useEffect(() => {
    void send({ type: 'setActive', active })
  }, [active, send])

  useEffect(() => {
    if (!active) {
      return
    }
    void send({ type: 'probe' })
  }, [active, send])

  /* --------------------------------------------------------------- actions */

  const runAction = useCallback(
    async (container: DockerContainerView, action: DockerContainerAction): Promise<void> => {
      setBusy(true)
      try {
        const result = (await send({ type: 'containerAction', id: container.id, action })) as
          | { ok?: boolean; error?: string }
          | undefined
        if (result && result.ok === false) {
          setNotice({ message: result.error ?? 'Action failed', kind: 'error' })
        }
      } finally {
        setBusy(false)
      }
    },
    [send]
  )

  const requestAction = useCallback(
    (container: DockerContainerView, action: DockerContainerAction): void => {
      if (isDestructiveContainerAction(action) && confirmDestructive) {
        setPending({ container, action })
        return
      }
      void runAction(container, action)
    },
    [confirmDestructive, runAction]
  )

  const checkUpdates = useCallback((): void => {
    void send({ type: 'checkUpdates' })
  }, [send])

  const updateContainer = useCallback(
    async (container: DockerContainerView): Promise<void> => {
      setBusy(true)
      try {
        const result = (await send({ type: 'updateContainer', id: container.id })) as
          | { ok?: boolean; error?: string; warnings?: string[] }
          | undefined
        if (result && result.ok === false) {
          setNotice({ message: result.error ?? 'Update failed', kind: 'error' })
          return
        }
        if (result?.warnings?.length) {
          setNotice({ message: result.warnings[0], kind: 'info' })
        }
      } finally {
        setBusy(false)
      }
    },
    [send]
  )

  const updateAll = useCallback(async (): Promise<void> => {
    setBusy(true)
    try {
      const result = (await send({ type: 'updateAll' })) as
        | { ok?: boolean; updated?: string[]; failures?: Array<{ name: string; error: string }> }
        | undefined
      const updatedCount = result?.updated?.length ?? 0
      const failures = result?.failures ?? []
      if (failures.length > 0) {
        setNotice({
          message: `Updated ${updatedCount}, ${failures.length} failed — ${failures[0].name}: ${failures[0].error}`,
          kind: 'error'
        })
      } else if (updatedCount === 0) {
        setNotice({ message: 'Everything is up to date', kind: 'info' })
      } else {
        setNotice({
          message: `Updated ${updatedCount} container${updatedCount === 1 ? '' : 's'}`,
          kind: 'info'
        })
      }
    } finally {
      setBusy(false)
    }
  }, [send])

  const toggleShowStopped = useCallback((): void => {
    onSettingsPatch({ showStopped: !showStopped })
    // The main module reads the merged host settings on each refresh; give the
    // connection update a moment to land before asking for a fresh list.
    setTimeout(() => void send({ type: 'refresh' }), 60)
  }, [onSettingsPatch, send, showStopped])

  const installEngine = useCallback((): void => {
    setInstallLines([])
    setInstalling(true)
    void send({ type: 'installEngine' }).then((result) => {
      const typed = result as { ok?: boolean; error?: string; command?: string } | undefined
      if (typed && typed.ok === false) {
        setInstalling(false)
        setNotice({ message: typed.error ?? 'Install failed', kind: 'error' })
        if (typed.command) {
          setInstallLines((prev) => [...prev, `Run manually: ${typed.command}`])
        }
      }
    })
  }, [send])

  /* ------------------------------------------------------------------ view */

  const visibleContainers = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    const sorted = sortContainers(containers)
    if (!needle) {
      return sorted
    }
    return sorted.filter(
      (container) =>
        containerLabel(container).toLowerCase().includes(needle) ||
        container.image.toLowerCase().includes(needle) ||
        container.id.toLowerCase().includes(needle) ||
        container.status.toLowerCase().includes(needle)
    )
  }, [containers, filter])

  const engineReady = engine?.state === 'ready'
  const runningCount = containers.filter((container) => container.state === 'running').length
  const staleCount = updates.staleContainerIds.length

  return (
    <div className="plugin-ui-panel docker-view">
      <div className="plugin-ui-toolbar docker-toolbar docker-toolbar-wrap">
        <div className="docker-tabs" role="tablist">
          {TABS.map((entry) => (
            <button
              key={entry.id}
              type="button"
              role="tab"
              aria-selected={tab === entry.id}
              className={`docker-tab${tab === entry.id ? ' active' : ''}`}
              onClick={() => setTab(entry.id)}
            >
              {entry.label}
            </button>
          ))}
        </div>
        <div className="plugin-ui-spacer" />
        {engineReady ? (
          <span className="plugin-ui-muted docker-summary">
            {runningCount} running / {containers.length}
            {updates.checking
              ? ' · checking updates…'
              : staleCount > 0
                ? ` · ${staleCount} update${staleCount === 1 ? '' : 's'}`
                : ''}
            {lastUpdated ? ` · ${new Date(lastUpdated).toLocaleTimeString()}` : ''}
          </span>
        ) : null}
        {tab === 'containers' ? (
          <>
            <input
              className="plugin-ui-input docker-filter"
              type="search"
              value={filter}
              placeholder="Filter…"
              aria-label="Filter containers"
              onChange={(event) => setFilter(event.target.value)}
            />
            <label className="docker-checkbox" title="Include exited and created containers">
              <input type="checkbox" checked={showStopped} onChange={toggleShowStopped} />
              <span>Show stopped</span>
            </label>
            <PluginButton compact onClick={() => { setCreateImage(''); setCreateOpen(true) }}>
              New container
            </PluginButton>
            <PluginButton compact onClick={() => void send({ type: 'probe' })}>
              Refresh
            </PluginButton>
            <PluginButton
              compact
              disabled={!engineReady || updates.checking}
              title="Check the registry for newer image tags"
              onClick={checkUpdates}
            >
              Check updates
            </PluginButton>
            <PluginButton
              variant={staleCount > 0 ? 'primary' : undefined}
              compact
              disabled={!engineReady || busy || updates.checking}
              title="Pull newer images and recreate the affected containers"
              onClick={() => void updateAll()}
            >
              Update all{staleCount > 0 ? ` (${staleCount})` : ''}
            </PluginButton>
            <PluginButton compact onClick={() => setEngineOpen(true)} title="Docker engine details and upgrade">
              Engine
            </PluginButton>
          </>
        ) : null}
      </div>

      <EngineBanner
        engine={engine}
        installing={installing}
        installLines={installLines}
        onInstall={installEngine}
        onRetry={() => void send({ type: 'probe' })}
      />

      <div className="docker-body">
        {tab === 'containers' ? (
          <ContainersList
            containers={visibleContainers}
            busy={busy}
            ready={engineReady}
            staleIds={updates.staleContainerIds}
            updateEvents={updateEvents}
            onAction={requestAction}
            onDetails={setDetail}
            onShell={setShell}
            onUpdate={updateContainer}
          />
        ) : null}
        {tab === 'images' ? <ImagesPanel tabId={tabId} pluginId={pluginId} ready={engineReady} notify={setNotice} confirmDestructive={confirmDestructive} /> : null}
        {tab === 'volumes' ? <VolumesPanel tabId={tabId} pluginId={pluginId} ready={engineReady} notify={setNotice} confirmDestructive={confirmDestructive} /> : null}
        {tab === 'networks' ? <NetworksPanel tabId={tabId} pluginId={pluginId} ready={engineReady} notify={setNotice} confirmDestructive={confirmDestructive} /> : null}
      </div>

      {notice ? (
        <div className={`docker-notice ${notice.kind === 'error' ? 'plugin-ui-error' : 'plugin-ui-success'}`}>
          <span>{notice.message}</span>
          <button type="button" className="docker-notice-close" onClick={() => setNotice(null)} aria-label="Dismiss">
            ×
          </button>
        </div>
      ) : null}

      {detail ? (
        <ContainerDetail
          tabId={tabId}
          pluginId={pluginId}
          container={detail}
          onClose={() => setDetail(null)}
          onAction={requestAction}
          onShell={() => setShell(detail)}
          onUpdate={() => void updateContainer(detail)}
          onOpenCreateFromImage={() => {
            setCreateImage(detail.image)
            setCreateOpen(true)
          }}
        />
      ) : null}

      {shell ? (
        <ContainerShell
          tabId={tabId}
          pluginId={pluginId}
          container={shell}
          onClose={() => setShell(null)}
        />
      ) : null}

      {createOpen ? (
        <CreateContainerDialog
          tabId={tabId}
          pluginId={pluginId}
          initialImage={createImage}
          onClose={() => setCreateOpen(false)}
          onCreated={(message) => {
            setCreateOpen(false)
            setNotice({ message, kind: 'info' })
            void send({ type: 'refresh' })
          }}
        />
      ) : null}

      {engineOpen ? (
        <EngineDialog
          status={engine}
          busy={installing}
          lines={installLines}
          updatesAt={updates.at}
          staleCount={staleCount}
          checkedCount={updates.checkedImages.length}
          updateErrors={updates.errors}
          onUpdateEngine={() => {
            setInstallLines([])
            setInstalling(true)
            void send({ type: 'updateEngine' }).then((result) => {
              const typed = result as { ok?: boolean; error?: string; command?: string } | undefined
              setInstalling(false)
              if (typed && typed.ok === false) {
                setNotice({ message: typed.error ?? 'Engine update failed', kind: 'error' })
              } else {
                setNotice({ message: 'Docker engine updated', kind: 'info' })
              }
            })
          }}
          onCheckUpdates={checkUpdates}
          onClose={() => setEngineOpen(false)}
        />
      ) : null}

      {pending ? (
        <ConfirmDialog
          title={`${pending.action === 'remove' ? 'Remove' : 'Kill'} container`}
          message={
            pending.action === 'remove'
              ? `Remove "${containerLabel(pending.container)}"? Its filesystem is deleted and this cannot be undone.`
              : `Kill "${containerLabel(pending.container)}"? The process is stopped immediately and this cannot be undone.`
          }
          confirmLabel={pending.action === 'remove' ? 'Remove' : 'Kill'}
          onCancel={() => setPending(null)}
          onConfirm={() => {
            const { container, action } = pending
            setPending(null)
            void runAction(container, action)
          }}
        />
      ) : null}
    </div>
  )
}

/* --------------------------------------------------------------- fragments */

function EngineBanner({
  engine,
  installing,
  installLines,
  onInstall,
  onRetry
}: {
  engine: DockerEngineStatus | null
  installing: boolean
  installLines: string[]
  onInstall: () => void
  onRetry: () => void
}): ReactElement | null {
  if (!engine) {
    return <div className="plugin-ui-hint docker-banner">Checking for a Docker daemon…</div>
  }
  if (engine.state === 'ready') {
    return null
  }

  if (engine.state === 'checking') {
    return <div className="plugin-ui-hint docker-banner">Checking for a Docker daemon…</div>
  }

  return (
    <div className="plugin-ui-warning docker-banner">
      <div className="docker-banner-text">
        <strong>
          {engine.state === 'unsupported'
            ? 'Docker is unavailable for this session'
            : engine.state === 'forbidden'
              ? 'Permission denied for the Docker socket'
              : 'No Docker daemon detected'}
        </strong>
        <span className="plugin-ui-muted">
          {engine.message ?? `Socket: ${engine.socketPath}`}
        </span>
        {engine.message ? (
          <span className="plugin-ui-hint">Socket: {engine.socketPath}</span>
        ) : null}
      </div>
      <div className="plugin-ui-actions">
        <PluginButton compact onClick={onRetry}>
          Retry
        </PluginButton>
        {engine.installable ? (
          <PluginButton variant="primary" compact onClick={onInstall} disabled={installing}>
            {installing ? 'Installing…' : 'Install Docker'}
          </PluginButton>
        ) : null}
      </div>
      {installLines.length > 0 ? (
        <pre className="docker-install-log">{installLines.slice(-40).join('\n')}</pre>
      ) : null}
    </div>
  )
}

function ContainersList({
  containers,
  busy,
  ready,
  staleIds,
  updateEvents,
  onAction,
  onDetails,
  onShell,
  onUpdate
}: {
  containers: DockerContainerView[]
  busy: boolean
  ready: boolean
  staleIds: string[]
  updateEvents: Record<string, DockerUpdateEvent>
  onAction: (container: DockerContainerView, action: DockerContainerAction) => void
  onDetails: (container: DockerContainerView) => void
  onShell: (container: DockerContainerView) => void
  onUpdate: (container: DockerContainerView) => void
}): ReactElement {
  const staleIdSet = useMemo(() => new Set(staleIds), [staleIds])

  const phaseOf = (id: string): DockerUpdateEvent['phase'] | undefined => updateEvents[id]?.phase
  const isUpdating = (id: string): boolean => {
    const phase = phaseOf(id)
    return phase === 'pulling' || phase === 'recreating' || phase === 'starting'
  }
  const updateLabel = (id: string): string => {
    switch (phaseOf(id)) {
      case 'pulling':
        return 'Pulling…'
      case 'recreating':
        return 'Recreating…'
      case 'starting':
        return 'Starting…'
      default:
        return 'Update'
    }
  }

  if (!ready) {
    return <div className="plugin-ui-empty">Docker is not available</div>
  }
  if (containers.length === 0) {
    return <div className="plugin-ui-empty">No containers</div>
  }
  return (
    <ul className="docker-list">
      {containers.map((container) => {
        const actions = applicableActions(container)
        return (
          <li key={container.id} className="docker-row">
            <button type="button" className="docker-row-main" onClick={() => onDetails(container)}>
              <span className={`docker-state docker-state-${container.state}`}>
                {stateLabel(container.state)}
              </span>
              {staleIdSet.has(container.id) ? (
                <span className="docker-stale" title="A newer image is available in the registry">
                  update
                </span>
              ) : null}
              <span className="docker-row-title">
                <span className="docker-row-name">{containerLabel(container)}</span>
                <span className="plugin-ui-muted docker-row-image">{container.image}</span>
              </span>
              <span className="plugin-ui-muted docker-row-ports">{formatPorts(container.ports)}</span>
              <span className="plugin-ui-muted docker-row-status">{container.status}</span>
            </button>
            <div className="docker-row-actions">
              <PluginButton
                variant={staleIdSet.has(container.id) ? 'primary' : undefined}
                compact
                disabled={busy || isUpdating(container.id)}
                title="Pull the newest image and recreate this container with the same settings"
                onClick={() => onUpdate(container)}
              >
                {updateLabel(container.id)}
              </PluginButton>
              <PluginButton compact disabled={busy} onClick={() => onShell(container)} title="Open a shell in this container">
                Shell
              </PluginButton>
              {actions.map((action) => {
                const entry = ROW_ACTIONS.find((candidate) => candidate.action === action)
                return (
                  <PluginButton
                    key={action}
                    compact
                    variant={action === 'remove' || action === 'kill' ? 'danger' : undefined}
                    disabled={busy}
                    title={entry?.title}
                    onClick={() => onAction(container, action)}
                  >
                    {entry?.label ?? action}
                  </PluginButton>
                )
              })}
            </div>
          </li>
        )
      })}
    </ul>
  )
}

function ConfirmDialog({
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
