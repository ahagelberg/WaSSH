import { useEffect, useRef, useState } from 'react'
import { PLUGIN_ID_SCRATCHPAD } from '@shared/plugins'
import type { PluginViewProps } from '../registry'

/** Debounce writes to the scratchpad plugin data file (ms) */
const SCRATCHPAD_SAVE_DEBOUNCE_MS = 400

interface ScratchpadData {
  content: string
}

function contentFromData(data: unknown): string {
  if (!data || typeof data !== 'object' || Array.isArray(data)) {
    return ''
  }
  const content = (data as ScratchpadData).content
  return typeof content === 'string' ? content : ''
}

/**
 * Storage scope for the notes: the saved host profile id, or a per-tab id for
 * sessions not bound to a saved host (quick connect / serial / telnet). Tabs
 * of the same saved host therefore share one scratchpad; other sessions get
 * their own.
 */
function scopeIdFor(hostId: string | null, tabId: string): string {
  return hostId || `tab:${tabId}`
}

export default function ScratchpadView({ tabId, hostId }: PluginViewProps) {
  const scope = scopeIdFor(hostId, tabId)
  const [content, setContent] = useState('')
  const [ready, setReady] = useState(false)
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  /** Scope the currently mounted editor belongs to (for flushing on change) */
  const scopeRef = useRef(scope)
  /** Whether the editor finished loading before it unmounts */
  const readyRef = useRef(false)
  /** Text the scope had when loaded; writes only happen when it changed */
  const baselineRef = useRef('')

  const contentRef = useRef(content)
  contentRef.current = content

  useEffect(() => {
    scopeRef.current = scope
    readyRef.current = false
    setContent('')
    setReady(false)
    let cancelled = false
    void (async () => {
      const stored = await window.wassh.getPluginData(PLUGIN_ID_SCRATCHPAD, scope)
      let text = contentFromData(stored)
      if (stored === null) {
        // One-time adoption of the pre-scoping shared notes: the first scope
        // without notes of its own claims and clears them, so they can never
        // appear (or be duplicated) on a different host's pad.
        const legacy = await window.wassh.getPluginData(PLUGIN_ID_SCRATCHPAD)
        const legacyText = contentFromData(legacy)
        if (legacyText) {
          text = legacyText
          await window.wassh.setPluginData(
            PLUGIN_ID_SCRATCHPAD,
            { content: legacyText } satisfies ScratchpadData,
            scope
          )
          await window.wassh.setPluginData(PLUGIN_ID_SCRATCHPAD, null)
        }
      }
      if (cancelled) {
        return
      }
      baselineRef.current = text
      contentRef.current = text
      setContent(text)
      setReady(true)
      readyRef.current = true
    })()
    return () => {
      cancelled = true
      if (saveTimer.current) {
        clearTimeout(saveTimer.current)
        saveTimer.current = null
      }
      // Flush edits made to the scope that is being left (tab close / host switch).
      const leaving = scopeRef.current
      if (readyRef.current && leaving && contentRef.current !== baselineRef.current) {
        void window.wassh.setPluginData(
          PLUGIN_ID_SCRATCHPAD,
          { content: contentRef.current } satisfies ScratchpadData,
          leaving
        )
      }
    }
  }, [scope])

  const persist = (value: string): void => {
    if (saveTimer.current) {
      clearTimeout(saveTimer.current)
    }
    saveTimer.current = setTimeout(() => {
      saveTimer.current = null
      void window.wassh.setPluginData(
        PLUGIN_ID_SCRATCHPAD,
        { content: value } satisfies ScratchpadData,
        scope
      )
    }, SCRATCHPAD_SAVE_DEBOUNCE_MS)
  }

  return (
    <div className="plugin-panel plugin-scratchpad">
      <textarea
        className="plugin-scratchpad-input"
        value={content}
        disabled={!ready}
        placeholder="Notes and scratch text…"
        onChange={(e) => {
          const value = e.target.value
          setContent(value)
          persist(value)
        }}
      />
    </div>
  )
}
