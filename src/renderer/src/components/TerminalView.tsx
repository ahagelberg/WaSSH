import { useEffect, useRef } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebLinksAddon } from '@xterm/addon-web-links'
import {
  BELL_MODE_INVERT_LINE,
  BELL_MODE_INVERT_WINDOW,
  BELL_MODE_SYSTEM,
  TAB_CYCLE_KEY,
  type AppSettings,
  type BellMode,
  type CursorStyle
} from '@shared/types'
import { terminalFontStack } from '@shared/connection'

/** Debounce resize observer notifications */
const RESIZE_DEBOUNCE_MS = 50

/** Visual BEL invert duration */
const BELL_FLASH_MS = 200

/** DECSCUSR: CSI Ps SP q */
const DECSCUSR_INTERMEDIATE = ' '
const DECSCUSR_FINAL = 'q'

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
  onData: (tabId: string, data: string) => void
  onResize: (tabId: string, cols: number, rows: number) => void
  registerWriter: (tabId: string, write: (data: string) => void) => void
  unregisterWriter: (tabId: string) => void
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
  onData,
  onResize,
  registerWriter,
  unregisterWriter
}: Props) {
  const hostRef = useRef<HTMLDivElement>(null)
  const bellLineRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
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
    const unicode = new Unicode11Addon()
    term.loadAddon(fit)
    term.loadAddon(unicode)
    term.loadAddon(new WebLinksAddon())
    term.unicode.activeVersion = '11'
    term.open(host)
    if (bellLineRef.current) {
      host.appendChild(bellLineRef.current)
    }
    fit.fit()
    term.focus()

    const applyCursor = (): void => {
      term.options.cursorStyle = cursorStyleRef.current
      term.options.cursorBlink = cursorBlinkRef.current
    }

    termRef.current = term
    fitRef.current = fit
    registerWriter(tabId, (data) => {
      term.write(data)
      applyCursor()
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
      // Let Alt+F4 reach the window manager (xterm must not preventDefault)
      if (ev.altKey && !ev.ctrlKey && !ev.metaKey && ev.key === 'F4') {
        return false
      }
      if (ev.ctrlKey && !ev.altKey && !ev.metaKey && ev.key === TAB_CYCLE_KEY) {
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
      term.dispose()
      termRef.current = null
      fitRef.current = null
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
    termRef.current?.focus()
    const term = termRef.current
    if (term) {
      onResize(tabId, term.cols, term.rows)
    }
  }, [active, tabId, onResize])

  return (
    <div
      className="terminal-host"
      ref={hostRef}
      data-term-bg={termBackground || undefined}
      data-term-fg={termForeground || undefined}
    >
      <div className="terminal-bell-line" ref={bellLineRef} />
    </div>
  )
}
