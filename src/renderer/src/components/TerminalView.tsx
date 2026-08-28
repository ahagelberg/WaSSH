import { useEffect, useRef } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebLinksAddon } from '@xterm/addon-web-links'
import type { AppSettings } from '@shared/types'

/** Debounce resize observer notifications */
const RESIZE_DEBOUNCE_MS = 50

interface Props {
  tabId: string
  active: boolean
  settings: AppSettings
  onData: (tabId: string, data: string) => void
  onResize: (tabId: string, cols: number, rows: number) => void
  registerWriter: (tabId: string, write: (data: string) => void) => void
  unregisterWriter: (tabId: string) => void
}

export default function TerminalView({
  tabId,
  active,
  settings,
  onData,
  onResize,
  registerWriter,
  unregisterWriter
}: Props) {
  const hostRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)

  useEffect(() => {
    const host = hostRef.current
    if (!host) {
      return
    }

    const term = new Terminal({
      cursorBlink: true,
      convertEol: false,
      scrollback: settings.scrollbackLines,
      fontSize: settings.fontSizePx,
      fontFamily: settings.fontFamily,
      allowProposedApi: true,
      theme: {
        background: '#0c0d0f',
        foreground: '#e8eaed',
        cursor: '#e8eaed',
        selectionBackground: '#3d8bfd66'
      }
    })
    const fit = new FitAddon()
    const unicode = new Unicode11Addon()
    term.loadAddon(fit)
    term.loadAddon(unicode)
    term.loadAddon(new WebLinksAddon())
    term.unicode.activeVersion = '11'
    term.open(host)
    fit.fit()
    term.focus()

    termRef.current = term
    fitRef.current = fit
    registerWriter(tabId, (data) => term.write(data))

    const dataDisp = term.onData((data) => onData(tabId, data))

    term.attachCustomKeyEventHandler((ev) => {
      const mod = ev.ctrlKey && ev.shiftKey
      if (mod && ev.key.toLowerCase() === 'c') {
        const sel = term.getSelection()
        if (sel) {
          void navigator.clipboard.writeText(sel)
          return false
        }
      }
      if (mod && ev.key.toLowerCase() === 'v') {
        void navigator.clipboard.readText().then((text) => {
          if (text) {
            onData(tabId, text)
          }
        })
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
      ro.disconnect()
      if (resizeTimer) {
        clearTimeout(resizeTimer)
      }
      unregisterWriter(tabId)
      term.dispose()
      termRef.current = null
      fitRef.current = null
    }
    // mount once per tab
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabId])

  useEffect(() => {
    const term = termRef.current
    if (!term) {
      return
    }
    term.options.scrollback = settings.scrollbackLines
    term.options.fontSize = settings.fontSizePx
    term.options.fontFamily = settings.fontFamily
    fitRef.current?.fit()
    onResize(tabId, term.cols, term.rows)
  }, [settings.scrollbackLines, settings.fontSizePx, settings.fontFamily, tabId, onResize])

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

  return <div className="terminal-host" ref={hostRef} />
}
