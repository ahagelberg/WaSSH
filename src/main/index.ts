import { app, BrowserWindow, shell } from 'electron'
import { join } from 'path'
import {
  DEFAULT_WINDOW_MIN_HEIGHT,
  DEFAULT_WINDOW_MIN_WIDTH
} from '../shared/types'
import { registerIpc } from './ipc/handlers'
import { SessionManager } from './ssh/SessionManager'
import { CredentialVault } from './store/credentialVault'
import {
  KnownHostsStore,
  SessionStore,
  SettingsStore,
  TabStore
} from './store/sessionStore'
import { attachWindowBoundsPersistence, restoreWindowBounds } from './windowBounds'

let mainWindow: BrowserWindow | null = null
let sessions: SessionManager | null = null
let settingsStore: SettingsStore | null = null

function getWindow(): BrowserWindow | null {
  return mainWindow
}

function createWindow(): void {
  const saved = settingsStore?.get().windowBounds ?? null
  const restored = restoreWindowBounds(saved)

  mainWindow = new BrowserWindow({
    x: restored.x,
    y: restored.y,
    width: restored.width,
    height: restored.height,
    minWidth: DEFAULT_WINDOW_MIN_WIDTH,
    minHeight: DEFAULT_WINDOW_MIN_HEIGHT,
    show: false,
    title: 'WaSSH',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  })

  if (settingsStore) {
    attachWindowBoundsPersistence(mainWindow, settingsStore)
  }

  mainWindow.on('ready-to-show', () => {
    if (restored.maximized) {
      mainWindow?.maximize()
    }
    mainWindow?.show()
  })

  mainWindow.webContents.setWindowOpenHandler((details) => {
    void shell.openExternal(details.url)
    return { action: 'deny' }
  })

  if (process.env.ELECTRON_RENDERER_URL) {
    void mainWindow.loadURL(process.env.ELECTRON_RENDERER_URL)
  } else {
    void mainWindow.loadFile(join(__dirname, '../renderer/index.html'))
  }
}

app.whenReady().then(() => {
  const vault = new CredentialVault()
  const sessionStore = new SessionStore()
  const tabStore = new TabStore()
  settingsStore = new SettingsStore()
  const knownHosts = new KnownHostsStore()
  sessions = new SessionManager(
    vault,
    knownHosts,
    sessionStore,
    settingsStore,
    getWindow
  )
  registerIpc(vault, sessionStore, tabStore, settingsStore, knownHosts, sessions, getWindow)
  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow()
    }
  })
})

app.on('window-all-closed', () => {
  sessions?.disposeAll()
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('before-quit', () => {
  sessions?.disposeAll()
})
