import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type DragEvent as ReactDragEvent,
  type ReactElement
} from 'react'
import type { PluginViewProps } from '../../../renderer/src/plugins/api'
import type {
  SftpEntry,
  SftpMainPayload,
  SftpRendererMessage,
  SftpStatusState
} from './protocol'
import {
  base64ToBytes,
  baseName,
  formatBytes,
  formatDate,
  hexDump,
  joinPath,
  octalMode,
  parentPath,
  typeIcon
} from './viewUtils'
import { collectDroppedFiles, isFileDrag, uploadFilesOverSftp } from './fileDrop'
import './styles.css'

interface TransferProgress {
  direction: 'upload' | 'download' | 'download-zip'
  transferred: number
  total: number
}

type SftpDialog =
  | { kind: 'mkdir' }
  | { kind: 'rename'; path: string; name: string }
  | { kind: 'chmod'; path: string; mode: number }
  | { kind: 'delete'; path: string; name: string }
  | null

interface SftpViewer {
  path: string
  name: string
  loading: boolean
  kind?: 'text' | 'binary'
  text?: string
  bytes?: Uint8Array
  truncated: boolean
  bytesRead: number
  totalBytes?: number
  error?: string
}

interface SftpContextMenu {
  x: number
  y: number
  /** Entry the menu was opened on; null = opened on empty folder space. */
  entry: SftpEntry | null
}

export default function SftpView({ tabId, pluginId }: PluginViewProps): ReactElement {
  const [status, setStatus] = useState<SftpStatusState>('idle')
  const [cwd, setCwd] = useState<string | null>(null)
  const [statusReason, setStatusReason] = useState<string | undefined>()
  const [path, setPath] = useState<string | null>(null)
  const [entries, setEntries] = useState<SftpEntry[]>([])
  const [loading, setLoading] = useState(false)
  const [listError, setListError] = useState<string | null>(null)
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [dialog, setDialog] = useState<SftpDialog>(null)
  const [dialogInput, setDialogInput] = useState('')
  const [dialogError, setDialogError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [transfers, setTransfers] = useState<Map<string, TransferProgress>>(() => new Map())
  const [contextMenu, setContextMenu] = useState<SftpContextMenu | null>(null)
  const [viewer, setViewer] = useState<SftpViewer | null>(null)
  const contextMenuRef = useRef<HTMLDivElement | null>(null)
  const fileInputRef = useRef<HTMLInputElement | null>(null)
  const dragDepthRef = useRef(0)
  const [dropActive, setDropActive] = useState(false)

  const requestedRef = useRef<string | null>(null)
  const pathRef = useRef<string | null>(null)
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  pathRef.current = path

  const showNotice = useCallback((msg: string): void => {
    setNotice(msg)
    if (noticeTimer.current) {
      clearTimeout(noticeTimer.current)
    }
    noticeTimer.current = setTimeout(() => setNotice(null), 3500)
  }, [])

  const requestList = useCallback(
    (target: string): void => {
      requestedRef.current = target
      setLoading(true)
      setListError(null)
      void window.wassh.sendPluginMessage(tabId, pluginId, {
        type: 'list',
        path: target
      } satisfies SftpRendererMessage)
    },
    [tabId, pluginId]
  )

  const send = useCallback(
    (payload: SftpRendererMessage): Promise<unknown> => window.wassh.sendPluginMessage(tabId, pluginId, payload),
    [tabId, pluginId]
  )

  const uploadDroppedFiles = useCallback(
    async (files: File[]): Promise<void> => {
      const dir = pathRef.current
      if (!dir || files.length === 0) {
        return
      }
      let failed = 0
      const before = new Set(files.map((file) => joinPath(dir, file.name)))
      await uploadFilesOverSftp({ tabId, pluginId, files, path: dir })
      setTransfers((prev) => {
        const next = new Map(prev)
        for (const remotePath of before) {
          if (next.has(remotePath)) {
            failed += 1
          }
          next.delete(remotePath)
        }
        return next
      })
      if (failed > 0) {
        showNotice(`${failed} upload${failed === 1 ? '' : 's'} failed`)
      }
    },
    [pluginId, showNotice, tabId]
  )

  const openFilePicker = useCallback((): void => {
    fileInputRef.current?.click()
  }, [])

  // On mount, request the latest status in case the connection event fired
  // before this view mounted. The 'status: connected' handler drives the list.
  useEffect(() => {
    void send({ type: 'getStatus' })
    return () => {
      if (noticeTimer.current) {
        clearTimeout(noticeTimer.current)
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabId, pluginId])

  useEffect(() => {
    return window.wassh.onPluginMessage((ev) => {
      if (ev.tabId !== tabId || ev.pluginId !== pluginId) {
        return
      }
      const payload = ev.payload as SftpMainPayload | null
      if (!payload || typeof payload !== 'object' || !('type' in payload)) {
        return
      }
      switch (payload.type) {
        case 'status': {
          setStatus(payload.state)
          setStatusReason(payload.reason)
          if (payload.state === 'connected') {
            if (payload.cwd) {
              setCwd(payload.cwd)
            }
            if (pathRef.current === null) {
              const start = payload.cwd ?? '/'
              setPath(start)
              requestList(start)
            }
          }
          return
        }
        case 'listResult': {
          setLoading(false)
          if (requestedRef.current !== payload.path) {
            return
          }
          setCwd(payload.cwd)
          if (pathRef.current === null) {
            setPath(payload.path)
          }
          if (payload.error) {
            setListError(payload.error)
            setEntries([])
          } else {
            setListError(null)
            setEntries(payload.entries)
            setSelectedPath((prev) =>
              prev && payload.entries.some((e) => e.path === prev) ? prev : null
            )
          }
          return
        }
        case 'opResult': {
          if (payload.ok) {
            setDialog(null)
            setDialogError(null)
            if (pathRef.current) {
              requestList(pathRef.current)
            }
          } else {
            setDialogError(payload.error ?? 'Operation failed')
            showNotice(payload.error ?? 'Operation failed')
          }
          return
        }
        case 'transferProgress': {
          setTransfers((prev) => {
            const next = new Map(prev)
            next.set(payload.remotePath, {
              direction: payload.direction,
              transferred: payload.transferredBytes,
              total: payload.totalBytes
            })
            return next
          })
          return
        }
        case 'transferDone': {
          // Single completion point for every upload/download (drag-drop and
          // file-picker uploads both funnel through the same chunk transfer,
          // so they arrive here identically).
          setTransfers((prev) => {
            const next = new Map(prev)
            next.delete(payload.remotePath)
            return next
          })
          if (payload.state !== 'done') {
            if (payload.error) {
              showNotice(payload.error)
            } else if (payload.state === 'cancelled') {
              showNotice('Transfer cancelled')
            }
            return
          }
          if (payload.direction === 'upload') {
            showNotice(`Uploaded ${baseName(payload.remotePath)}`)
            if (pathRef.current) {
              requestList(pathRef.current)
            }
          } else if (payload.direction === 'download-zip') {
            showNotice(`Downloaded ${baseName(payload.remotePath)} as ZIP`)
          }
          return
        }
        case 'viewFileResult': {
          setViewer((prev) => {
            if (!prev || prev.path !== payload.path) {
              return prev
            }
            if (!payload.ok) {
              return {
                ...prev,
                loading: false,
                error: payload.error ?? 'Failed to read file'
              }
            }
            const kind = payload.kind ?? 'binary'
            return {
              ...prev,
              loading: false,
              kind,
              text: kind === 'text' ? (payload.text ?? '') : undefined,
              bytes:
                kind === 'binary'
                  ? payload.contentBase64
                    ? base64ToBytes(payload.contentBase64)
                    : new Uint8Array(0)
                  : undefined,
              truncated: payload.truncated,
              bytesRead: payload.bytesRead,
              totalBytes: payload.totalBytes,
              error: undefined
            }
          })
          return
        }
      }
    })
  }, [tabId, pluginId, requestList, showNotice])

  const closeViewer = useCallback((): void => {
    setViewer(null)
  }, [])

  const openViewer = useCallback(
    (entry: SftpEntry): void => {
      setViewer({
        path: entry.path,
        name: entry.name,
        loading: true,
        truncated: false,
        bytesRead: 0
      })
      void send({ type: 'viewFile', path: entry.path })
    },
    [send]
  )

  // Close the context menu on outside mousedown, Escape, or window blur.
  useEffect(() => {
    if (!contextMenu) {
      return
    }
    const onMouseDown = (e: MouseEvent): void => {
      if (contextMenuRef.current && contextMenuRef.current.contains(e.target as Node)) {
        return
      }
      setContextMenu(null)
    }
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        setContextMenu(null)
      }
    }
    const onBlur = (): void => setContextMenu(null)
    window.addEventListener('mousedown', onMouseDown)
    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('blur', onBlur)
    return () => {
      window.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('blur', onBlur)
    }
  }, [contextMenu])

  // Keep the menu inside the window when opened near an edge.
  useLayoutEffect(() => {
    const el = contextMenuRef.current
    if (!contextMenu || !el) {
      return
    }
    const rect = el.getBoundingClientRect()
    const pad = 4
    let x = contextMenu.x
    let y = contextMenu.y
    if (x + rect.width > window.innerWidth - pad) {
      x = Math.max(pad, window.innerWidth - rect.width - pad)
    }
    if (y + rect.height > window.innerHeight - pad) {
      y = Math.max(pad, window.innerHeight - rect.height - pad)
    }
    el.style.left = `${x}px`
    el.style.top = `${y}px`
  }, [contextMenu])

  const selected = entries.find((e) => e.path === selectedPath) ?? null
  const zipTarget = selected?.type === 'directory' ? selected.path : !selected && path ? path : null
  const parentDir = path ? parentPath(path) : null

  const navigate = (target: string): void => {
    setPath(target)
    requestList(target)
  }

  const openEntry = (entry: SftpEntry): void => {
    if (entry.type === 'directory') {
      navigate(entry.path)
    }
  }

  const submitDialog = async (): Promise<void> => {
    if (!dialog) {
      return
    }
    setDialogError(null)
    try {
      if (dialog.kind === 'mkdir') {
        const name = dialogInput.trim()
        if (!name) {
          setDialogError('Name is required')
          return
        }
        await send({ type: 'mkdir', path: joinPath(pathRef.current ?? '/', name) })
      } else if (dialog.kind === 'rename') {
        const name = dialogInput.trim()
        if (!name) {
          setDialogError('Name is required')
          return
        }
        const parent = parentPath(dialog.path) ?? '/'
        await send({ type: 'rename', oldPath: dialog.path, newPath: joinPath(parent, name) })
      } else if (dialog.kind === 'chmod') {
        const mode = parseInt(dialogInput.trim(), 8)
        if (Number.isNaN(mode) || mode < 0 || mode > 0o7777) {
          setDialogError('Enter a valid octal mode (e.g. 644 or 755)')
          return
        }
        await send({ type: 'chmod', path: dialog.path, mode })
      } else if (dialog.kind === 'delete') {
        await send({ type: 'delete', path: dialog.path })
      }
    } catch (err) {
      setDialogError(err instanceof Error ? err.message : String(err))
    }
  }

  const openDialog = (d: SftpDialog): void => {
    setDialogError(null)
    setDialog(d)
    if (d?.kind === 'mkdir') {
      setDialogInput('')
    } else if (d?.kind === 'rename') {
      setDialogInput(d.name)
    } else if (d?.kind === 'chmod') {
      setDialogInput(octalMode(d.mode))
    }
  }

  const renderDialog = (): ReactElement | null => {
    if (!dialog) {
      return null
    }
    return (
      <div className="sftp-modal-backdrop" onMouseDown={() => setDialog(null)}>
        <div
          className="sftp-modal"
          onMouseDown={(e) => e.stopPropagation()}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              e.preventDefault()
              setDialog(null)
              return
            }
            if (e.key !== 'Enter' && e.key !== ' ') {
              return
            }
            const target = e.target as HTMLElement
            // Focused buttons already activate on Enter/Space natively.
            if (target instanceof HTMLButtonElement) {
              return
            }
            // Space still types inside text fields.
            if (target instanceof HTMLInputElement && e.key === ' ') {
              return
            }
            e.preventDefault()
            void submitDialog()
          }}
        >
          <div className="sftp-modal-title">
            {dialog.kind === 'mkdir'
              ? 'New folder'
              : dialog.kind === 'rename'
                ? 'Rename'
                : dialog.kind === 'chmod'
                  ? 'Change permissions'
                  : 'Delete'}
          </div>
          {dialog.kind === 'delete' ? (
            <div className="sftp-modal-body">
              <p>
                Delete <strong>{dialog.name}</strong>? This cannot be undone.
              </p>
              {dialogError && <p className="sftp-modal-error">{dialogError}</p>}
            </div>
          ) : (
            <div className="sftp-modal-body">
              {dialog.kind === 'chmod' && (
                <p className="sftp-modal-hint">Octal mode, e.g. 644, 755, 700</p>
              )}
              <input
                className="sftp-modal-input"
                value={dialogInput}
                onChange={(e) => setDialogInput(e.target.value)}
                autoFocus
              />
              {dialogError && <p className="sftp-modal-error">{dialogError}</p>}
            </div>
          )}
          <div className="sftp-modal-actions">
            <button type="button" className="sftp-btn" onClick={() => setDialog(null)}>
              Cancel
            </button>
            <button
              type="button"
              className="sftp-btn sftp-btn-primary"
              autoFocus={dialog.kind === 'delete'}
              onClick={() => void submitDialog()}
            >
              {dialog.kind === 'delete' ? 'Delete' : 'OK'}
            </button>
          </div>
        </div>
      </div>
    )
  }

  const renderViewer = (): ReactElement | null => {
    if (!viewer) {
      return null
    }
    const hex = viewer.kind === 'binary' && viewer.bytes ? hexDump(viewer.bytes) : ''
    const metaParts: string[] = []
    if (viewer.kind) {
      metaParts.push(viewer.kind)
    }
    metaParts.push(formatBytes(viewer.bytesRead))
    if (viewer.totalBytes && viewer.totalBytes > viewer.bytesRead) {
      metaParts.push(`of ${formatBytes(viewer.totalBytes)}`)
    }
    return (
      <div className="sftp-modal-backdrop sftp-viewer-backdrop" onMouseDown={closeViewer}>
        <div
          className="sftp-viewer"
          onMouseDown={(e) => e.stopPropagation()}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              e.preventDefault()
              closeViewer()
            }
          }}
        >
          <div className="sftp-viewer-titlebar">
            <span className="sftp-viewer-title" title={viewer.path}>
              {viewer.name}
            </span>
            <span className="sftp-viewer-path" title={viewer.path}>
              {viewer.path}
            </span>
            <span className="sftp-viewer-meta">{metaParts.join(' · ')}</span>
            <button
              type="button"
              className="sftp-btn sftp-btn-mini"
              autoFocus
              onClick={closeViewer}
            >
              ✕ Close
            </button>
          </div>
          {viewer.truncated && !viewer.loading && !viewer.error && (
            <div className="sftp-viewer-truncated">
              Showing first {formatBytes(viewer.bytesRead)}
              {viewer.totalBytes ? ` of ${formatBytes(viewer.totalBytes)}` : ''} — file larger than
              the 1 MiB view limit.
            </div>
          )}
          <div className={`sftp-viewer-body${viewer.kind === 'binary' ? ' sftp-viewer-hex' : ''}`}>
            {viewer.loading && (
              <div className="sftp-loading">
                <span className="sftp-spinner" />
                Reading file…
              </div>
            )}
            {!viewer.loading && viewer.error && (
              <div className="sftp-empty sftp-error">⚠ {viewer.error}</div>
            )}
            {!viewer.loading && !viewer.error && viewer.bytesRead === 0 && (
              <div className="sftp-empty">File is empty</div>
            )}
            {!viewer.loading &&
              !viewer.error &&
              viewer.bytesRead > 0 &&
              (viewer.kind === 'binary' ? (
                <pre className="sftp-viewer-content">{hex}</pre>
              ) : (
                <pre className="sftp-viewer-content sftp-viewer-text">{(viewer.text ?? '').replace(/\r?\n$/, '')}</pre>
              ))}
          </div>
        </div>
      </div>
    )
  }

  const renderContextMenu = (): ReactElement | null => {
    if (!contextMenu) {
      return null
    }
    const { entry } = contextMenu
    const run = (fn: () => void): void => {
      setContextMenu(null)
      fn()
    }
    const item = (label: string, icon: string, danger: boolean, onSelect: () => void): ReactElement => (
      <button
        type="button"
        className={`sftp-context-item${danger ? ' danger' : ''}`}
        onClick={() => run(onSelect)}
      >
        <span className="sftp-context-icon">{icon}</span>
        {label}
      </button>
    )
    return (
      <div
        className="sftp-context-menu"
        ref={contextMenuRef}
        style={{ left: contextMenu.x, top: contextMenu.y }}
      >
        {entry ? (
          <>
            {entry.type === 'directory' &&
              item('Open', '📂', false, () => navigate(entry.path))}
            {entry.type === 'file' &&
              item('View', '👁', false, () => openViewer(entry))}
            {entry.type === 'file' &&
              item('Download', '⬇', false, () => {
                void send({ type: 'download', path: entry.path })
              })}
            {entry.type === 'directory' &&
              item('Download as ZIP', '⬇', false, () => {
                void send({ type: 'downloadZip', path: entry.path })
              })}
            {item('Rename', '✎', false, () =>
              openDialog({ kind: 'rename', path: entry.path, name: entry.name })
            )}
            {item('Permissions', '🔒', false, () =>
              openDialog({ kind: 'chmod', path: entry.path, mode: entry.mode })
            )}
            <div className="sftp-context-sep" />
            {item('Delete', '🗑', true, () =>
              openDialog({ kind: 'delete', path: entry.path, name: entry.name })
            )}
          </>
        ) : (
          <>
            {item('New folder', '+', false, () => openDialog({ kind: 'mkdir' }))}
            {item('Upload files…', '⬆', false, () => openFilePicker())}
            {path &&
              item('Download current folder as ZIP', '⬇', false, () => {
                void send({ type: 'downloadZip', path })
              })}
            {item('Refresh', '⟳', false, () => {
              if (pathRef.current) {
                requestList(pathRef.current)
              }
            })}
          </>
        )}
      </div>
    )
  }

  const connected = status === 'connected'

  const handleDragEnter = (e: ReactDragEvent<HTMLDivElement>): void => {
    if (!connected || !isFileDrag(e)) {
      return
    }
    e.preventDefault()
    dragDepthRef.current += 1
    setDropActive(true)
  }

  const handleDragOver = (e: ReactDragEvent<HTMLDivElement>): void => {
    if (!connected || !isFileDrag(e)) {
      return
    }
    e.preventDefault()
    e.dataTransfer.dropEffect = 'copy'
  }

  const handleDragLeave = (e: ReactDragEvent<HTMLDivElement>): void => {
    if (!connected || !isFileDrag(e)) {
      return
    }
    dragDepthRef.current = Math.max(0, dragDepthRef.current - 1)
    if (dragDepthRef.current === 0) {
      setDropActive(false)
    }
  }

  const handleDrop = (e: ReactDragEvent<HTMLDivElement>): void => {
    if (!isFileDrag(e)) {
      return
    }
    e.preventDefault()
    dragDepthRef.current = 0
    setDropActive(false)
    if (!connected) {
      return
    }
    const files = collectDroppedFiles(e.dataTransfer)
    if (files.length > 0) {
      void uploadDroppedFiles(files)
    }
  }

  return (
    <div
      className="sftp-view"
      onDragEnter={handleDragEnter}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      <div className="sftp-statusbar">
        <span className={`sftp-status-dot sftp-status-${status}`} />
        <span className="sftp-status-text">
          {status === 'connected'
            ? `Connected — uploads go to ${cwd ?? '/'}`
            : status === 'connecting'
              ? 'Connecting SFTP…'
              : status === 'error'
                ? statusReason ?? 'SFTP error'
                : 'SFTP not connected'}
        </span>
        {connected && (
          <button
            type="button"
            className="sftp-btn sftp-btn-mini"
            title="Re-resolve upload directory (session cwd → ~/Downloads → home)"
            onClick={() => void send({ type: 'resetCwd' })}
          >
            ↺ Reset upload dir
          </button>
        )}
      </div>

      <div className="sftp-toolbar">
        <button
          type="button"
          className="sftp-btn"
          disabled={!connected || path === null}
          title="Go to parent directory"
          onClick={() => {
            if (path) {
              const p = parentPath(path)
              if (p) {
                navigate(p)
              }
            }
          }}
        >
          ⬆ Up
        </button>
        <button
          type="button"
          className="sftp-btn"
          disabled={!connected}
          title="Refresh listing"
          onClick={() => {
            if (path) {
              requestList(path)
            }
          }}
        >
          ⟳ Refresh
        </button>
        <span className="sftp-toolbar-sep" />
        <button
          type="button"
          className="sftp-btn"
          disabled={!connected}
          title="Create a new folder here"
          onClick={() => openDialog({ kind: 'mkdir' })}
        >
          + New folder
        </button>
        <button
          type="button"
          className="sftp-btn"
          disabled={!connected}
          title="Upload files into the current folder (or drop them here)"
          onClick={() => openFilePicker()}
        >
          ⬆ Upload…
        </button>
        <span className="sftp-toolbar-sep" />
        <button
          type="button"
          className="sftp-btn"
          disabled={!connected || !selected}
          title="Rename selected item"
          onClick={() => {
            if (selected) {
              openDialog({ kind: 'rename', path: selected.path, name: selected.name })
            }
          }}
        >
          ✎ Rename
        </button>
        <button
          type="button"
          className="sftp-btn"
          disabled={!connected || !selected}
          title="Change permissions of selected item"
          onClick={() => {
            if (selected) {
              openDialog({ kind: 'chmod', path: selected.path, mode: selected.mode })
            }
          }}
        >
          🔒 Permissions
        </button>
        <button
          type="button"
          className="sftp-btn sftp-btn-danger"
          disabled={!connected || !selected}
          title="Delete selected item"
          onClick={() => {
            if (selected) {
              openDialog({ kind: 'delete', path: selected.path, name: selected.name })
            }
          }}
        >
          🗑 Delete
        </button>
        <span className="sftp-toolbar-sep" />
        <button
          type="button"
          className="sftp-btn"
          disabled={!connected || !selected || selected.type !== 'file'}
          title="Download selected file"
          onClick={() => {
            if (selected) {
              void send({ type: 'download', path: selected.path })
            }
          }}
        >
          ⬇ Download
        </button>
        <button
          type="button"
          className="sftp-btn"
          disabled={!connected || !zipTarget}
          title={
            selected?.type === 'directory'
              ? 'Download selected folder as a ZIP archive'
              : 'Download current folder as a ZIP archive'
          }
          onClick={() => {
            if (zipTarget) {
              void send({ type: 'downloadZip', path: zipTarget })
            }
          }}
        >
          ⬇ Download ZIP
        </button>
      </div>

      <div className="sftp-pathbar" title={path ?? ''}>
        <span className="sftp-pathbar-label">Path</span>
        <button
          type="button"
          className="sftp-path-link"
          onClick={() => {
            if (path) {
              navigate('/')
            }
          }}
        >
          /
        </button>
        {path && path !== '/' && (
          <>
            <span className="sftp-path-sep">›</span>
            <span className="sftp-path-current">{path.replace(/^\/+/, '')}</span>
          </>
        )}
      </div>

      {notice && <div className="sftp-notice">{notice}</div>}

      <div
        className="sftp-table-wrap"
        onContextMenu={(e) => {
          e.preventDefault()
          if (!connected || !path || (e.target as HTMLElement).closest('th')) {
            setContextMenu(null)
            return
          }
          setSelectedPath(null)
          setContextMenu({ x: e.clientX, y: e.clientY, entry: null })
        }}
      >
        <table className="sftp-table">
          <thead>
            <tr>
              <th className="sftp-col-name">Name</th>
              <th className="sftp-col-size">Size</th>
              <th className="sftp-col-mode">Mode</th>
              <th className="sftp-col-date">Modified</th>
            </tr>
          </thead>
          <tbody>
            {parentDir && (
              <tr
                key=".."
                className="sftp-row dir"
                onDoubleClick={() => navigate(parentDir)}
                onContextMenu={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  setContextMenu(null)
                }}
              >
                <td className="sftp-col-name">
                  <span className="sftp-icon">📁</span>
                  <span className="sftp-name">..</span>
                </td>
                <td className="sftp-col-size">—</td>
                <td className="sftp-col-mode" />
                <td className="sftp-col-date">—</td>
              </tr>
            )}
            {entries.map((entry) => (
              <tr
                key={entry.path}
                tabIndex={-1}
                className={`sftp-row${entry.path === selectedPath ? ' selected' : ''}${entry.type === 'directory' ? ' dir' : ''}`}
                onClick={(e) => {
                  setSelectedPath(entry.path)
                  e.currentTarget.focus({ preventScroll: true })
                }}
                onDoubleClick={() => openEntry(entry)}
                onKeyDown={(e) => {
                  if (e.key === 'Delete' && connected) {
                    e.preventDefault()
                    openDialog({ kind: 'delete', path: entry.path, name: entry.name })
                  }
                }}
                onContextMenu={(e) => {
                  if (!connected) {
                    return
                  }
                  e.preventDefault()
                  e.stopPropagation()
                  setSelectedPath(entry.path)
                  setContextMenu({ x: e.clientX, y: e.clientY, entry })
                }}
              >
                <td className="sftp-col-name">
                  <span className="sftp-icon">{typeIcon(entry)}</span>
                  <span className="sftp-name" title={entry.path}>
                    {entry.name}
                    {entry.type === 'symlink' && ' →'}
                  </span>
                  <span className="sftp-modesym" title={entry.modeSymbolic}>
                    {entry.modeSymbolic}
                  </span>
                </td>
                <td className="sftp-col-size">
                  {entry.type === 'directory' ? '—' : formatBytes(entry.size)}
                </td>
                <td className="sftp-col-mode">{octalMode(entry.mode)}</td>
                <td className="sftp-col-date">{formatDate(entry.mtime)}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {loading && (
          <div className="sftp-loading">
            <span className="sftp-spinner" />
            Loading…
          </div>
        )}
        {!loading && listError && <div className="sftp-empty sftp-error">⚠ {listError}</div>}
        {!loading && !listError && entries.length === 0 && (
          <div className="sftp-empty">Empty folder</div>
        )}
        {dropActive && (
          <div className="sftp-drop-hint">
            <div className="sftp-drop-box">
              <div className="sftp-drop-icon">⬆</div>
              <div className="sftp-drop-title">Drop files to upload</div>
              <div className="sftp-drop-text">into {path ?? '/'}</div>
            </div>
          </div>
        )}
      </div>

      {transfers.size > 0 && (
        <div className="sftp-transfers">
          {Array.from(transfers.entries()).map(([remotePath, t]) => (
            <div key={remotePath} className="sftp-transfer">
              <div className="sftp-transfer-label">
                <span>
                  {t.direction === 'upload'
                    ? '⬆ Upload'
                    : t.direction === 'download-zip'
                      ? '⬇ Download ZIP'
                      : '⬇ Download'}
                </span>
                <span className="sftp-transfer-name" title={remotePath}>
                  {remotePath}
                </span>
                <span className="sftp-transfer-amount">
                  {formatBytes(t.transferred)}
                  {t.total > 0 ? ` / ${formatBytes(t.total)}` : ''}
                </span>
              </div>
              <div className="sftp-transfer-track">
                <div
                  className="sftp-transfer-fill"
                  style={{ width: t.total > 0 ? `${Math.min(100, (t.transferred / t.total) * 100)}%` : '100%' }}
                />
              </div>
            </div>
          ))}
        </div>
      )}

      <input
        ref={fileInputRef}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          const files = e.target.files ? Array.from(e.target.files) : []
          e.target.value = ''
          if (files.length > 0) {
            void uploadDroppedFiles(files)
          }
        }}
      />
      {renderDialog()}
      {renderViewer()}
      {renderContextMenu()}
    </div>
  )
}
