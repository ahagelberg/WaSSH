import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type DragEvent as ReactDragEvent,
  type ReactElement
} from 'react'
import type {
  SftpEntry,
  SftpErrorKind,
  SftpRendererMessage,
  SftpStatusState
} from '@shared/plugins'
import type { PluginViewProps } from '../registry'

type SftpMainPayload =
  | {
      type: 'status'
      state: SftpStatusState
      cwd?: string
      reason?: string
      errorKind?: SftpErrorKind
    }
  | {
      type: 'listResult'
      path: string
      cwd: string
      entries: SftpEntry[]
      error?: string
      errorKind?: SftpErrorKind
    }
  | {
      type: 'opResult'
      op: 'mkdir' | 'rename' | 'chmod' | 'delete'
      path: string
      ok: boolean
      error?: string
      errorKind?: SftpErrorKind
    }
  | {
      type: 'transferProgress'
      direction: 'upload' | 'download'
      remotePath: string
      transferredBytes: number
      totalBytes: number
    }
  | {
      type: 'transferDone'
      direction: 'upload' | 'download'
      remotePath: string
      state: 'done' | 'error' | 'cancelled'
      error?: string
      errorKind?: SftpErrorKind
    }

interface TransferProgress {
  direction: 'upload' | 'download'
  transferred: number
  total: number
}

type SftpDialog =
  | { kind: 'mkdir' }
  | { kind: 'rename'; path: string; name: string }
  | { kind: 'chmod'; path: string; mode: number }
  | { kind: 'delete'; path: string; name: string }
  | null

interface SftpContextMenu {
  x: number
  y: number
  /** Entry the menu was opened on; null = opened on empty folder space. */
  entry: SftpEntry | null
}

function joinPath(parent: string, name: string): string {
  if (parent === '/' || parent === '') {
    return `/${name}`
  }
  return `${parent.replace(/\/+$/, '')}/${name}`
}

function parentPath(path: string): string | null {
  if (path === '/' || path === '') {
    return null
  }
  const trimmed = path.replace(/\/+$/, '')
  const idx = trimmed.lastIndexOf('/')
  if (idx <= 0) {
    return '/'
  }
  return trimmed.slice(0, idx)
}

function baseName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

function formatBytes(n: number): string {
  if (!n || n <= 0) {
    return '—'
  }
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = n
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${units[i]}`
}

function formatDate(ms: number): string {
  if (!ms) {
    return '—'
  }
  const d = new Date(ms)
  const pad = (x: number): string => String(x).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

function octalMode(mode: number): string {
  return (mode & 0o777).toString(8).padStart(3, '0')
}

function typeIcon(entry: SftpEntry): string {
  if (entry.type === 'directory') return '📁'
  if (entry.type === 'symlink') return '🔗'
  if (entry.type === 'file') return '📄'
  return '❓'
}

const UPLOAD_CHUNK_SIZE = 256 * 1024

/** Only OS file payloads qualify for drop-upload (never internal panel drags). */
function isFileDrag(e: ReactDragEvent<HTMLDivElement>): boolean {
  return Array.from(e.dataTransfer.types).includes('Files')
}

/** Files dropped from the OS; directories are skipped (cannot be chunk-uploaded). */
function collectDroppedFiles(dt: DataTransfer | null): File[] {
  if (!dt) {
    return []
  }
  const items = dt.items
  if (items && items.length > 0) {
    const files: File[] = []
    for (const item of Array.from(items)) {
      if (item.kind !== 'file') {
        continue
      }
      const entry = item.webkitGetAsEntry?.()
      if (entry?.isDirectory) {
        continue
      }
      const file = item.getAsFile()
      if (file) {
        files.push(file)
      }
    }
    return files
  }
  // Some sources expose only dataTransfer.files (no items API).
  return Array.from(dt.files)
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
    (payload: SftpRendererMessage): Promise<unknown> =>
      window.wassh.sendPluginMessage(tabId, pluginId, payload),
    [tabId, pluginId]
  )

  const uploadDroppedFiles = useCallback(
    async (files: File[]): Promise<void> => {
      const dir = pathRef.current
      if (!dir || files.length === 0) {
        return
      }
      const removeTransfer = (remotePath: string): void => {
        setTransfers((prev) => {
          const next = new Map(prev)
          next.delete(remotePath)
          return next
        })
      }
      let failed = 0
      for (const file of files) {
        const remotePath = joinPath(dir, file.name)
        try {
          await send({ type: 'uploadStart', name: file.name, size: file.size, path: dir })
          let offset = 0
          while (offset < file.size) {
            const end = Math.min(offset + UPLOAD_CHUNK_SIZE, file.size)
            const data = await file.slice(offset, end).arrayBuffer()
            await send({ type: 'uploadChunk', name: file.name, data: new Uint8Array(data) })
            offset = end
          }
          await send({ type: 'uploadEnd', name: file.name })
        } catch {
          void send({ type: 'cancel' })
          // Cancelled chunk uploads never emit transferDone; drop their bar now.
          removeTransfer(remotePath)
          failed += 1
        }
      }
      if (failed > 0) {
        showNotice(`${failed} upload${failed === 1 ? '' : 's'} failed`)
      }
    },
    [send, showNotice]
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
          }
          return
        }
      }
    })
  }, [tabId, pluginId, requestList, showNotice])

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
        <div className="sftp-modal" onMouseDown={(e) => e.stopPropagation()}>
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
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    void submitDialog()
                  }
                  if (e.key === 'Escape') {
                    setDialog(null)
                  }
                }}
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
              onClick={() => void submitDialog()}
            >
              {dialog.kind === 'delete' ? 'Delete' : 'OK'}
            </button>
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
              item('Download', '⬇', false, () => {
                void send({ type: 'download', path: entry.path })
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
                className={`sftp-row${entry.path === selectedPath ? ' selected' : ''}${entry.type === 'directory' ? ' dir' : ''}`}
                onClick={() => setSelectedPath(entry.path)}
                onDoubleClick={() => openEntry(entry)}
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
                <span>{t.direction === 'upload' ? '⬆ Upload' : '⬇ Download'}</span>
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
      {renderContextMenu()}
    </div>
  )
}
