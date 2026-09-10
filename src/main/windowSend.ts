import type { BrowserWindow } from 'electron'

/**
 * Send an IPC message to the main window, silently skipping if it doesn't
 * exist or has already been destroyed (e.g. during shutdown).
 */
export function sendToWindow(
  getWindow: () => BrowserWindow | null,
  channel: string,
  ...args: unknown[]
): void {
  const win = getWindow()
  if (!win || win.isDestroyed()) {
    return
  }
  win.webContents.send(channel, ...args)
}
