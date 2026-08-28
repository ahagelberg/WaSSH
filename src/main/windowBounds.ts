import { BrowserWindow, screen } from 'electron'
import {
  DEFAULT_WINDOW_HEIGHT,
  DEFAULT_WINDOW_MIN_HEIGHT,
  DEFAULT_WINDOW_MIN_WIDTH,
  DEFAULT_WINDOW_WIDTH,
  WINDOW_BOUNDS_DEBOUNCE_MS,
  WINDOW_VISIBLE_MARGIN_PX,
  WindowBounds
} from '../shared/types'
import { SettingsStore } from './store/sessionStore'

export function restoreWindowBounds(saved: WindowBounds | null): {
  x?: number
  y?: number
  width: number
  height: number
  maximized: boolean
} {
  if (!saved) {
    return {
      width: DEFAULT_WINDOW_WIDTH,
      height: DEFAULT_WINDOW_HEIGHT,
      maximized: false
    }
  }
  const width = Math.max(saved.width, DEFAULT_WINDOW_MIN_WIDTH)
  const height = Math.max(saved.height, DEFAULT_WINDOW_MIN_HEIGHT)
  if (!Number.isFinite(saved.x) || !Number.isFinite(saved.y)) {
    return { width, height, maximized: saved.maximized }
  }
  const probe = { x: saved.x, y: saved.y, width, height }
  const display = screen.getDisplayMatching(probe)
  const area = display.workArea
  const margin = WINDOW_VISIBLE_MARGIN_PX
  const visible =
    saved.x + width > area.x + margin &&
    saved.x < area.x + area.width - margin &&
    saved.y + height > area.y + margin &&
    saved.y < area.y + area.height - margin
  if (!visible) {
    return { width, height, maximized: saved.maximized }
  }
  return { x: saved.x, y: saved.y, width, height, maximized: saved.maximized }
}

export function readWindowBounds(win: BrowserWindow): WindowBounds {
  const maximized = win.isMaximized()
  const bounds = maximized ? win.getNormalBounds() : win.getBounds()
  return {
    x: bounds.x,
    y: bounds.y,
    width: bounds.width,
    height: bounds.height,
    maximized
  }
}

export function attachWindowBoundsPersistence(
  win: BrowserWindow,
  settingsStore: SettingsStore
): void {
  let timer: ReturnType<typeof setTimeout> | null = null

  const persist = (): void => {
    if (win.isDestroyed()) {
      return
    }
    settingsStore.set({ windowBounds: readWindowBounds(win) })
  }

  const schedulePersist = (): void => {
    if (timer) {
      clearTimeout(timer)
    }
    timer = setTimeout(() => {
      timer = null
      persist()
    }, WINDOW_BOUNDS_DEBOUNCE_MS)
  }

  win.on('resize', schedulePersist)
  win.on('move', schedulePersist)
  win.on('maximize', persist)
  win.on('unmaximize', schedulePersist)
  win.on('close', persist)
}
