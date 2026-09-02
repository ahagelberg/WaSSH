import { useEffect, useRef, useState } from 'react'
import { PLUGIN_ID_SCRATCHPAD } from '@shared/plugins'
import type { PluginViewProps } from '../registry'

/** Debounce writes to plugin-scratchpad.json (ms) */
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

export default function ScratchpadView(_props: PluginViewProps) {
  const [content, setContent] = useState('')
  const [ready, setReady] = useState(false)
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    let cancelled = false
    void window.wassh.getPluginData(PLUGIN_ID_SCRATCHPAD).then((data) => {
      if (cancelled) {
        return
      }
      setContent(contentFromData(data))
      setReady(true)
    })
    return () => {
      cancelled = true
      if (saveTimer.current) {
        clearTimeout(saveTimer.current)
        saveTimer.current = null
      }
    }
  }, [])

  const contentRef = useRef(content)
  contentRef.current = content

  useEffect(() => {
    return () => {
      if (!ready) {
        return
      }
      void window.wassh.setPluginData(PLUGIN_ID_SCRATCHPAD, {
        content: contentRef.current
      } satisfies ScratchpadData)
    }
  }, [ready])

  const persist = (value: string): void => {
    if (saveTimer.current) {
      clearTimeout(saveTimer.current)
    }
    saveTimer.current = setTimeout(() => {
      saveTimer.current = null
      void window.wassh.setPluginData(PLUGIN_ID_SCRATCHPAD, {
        content: value
      } satisfies ScratchpadData)
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
