import { useEffect } from 'react'
import type { PluginMacroButton } from '@shared/plugins'
import type { PluginViewProps } from '../registry'

function parseHotkey(hotkey: string): { ctrl: boolean; shift: boolean; alt: boolean; key: string } | null {
  const parts = hotkey
    .split('+')
    .map((p) => p.trim())
    .filter(Boolean)
  if (parts.length === 0) {
    return null
  }
  const key = parts[parts.length - 1]
  const mods = parts.slice(0, -1).map((p) => p.toLowerCase())
  return {
    ctrl: mods.includes('ctrl') || mods.includes('control') || mods.includes('cmd') || mods.includes('meta'),
    shift: mods.includes('shift'),
    alt: mods.includes('alt'),
    key: key.length === 1 ? key.toLowerCase() : key
  }
}

function matchesHotkey(e: KeyboardEvent, hotkey: string): boolean {
  const parsed = parseHotkey(hotkey)
  if (!parsed) {
    return false
  }
  const eventKey = e.key.length === 1 ? e.key.toLowerCase() : e.key
  return (
    Boolean(e.ctrlKey || e.metaKey) === parsed.ctrl &&
    e.shiftKey === parsed.shift &&
    e.altKey === parsed.alt &&
    eventKey === parsed.key
  )
}

export default function MacroPadView({
  tabId,
  pluginId,
  settings,
  activeTab
}: PluginViewProps & { activeTab?: boolean }) {
  const buttons = Array.isArray(settings.buttons)
    ? (settings.buttons as PluginMacroButton[])
    : []

  const send = (text: string): void => {
    void window.wassh.sendPluginMessage(tabId, pluginId, { type: 'send', text })
  }

  useEffect(() => {
    if (activeTab === false) {
      return
    }
    const onKeyDown = (e: KeyboardEvent): void => {
      for (const btn of buttons) {
        if (!btn.hotkey) {
          continue
        }
        if (matchesHotkey(e, btn.hotkey)) {
          e.preventDefault()
          send(btn.text)
          return
        }
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [buttons, tabId, pluginId, activeTab])

  return (
    <div
      className="plugin-panel plugin-macro-pad"
      onMouseDown={(e) => {
        // Keep terminal focus when interacting with the pad.
        if ((e.target as HTMLElement).closest('button, input, select, textarea, a')) {
          return
        }
        e.preventDefault()
      }}
    >
      <div className="plugin-macro-grid">
        {buttons.map((btn) => (
          <button
            key={btn.id}
            type="button"
            className="plugin-macro-btn"
            title={btn.hotkey ? `${btn.label} (${btn.hotkey})` : btn.label}
            tabIndex={-1}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => send(btn.text)}
          >
            <span className="plugin-macro-label">{btn.label}</span>
            {btn.hotkey ? <span className="plugin-macro-hotkey">{btn.hotkey}</span> : null}
          </button>
        ))}
      </div>
    </div>
  )
}
