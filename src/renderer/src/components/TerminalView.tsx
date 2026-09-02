import { useEffect, useRef, useState, type DragEvent as ReactDragEvent } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebLinksAddon } from '@xterm/addon-web-links'
import {
  BELL_MODE_INVERT_LINE,
  BELL_MODE_INVERT_WINDOW,
  BELL_MODE_SYSTEM,
  TAB_CLOSE_KEY,
  TAB_CYCLE_KEY,
  REOPEN_CLOSED_TAB_KEY,
  type AppSettings,
  type BellMode,
  type CursorStyle
} from '@shared/types'
import { terminalFontStack } from '@shared/connection'
import { toXtermSearchOptions, type TerminalSearchController } from '../terminalSearch'
import { sessionTerminalStyle } from '../sessionStyleCss'
import TerminalSearchBar from './TerminalSearchBar'

/** Debounce resize observer notifications */
const RESIZE_DEBOUNCE_MS = 50

/** Visual BEL invert duration */
const BELL_FLASH_MS = 200

/** DECSCUSR: CSI Ps SP q */
const DECSCUSR_INTERMEDIATE = ' '
const DECSCUSR_FINAL = 'q'

/** Open find bar (Ctrl/Cmd+Shift+F; must not be consumed by xterm) */
const FIND_KEY = 'f'

interface Props {
  tabId: string
  active: boolean
  settings: AppSettings
  fontSizePx: number
  fontFamily: string
  scrollbackLines: number
  bellMode: BellMode
  cursorStyle: CursorStyle
  cursorBlink: boolean
  termBackground: string
  termForeground: string
  findOpen: boolean
  findQuery: string
  findCaseSensitive: boolean
  findFocusNonce: number
  findFound: boolean | null
  /** Bump to request terminal focus (e.g. after an overlay closes) */
  focusNonce: number
  onFindQueryChange: (query: string) => void
  onFindCaseSensitiveChange: (value: boolean) => void
  onFindPrevious: () => void
  onFindNext: () => void
  onFindClose: () => void
  onData: (tabId: string, data: string) => void
  onResize: (tabId: string, cols: number, rows: number) => void
  registerWriter: (tabId: string, write: (data: string) => void) => void
  unregisterWriter: (tabId: string) => void
  registerSearch: (tabId: string, controller: TerminalSearchController) => void
  unregisterSearch: (tabId: string) => void
  /** When true, dropping files onto the terminal uploads them via the SFTP plugin. */
  dropEnabled: boolean
  onDropFiles: (tabId: string, files: File[]) => void
  /** Live progress of a file dropped onto this terminal for SFTP upload (null when idle). */
  dropUpload: { name: string; meta: string; pct: number } | null
}

function xtermThemeFromHost(el: HTMLElement) {
  const styles = getComputedStyle(el)
  const selection = styles.getPropertyValue('--term-selection').trim()
  return {
    background: styles.backgroundColor,
    foreground: styles.color,
    cursor: styles.color,
    selectionBackground: selection
  }
}

function positionBellLine(host: HTMLElement, term: Terminal, lineEl: HTMLElement): void {
  const screen = host.querySelector('.xterm-screen')
  if (!(screen instanceof HTMLElement) || term.rows <= 0) {
    return
  }
  const hostRect = host.getBoundingClientRect()
  const screenRect = screen.getBoundingClientRect()
  const cellHeight = screenRect.height / term.rows
  lineEl.style.left = `${screenRect.left - hostRect.left}px`
  lineEl.style.top = `${screenRect.top - hostRect.top + term.buffer.active.cursorY * cellHeight}px`
  lineEl.style.width = `${screenRect.width}px`
  lineEl.style.height = `${cellHeight}px`
}

export default function TerminalView({
  tabId,
  active,
  settings,
  fontSizePx,
  fontFamily,
  scrollbackLines,
  bellMode,
  cursorStyle,
  cursorBlink,
  termBackground,
  termForeground,
  findOpen,
  findQuery,
  findCaseSensitive,
  findFocusNonce,
  findFound,
  focusNonce,
  onFindQueryChange,
  onFindCaseSensitiveChange,
  onFindPrevious,
  onFindNext,
  onFindClose,
  onData,
  onResize,
  registerWriter,
  unregisterWriter,
  registerSearch,
  unregisterSearch,
  dropEnabled,
  onDropFiles,
  dropUpload
}: Props) {
  const hostRef = useRef<HTMLDivElement>(null)
  const bellLineRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const searchRef = useRef<SearchAddon | null>(null)
  const bellModeRef = useRef(bellMode)
  const cursorStyleRef = useRef(cursorStyle)
  const cursorBlinkRef = useRef(cursorBlink)
  const bellTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  bellModeRef.current = bellMode
  cursorStyleRef.current = cursorStyle
  cursorBlinkRef.current = cursorBlink

  useEffect(() => {
    const host = hostRef.current
    if (!host) {
      return
    }

    const term = new Terminal({
      cursorBlink,
      cursorStyle,
      convertEol: false,
      scrollback: scrollbackLines,
      fontSize: fontSizePx,
      fontFamily: terminalFontStack(fontFamily),
      allowProposedApi: true,
      theme: xtermThemeFromHost(host)
    })
    const fit = new FitAddon()
    const search = new SearchAddon()
    const unicode = new Unicode11Addon()
    term.loadAddon(fit)
    term.loadAddon(search)
    term.loadAddon(unicode)
    term.loadAddon(new WebLinksAddon())
    term.unicode.activeVersion = '11'
    term.open(host)
    if (bellLineRef.current) {
      host.appendChild(bellLineRef.current)
    }
    fit.fit()
    if (active) {
      term.focus()
    }

    const applyCursor = (): void => {
      term.options.cursorStyle = cursorStyleRef.current
      term.options.cursorBlink = cursorBlinkRef.current
    }

    termRef.current = term
    fitRef.current = fit
    searchRef.current = search
    registerWriter(tabId, (data) => {
      term.write(data)
      applyCursor()
    })
    registerSearch(tabId, {
      findPrevious: (needle, options) => {
        const opts = toXtermSearchOptions(options.caseSensitive)
        if (options.fromEnd) {
          term.clearSelection()
          search.clearDecorations()
        }
        return search.findPrevious(needle, opts)
      },
      findNext: (needle, options) => {
        const opts = toXtermSearchOptions(options.caseSensitive)
        if (options.fromEnd) {
          term.clearSelection()
          search.clearDecorations()
        }
        return search.findNext(needle, opts)
      },
      clear: () => {
        search.clearDecorations()
        term.clearSelection()
      }
    })

    const dataDisp = term.onData((data) => onData(tabId, data))
    const cursorSeqDisp = term.parser.registerCsiHandler(
      { intermediates: DECSCUSR_INTERMEDIATE, final: DECSCUSR_FINAL },
      () => true
    )

    const clearBellFlash = (): void => {
      delete host.dataset.bellFlash
      if (bellTimerRef.current) {
        clearTimeout(bellTimerRef.current)
        bellTimerRef.current = null
      }
    }

    const flashBell = (kind: 'window' | 'line'): void => {
      if (kind === 'line' && bellLineRef.current) {
        positionBellLine(host, term, bellLineRef.current)
      }
      host.dataset.bellFlash = kind
      if (bellTimerRef.current) {
        clearTimeout(bellTimerRef.current)
      }
      bellTimerRef.current = setTimeout(() => {
        delete host.dataset.bellFlash
        bellTimerRef.current = null
      }, BELL_FLASH_MS)
    }

    const bellDisp = term.onBell(() => {
      const mode = bellModeRef.current
      if (mode === BELL_MODE_SYSTEM) {
        void window.wassh.beep()
        return
      }
      if (mode === BELL_MODE_INVERT_WINDOW) {
        flashBell('window')
        return
      }
      if (mode === BELL_MODE_INVERT_LINE) {
        flashBell('line')
      }
    })

    const copySelection = (): void => {
      const sel = term.getSelection()
      if (sel) {
        void navigator.clipboard.writeText(sel)
      }
    }

    const pasteClipboard = (): void => {
      void navigator.clipboard.readText().then((text) => {
        if (text) {
          onData(tabId, text)
        }
      })
    }

    // PuTTY: finishing a left-button selection copies; right-click pastes
    const onMouseUp = (ev: MouseEvent): void => {
      if (ev.button === 0) {
        copySelection()
      }
    }
    const onContextMenu = (ev: MouseEvent): void => {
      ev.preventDefault()
      ev.stopPropagation()
      pasteClipboard()
    }
    host.addEventListener('mouseup', onMouseUp)
    host.addEventListener('contextmenu', onContextMenu)

    term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== 'keydown') {
        return true
      }
      // Let Alt+F4 / Ctrl+F4 reach the app / window manager (xterm must not preventDefault)
      if (
        !ev.metaKey &&
        ev.key === TAB_CLOSE_KEY &&
        ((ev.altKey && !ev.ctrlKey) || (ev.ctrlKey && !ev.altKey && !ev.shiftKey))
      ) {
        return false
      }
      if (ev.ctrlKey && !ev.altKey && !ev.metaKey && ev.key === TAB_CYCLE_KEY) {
        return false
      }
      // Ctrl/Cmd+Shift+F opens find; plain Ctrl+F must reach the remote program.
      if (
        (ev.ctrlKey || ev.metaKey) &&
        ev.shiftKey &&
        !ev.altKey &&
        ev.key.toLowerCase() === FIND_KEY
      ) {
        return false
      }
      if (
        (ev.ctrlKey || ev.metaKey) &&
        ev.shiftKey &&
        !ev.altKey &&
        ev.key.toLowerCase() === REOPEN_CLOSED_TAB_KEY
      ) {
        return false
      }
      return true
    })

    let resizeTimer: ReturnType<typeof setTimeout> | null = null
    const notifyResize = (): void => {
      fit.fit()
      onResize(tabId, term.cols, term.rows)
    }
    const ro = new ResizeObserver(() => {
      if (resizeTimer) {
        clearTimeout(resizeTimer)
      }
      resizeTimer = setTimeout(() => {
        resizeTimer = null
        notifyResize()
      }, RESIZE_DEBOUNCE_MS)
    })
    ro.observe(host)
    notifyResize()

    return () => {
      dataDisp.dispose()
      cursorSeqDisp.dispose()
      bellDisp.dispose()
      clearBellFlash()
      host.removeEventListener('mouseup', onMouseUp)
      host.removeEventListener('contextmenu', onContextMenu)
      ro.disconnect()
      if (resizeTimer) {
        clearTimeout(resizeTimer)
      }
      unregisterWriter(tabId)
      unregisterSearch(tabId)
      term.dispose()
      termRef.current = null
      fitRef.current = null
      searchRef.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabId])

  useEffect(() => {
    const term = termRef.current
    if (!term) {
      return
    }
    term.options.scrollback = scrollbackLines
    term.options.fontSize = fontSizePx
    term.options.fontFamily = terminalFontStack(fontFamily)
    fitRef.current?.fit()
    onResize(tabId, term.cols, term.rows)
  }, [scrollbackLines, fontSizePx, fontFamily, tabId, onResize])

  useEffect(() => {
    const term = termRef.current
    if (!term) {
      return
    }
    term.options.cursorStyle = cursorStyle
    term.options.cursorBlink = cursorBlink
  }, [cursorStyle, cursorBlink])

  useEffect(() => {
    const term = termRef.current
    const host = hostRef.current
    if (!term || !host) {
      return
    }
    term.options.theme = xtermThemeFromHost(host)
  }, [termBackground, termForeground, settings.theme])

  useEffect(() => {
    if (!active) {
      return
    }
    fitRef.current?.fit()
    const term = termRef.current
    if (term) {
      onResize(tabId, term.cols, term.rows)
    }
    // Defer past display:none → visible and past the control that activated this tab
    // (tab bar, Connect, host menu) so xterm can accept focus.
    const frame = requestAnimationFrame(() => {
      termRef.current?.focus()
    })
    return () => cancelAnimationFrame(frame)
  }, [active, tabId, onResize])

  useEffect(() => {
    if (!active) {
      return
    }
    // Defer past the overlay that released focus so xterm can accept it.
    const frame = requestAnimationFrame(() => {
      termRef.current?.focus()
    })
    return () => cancelAnimationFrame(frame)
  }, [focusNonce, active])

  const [dragActive, setDragActive] = useState(false)
  const dragDepthRef = useRef(0)

  const handleDragEnter = (e: ReactDragEvent<HTMLDivElement>): void => {
    if (!dropEnabled) {
      return
    }
    e.preventDefault()
    dragDepthRef.current += 1
    setDragActive(true)
  }

  const handleDragOver = (e: ReactDragEvent<HTMLDivElement>): void => {
    if (!dropEnabled) {
      return
    }
    e.preventDefault()
    e.dataTransfer.dropEffect = 'copy'
  }

  const handleDragLeave = (e: ReactDragEvent<HTMLDivElement>): void => {
    if (!dropEnabled) {
      return
    }
    e.preventDefault()
    dragDepthRef.current -= 1
    if (dragDepthRef.current <= 0) {
      dragDepthRef.current = 0
      setDragActive(false)
    }
  }

  const handleDrop = (e: ReactDragEvent<HTMLDivElement>): void => {
    e.preventDefault()
    dragDepthRef.current = 0
    setDragActive(false)
    if (!dropEnabled) {
      return
    }
    const files = e.dataTransfer.files
    if (files && files.length > 0) {
      onDropFiles(tabId, Array.from(files))
    }
  }

  return (
    <div
      className="terminal-host"
      ref={hostRef}
      data-term-bg={termBackground || undefined}
      data-term-fg={termForeground || undefined}
      style={sessionTerminalStyle(termBackground, termForeground)}
      onDragEnter={handleDragEnter}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      <div className="terminal-bell-line" ref={bellLineRef} />
      {findOpen && active ? (
        <TerminalSearchBar
          query={findQuery}
          caseSensitive={findCaseSensitive}
          focusNonce={findFocusNonce}
          found={findFound}
          onQueryChange={onFindQueryChange}
          onCaseSensitiveChange={onFindCaseSensitiveChange}
          onFindPrevious={onFindPrevious}
          onFindNext={onFindNext}
          onClose={onFindClose}
        />
      ) : null}
      {dragActive && dropEnabled ? (
        <div className="terminal-drop-overlay">
          <div className="terminal-drop-box">
            <div className="terminal-drop-icon">⬆</div>
            <div className="terminal-drop-title">Drop to upload over SFTP</div>
            <div className="terminal-drop-hint">Files are uploaded to the remote upload directory</div>
          </div>
        </div>
      ) : null}
      {dropUpload ? (
        <div className="drop-upload-banner">
          <div className="drop-upload-row">
            <span className="drop-upload-name">⬆ {dropUpload.name}</span>
            <span className="drop-upload-meta">{dropUpload.meta}</span>
          </div>
          <div className="drop-upload-track">
            <div className="drop-upload-fill" style={{ width: `${dropUpload.pct}%` }} />
          </div>
        </div>
      ) : null}
    </div>
  )
}
