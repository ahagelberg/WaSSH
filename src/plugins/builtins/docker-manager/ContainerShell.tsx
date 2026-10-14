import { useCallback, useEffect, useRef, useState, type ReactElement } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { PluginButton } from '@plugin-api/renderer'
import { DOCKER_SHELL_CHOICES } from './defaults'
import { isDockerMainMessage, type DockerContainerView } from './protocol'

interface Props {
  tabId: string
  pluginId: string
  container: DockerContainerView
  onClose: () => void
}

const CUSTOM_SHELL = 'custom'

/**
 * Interactive shell inside a container, backed by the Docker exec API. The
 * hijacked exec stream is bridged over plugin IPC: keystrokes and output are
 * small JSON messages, which is plenty for an interactive shell.
 */
export default function ContainerShell({ tabId, pluginId, container, onClose }: Props): ReactElement {
  const hostRef = useRef<HTMLDivElement | null>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const [choice, setChoice] = useState<string>(DOCKER_SHELL_CHOICES[0])
  const [customShell, setCustomShell] = useState('/bin/sh')
  const [nonce, setNonce] = useState(0)
  const [status, setStatus] = useState<'connecting' | 'open' | 'closed'>('connecting')
  const [error, setError] = useState<string | null>(null)

  const effectiveShell = choice === CUSTOM_SHELL ? customShell.trim() : choice

  const send = useCallback(
    (payload: Parameters<typeof window.wassh.sendPluginMessage>[2]): Promise<unknown> =>
      window.wassh.sendPluginMessage(tabId, pluginId, payload),
    [tabId, pluginId]
  )

  // Output and lifecycle events from the main process.
  useEffect(() => {
    return window.wassh.onPluginMessage((event) => {
      if (event.tabId !== tabId || event.pluginId !== pluginId) {
        return
      }
      if (!isDockerMainMessage(event.payload)) {
        return
      }
      const message = event.payload
      if (message.type === 'execOpened') {
        setStatus('open')
        setError(null)
        return
      }
      if (message.type === 'execData') {
        terminalRef.current?.write(message.data)
        return
      }
      if (message.type === 'execClosed') {
        setStatus('closed')
        if (message.error) {
          setError(message.error)
        }
        terminalRef.current?.write(
          `\r\n\x1b[90m[session ${message.error ? 'failed' : 'closed'}]\x1b[0m\r\n`
        )
      }
    })
  }, [tabId, pluginId])

  // Terminal setup runs once per mounted panel.
  useEffect(() => {
    const host = hostRef.current
    if (!host) {
      return
    }
    const term = new Terminal({
      convertEol: false,
      fontSize: 13,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      scrollback: 5000,
      cursorBlink: true
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(host)
    try {
      fit.fit()
    } catch {
      /* container not laid out yet */
    }
    terminalRef.current = term
    fitRef.current = fit

    const disposable = term.onData((data) => {
      void send({ type: 'execInput', data })
    })

    return () => {
      disposable.dispose()
      term.dispose()
      terminalRef.current = null
      fitRef.current = null
      void send({ type: 'execClose' })
    }
  }, [send])

  // (Re)start the exec whenever the effective shell changes or a reconnect is asked for.
  useEffect(() => {
    if (!effectiveShell) {
      return
    }
    const term = terminalRef.current
    setStatus('connecting')
    setError(null)
    void send({
      type: 'execOpen',
      id: container.id,
      shell: effectiveShell,
      cols: term?.cols ?? 80,
      rows: term?.rows ?? 24
    })
    return () => {
      void send({ type: 'execClose' })
    }
  }, [container.id, effectiveShell, nonce, send])

  // Keep the remote PTY in step with the panel size.
  useEffect(() => {
    const host = hostRef.current
    if (!host) {
      return
    }
    const observer = new ResizeObserver(() => {
      const fit = fitRef.current
      const term = terminalRef.current
      if (!fit || !term) {
        return
      }
      try {
        fit.fit()
      } catch {
        return
      }
      void send({ type: 'execResize', cols: term.cols, rows: term.rows })
    })
    observer.observe(host)
    return () => observer.disconnect()
  }, [send])

  return (
    <div className="docker-overlay" role="dialog" aria-modal="true" aria-label="Container shell">
      <div className="plugin-ui-toolbar docker-overlay-head">
        <strong className="docker-overlay-title">Shell</strong>
        <span className="plugin-ui-muted">{container.name || container.shortId}</span>
        <span
          className={`docker-shell-status docker-shell-status-${status}`}
          title={status === 'open' ? 'Connected' : status === 'connecting' ? 'Connecting' : 'Disconnected'}
        >
          {status}
        </span>
        <div className="plugin-ui-spacer" />
        <select
          className="plugin-ui-select"
          value={choice}
          aria-label="Shell"
          onChange={(event) => {
            const value = event.target.value
            setChoice(value)
            if (value === CUSTOM_SHELL && !customShell) {
              setCustomShell('/bin/sh')
            }
          }}
        >
          {DOCKER_SHELL_CHOICES.map((shellChoice) => (
            <option key={shellChoice} value={shellChoice}>
              {shellChoice}
            </option>
          ))}
          <option value={CUSTOM_SHELL}>Custom…</option>
        </select>
        {choice === CUSTOM_SHELL ? (
          <input
            className="plugin-ui-input docker-shell-input"
            value={customShell}
            aria-label="Custom shell command"
            onChange={(event) => setCustomShell(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                setNonce((value) => value + 1)
              }
            }}
          />
        ) : null}
        <PluginButton compact disabled={!effectiveShell} onClick={() => setNonce((value) => value + 1)}>
          Reconnect
        </PluginButton>
        <PluginButton compact onClick={onClose}>
          Close
        </PluginButton>
      </div>

      {error ? <div className="plugin-ui-error docker-banner">{error}</div> : null}

      <div className="docker-terminal" ref={hostRef} />
    </div>
  )
}
