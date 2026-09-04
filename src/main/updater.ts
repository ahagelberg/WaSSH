import { app, BrowserWindow, dialog } from 'electron'
import { autoUpdater } from 'electron-updater'
import { APP_NAME, APP_VERSION } from '../shared/version'

/** Wait this long after startup before asking GitHub for a newer release. */
const STARTUP_UPDATE_CHECK_DELAY_MS = 10_000

/** Index of the "Restart now" button in the restart prompt. */
const RESTART_NOW_BUTTON_INDEX = 0

/** Index of the "Later" (cancel) button in the restart prompt. */
const LATER_BUTTON_INDEX = 1

/** Index of the "Download and install" button in the update-available prompt. */
const DOWNLOAD_AND_INSTALL_BUTTON_INDEX = 0

/** Index of the "Not now" (dismiss) button in the update-available prompt. */
const NOT_NOW_BUTTON_INDEX = 1

let downloadedVersion: string | null = null
let declinedVersion: string | null = null
let checkInProgress = false
let manualCheckPending = false
let getWindow: () => BrowserWindow | null = () => null

/** Auto-update only works for NSIS-installed builds. */
function canAutoUpdate(): boolean {
  if (!app.isPackaged) {
    return false
  }
  // Portable and win-unpacked builds run from an unpacked dir; only an install can update itself.
  if (process.env.PORTABLE_EXECUTABLE_DIR || app.getPath('exe').includes('win-unpacked')) {
    return false
  }
  return true
}

function parentWindow(): BrowserWindow | null {
  const win = getWindow()
  return win && !win.isDestroyed() ? win : null
}

function messageBox(options: Electron.MessageBoxOptions): Promise<Electron.MessageBoxReturnValue> {
  const win = parentWindow()
  return win ? dialog.showMessageBox(win, options) : dialog.showMessageBox(options)
}

async function showInfoDialog(message: string, detail: string): Promise<void> {
  await messageBox({ type: 'info', message, detail, buttons: ['OK'] })
}

function restartPrompt(version: string): Electron.MessageBoxOptions {
  return {
    type: 'info',
    title: 'Update ready',
    message: `${APP_NAME} ${version} has been downloaded.`,
    detail: 'Restart now to finish installing the update?',
    buttons: ['Restart now', 'Later'],
    defaultId: RESTART_NOW_BUTTON_INDEX,
    cancelId: LATER_BUTTON_INDEX
  }
}

async function promptRestart(version: string): Promise<void> {
  const { response } = await messageBox(restartPrompt(version))
  if (response === RESTART_NOW_BUTTON_INDEX) {
    autoUpdater.quitAndInstall()
  }
}

async function checkNow(): Promise<void> {
  if (checkInProgress) {
    return
  }
  checkInProgress = true
  try {
    await autoUpdater.checkForUpdates()
  } catch (err) {
    console.error('Update check failed:', err)
    if (manualCheckPending) {
      manualCheckPending = false
      void showInfoDialog(
        'Update check failed',
        'Could not reach the update server. Check your connection and try again.'
      )
    }
  } finally {
    checkInProgress = false
  }
}

export function setupAutoUpdater(getWindowFn: () => BrowserWindow | null): void {
  if (!canAutoUpdate()) {
    return
  }
  getWindow = getWindowFn
  autoUpdater.logger = console
  autoUpdater.autoDownload = false
  autoUpdater.autoInstallOnAppQuit = false

  autoUpdater.on('update-available', async (info) => {
    // Ask before downloading. Declined updates are only re-offered on an explicit
    // "Check for Updates" so the startup check stays silent after a decline.
    const isManualCheck = manualCheckPending
    manualCheckPending = false
    if (!isManualCheck && declinedVersion === info.version) {
      return
    }

    const { response } = await messageBox({
      type: 'info',
      title: 'Update available',
      message: `${APP_NAME} ${info.version} is available.`,
      detail: 'Download and install it now?',
      buttons: ['Download and install', 'Not now'],
      defaultId: DOWNLOAD_AND_INSTALL_BUTTON_INDEX,
      cancelId: NOT_NOW_BUTTON_INDEX
    })
    if (response !== DOWNLOAD_AND_INSTALL_BUTTON_INDEX) {
      declinedVersion = info.version
      return
    }
    declinedVersion = null

    try {
      await autoUpdater.downloadUpdate()
    } catch (err) {
      console.error('Update download failed:', err)
      void showInfoDialog(
        'Download failed',
        `Version ${info.version} could not be downloaded. Check your connection and try again.`
      )
    }
  })

  autoUpdater.on('update-not-available', () => {
    if (manualCheckPending) {
      manualCheckPending = false
      void showInfoDialog('No update available', `You are on the latest version (${APP_VERSION}).`)
    }
  })

  autoUpdater.on('update-downloaded', (info) => {
    downloadedVersion = info.version
    manualCheckPending = false
    void promptRestart(info.version)
  })

  autoUpdater.on('error', (err) => {
    console.error('Auto-update error:', err)
  })

  setTimeout(() => {
    void checkNow()
  }, STARTUP_UPDATE_CHECK_DELAY_MS)
}

export function checkForUpdatesManually(): void {
  if (!canAutoUpdate()) {
    void showInfoDialog(
      'Updates not available here',
      'Automatic updates work in the installed version of the app. This looks like a development or portable build.'
    )
    return
  }
  if (downloadedVersion) {
    void promptRestart(downloadedVersion)
    return
  }
  manualCheckPending = true
  void checkNow()
}
