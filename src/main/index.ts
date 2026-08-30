import { app, BrowserWindow, Menu, powerMonitor, shell } from 'electron'
import { join } from 'path'
import {
  DEFAULT_THEME,
  DEFAULT_WINDOW_MIN_HEIGHT,
  DEFAULT_WINDOW_MIN_WIDTH,
  TAB_CLOSE_KEY,
  TAB_CYCLE_KEY,
  TAB_CYCLE_NEXT,
  TAB_CYCLE_PREV,
  THEME_WINDOW_BACKGROUND,
  WAKE_RECONNECT_DELAY_MS
} from '../shared/types'
import { APP_NAME } from '../shared/version'
import { applyChromeTheme, registerIpc } from './ipc/handlers'
import { SessionManager } from './ssh/SessionManager'
import { CredentialVault } from './store/credentialVault'
import {
  KnownHostsStore,
  SessionStore,
  SettingsStore,
  TabStore
} from './store/sessionStore'
import { attachWindowBoundsPersistence, restoreWindowBounds } from './windowBounds'
import { createPluginSystem } from './plugins/createPluginSystem'
import { PluginDataStore } from './store/pluginDataStore'

/** Accelerator for File > Preferences */
const PREFERENCES_ACCELERATOR = 'CommandOrControl+,'

/** Accelerator for Session > Reconnect */
const RECONNECT_ACCELERATOR = 'CommandOrControl+R'

/** Accelerator for Session > Reconnect All */
const RECONNECT_ALL_ACCELERATOR = 'CommandOrControl+Shift+R'

/** Network resets that happen on sleep/wake; must not crash the main process */
const TRANSIENT_NETWORK_ERROR_CODES = new Set([
  'ECONNRESET',
  'EPIPE',
  'ETIMEDOUT',
  'ENETRESET',
  'ECONNABORTED',
  'ENETUNREACH',
  'EHOSTUNREACH',
  'ENOTCONN',
  'ERR_STREAM_DESTROYED'
])

let wakeReconnectTimer: ReturnType<typeof setTimeout> | null = null

function errorCodeOf(err: unknown): string {
  if (!err || typeof err !== 'object' || !('code' in err)) {
    return ''
  }
  return String((err as { code?: unknown }).code)
}

function isTransientNetworkError(err: unknown): boolean {
  return TRANSIENT_NETWORK_ERROR_CODES.has(errorCodeOf(err))
}

function installProcessErrorGuards(): void {
  process.on('uncaughtException', (err) => {
    if (isTransientNetworkError(err)) {
      return
    }
    console.error(err)
  })
  process.on('unhandledRejection', (reason) => {
    if (isTransientNetworkError(reason)) {
      return
    }
    console.error(reason)
  })
}

function attachPowerMonitor(): void {
  powerMonitor.on('suspend', () => {
    if (wakeReconnectTimer) {
      clearTimeout(wakeReconnectTimer)
      wakeReconnectTimer = null
    }
    sessions?.prepareForSleep()
  })
  powerMonitor.on('resume', () => {
    if (wakeReconnectTimer) {
      clearTimeout(wakeReconnectTimer)
    }
    wakeReconnectTimer = setTimeout(() => {
      wakeReconnectTimer = null
      sessions?.reconnectOnWake()
    }, WAKE_RECONNECT_DELAY_MS)
  })
}

const ALLOWED_WEB_PERMISSIONS = new Set([
  'local-fonts',
  'clipboard-read',
  'clipboard-sanitized-write',
  'fullscreen'
])

let mainWindow: BrowserWindow | null = null
let sessions: SessionManager | null = null
let settingsStore: SettingsStore | null = null
let pluginSystem: ReturnType<typeof createPluginSystem> | null = null

function getWindow(): BrowserWindow | null {
  return mainWindow
}

function createWindow(): void {
  const saved = settingsStore?.get().windowBounds ?? null
  const restored = restoreWindowBounds(saved)
  const theme = settingsStore?.get().theme ?? DEFAULT_THEME
  applyChromeTheme(theme, null)

  mainWindow = new BrowserWindow({
    x: restored.x,
    y: restored.y,
    width: restored.width,
    height: restored.height,
    minWidth: DEFAULT_WINDOW_MIN_WIDTH,
    minHeight: DEFAULT_WINDOW_MIN_HEIGHT,
    backgroundColor: THEME_WINDOW_BACKGROUND[theme],
    show: false,
    title: 'WaSSH',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  })

  applyChromeTheme(theme, mainWindow)

  const sess = mainWindow.webContents.session
  sess.setPermissionCheckHandler((_wc, permission) => ALLOWED_WEB_PERMISSIONS.has(permission))
  sess.setPermissionRequestHandler((_wc, permission, callback) => {
    callback(ALLOWED_WEB_PERMISSIONS.has(permission))
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

  mainWindow.on('focus', () => {
    sessions?.reconnectOnFocus()
  })

  mainWindow.webContents.setWindowOpenHandler((details) => {
    void shell.openExternal(details.url)
    return { action: 'deny' }
  })

  mainWindow.webContents.on('before-input-event', (event, input) => {
    if (input.type !== 'keyDown' || !input.control || input.alt || input.meta) {
      return
    }
    if (input.key === TAB_CYCLE_KEY) {
      event.preventDefault()
      const delta = input.shift ? TAB_CYCLE_PREV : TAB_CYCLE_NEXT
      mainWindow?.webContents.send('tabs:cycle', delta)
      return
    }
    if (input.key === TAB_CLOSE_KEY && !input.shift) {
      event.preventDefault()
      mainWindow?.webContents.send('tabs:closeActive')
    }
  })

  if (process.env.ELECTRON_RENDERER_URL) {
    void mainWindow.loadURL(process.env.ELECTRON_RENDERER_URL)
  } else {
    void mainWindow.loadFile(join(__dirname, '../renderer/index.html'))
  }
}

function openAboutDialog(): void {
  getWindow()?.webContents.send('app:openAbout')
}

function sendToRenderer(channel: string): void {
  getWindow()?.webContents.send(channel)
}

function installAppMenu(): void {
  const isMac = process.platform === 'darwin'
  const aboutLabel = `About ${APP_NAME}`
  const template: Electron.MenuItemConstructorOptions[] = [
    ...(isMac
      ? [
          {
            label: APP_NAME,
            submenu: [
              {
                label: aboutLabel,
                click: () => openAboutDialog()
              },
              { type: 'separator' as const },
              { role: 'services' as const },
              { type: 'separator' as const },
              { role: 'hide' as const },
              { role: 'hideOthers' as const },
              { role: 'unhide' as const },
              { type: 'separator' as const },
              { role: 'quit' as const }
            ]
          }
        ]
      : []),
    {
      label: 'File',
      submenu: [
        {
          label: 'Preferences',
          accelerator: PREFERENCES_ACCELERATOR,
          click: () => {
            sendToRenderer('app:openPreferences')
          }
        },
        { type: 'separator' },
        isMac ? { role: 'close' } : { role: 'quit' }
      ]
    },
    { role: 'editMenu' },
    {
      label: 'Session',
      submenu: [
        {
          label: 'Reconnect',
          accelerator: RECONNECT_ACCELERATOR,
          click: () => sendToRenderer('session:reconnectActive')
        },
        {
          label: 'Reconnect All',
          accelerator: RECONNECT_ALL_ACCELERATOR,
          click: () => sendToRenderer('session:reconnectAll')
        }
      ]
    },
    {
      label: 'View',
      submenu: [
        { role: 'resetZoom' },
        { role: 'zoomIn' },
        { role: 'zoomOut' },
        { type: 'separator' },
        { role: 'togglefullscreen' },
        { type: 'separator' },
        { role: 'toggleDevTools' }
      ]
    },
    {
      role: 'help',
      submenu: [
        {
          label: aboutLabel,
          click: () => openAboutDialog()
        }
      ]
    }
  ]
  Menu.setApplicationMenu(Menu.buildFromTemplate(template))
}

installProcessErrorGuards()

app.whenReady().then(() => {
  const vault = new CredentialVault()
  const sessionStore = new SessionStore()
  const tabStore = new TabStore()
  settingsStore = new SettingsStore()
  sessionStore.migrateReconnectModes()
  const knownHosts = new KnownHostsStore()
  const pluginData = new PluginDataStore()
  sessions = new SessionManager(
    vault,
    knownHosts,
    sessionStore,
    getWindow
  )
  pluginSystem = createPluginSystem(settingsStore, sessionStore, sessions, getWindow, pluginData)
  registerIpc(
    vault,
    sessionStore,
    tabStore,
    settingsStore,
    knownHosts,
    sessions,
    getWindow,
    pluginSystem.host,
    pluginData
  )
  installAppMenu()
  attachPowerMonitor()
  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow()
    }
  })
})

app.on('window-all-closed', () => {
  pluginSystem?.dispose()
  sessions?.disposeAll()
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('before-quit', () => {
  pluginSystem?.dispose()
  sessions?.disposeAll()
})

