import { BrowserWindow, dialog, ipcMain, nativeTheme, shell } from 'electron'
import {
  AppSettings,
  AppTheme,
  ConnectRequest,
  HostKeyDecision,
  HostProfile,
  SavePasswordDecision,
  TabSnapshot,
  THEME_WINDOW_BACKGROUND
} from '../../shared/types'
import { CredentialVault } from '../store/credentialVault'
import {
  KnownHostsStore,
  SessionStore,
  SettingsStore,
  TabStore
} from '../store/sessionStore'
import { SessionManager } from '../ssh/SessionManager'
import { listSerialPorts } from '../serial/listSerialPorts'
import type { PluginHost } from '../plugins/PluginHost'
import { queuePluginRestore } from '../plugins/createPluginSystem'
import type { PluginDataStore } from '../store/pluginDataStore'

export function applyChromeTheme(theme: AppTheme, win: BrowserWindow | null): void {
  nativeTheme.themeSource = theme
  if (win && !win.isDestroyed()) {
    win.setBackgroundColor(THEME_WINDOW_BACKGROUND[theme])
  }
}

export function registerIpc(
  vault: CredentialVault,
  sessionStore: SessionStore,
  tabStore: TabStore,
  settingsStore: SettingsStore,
  knownHosts: KnownHostsStore,
  sessions: SessionManager,
  getWindow: () => BrowserWindow | null,
  pluginHost: PluginHost,
  pluginData: PluginDataStore
): void {
  ipcMain.handle('settings:get', () => settingsStore.get())
  ipcMain.handle('settings:set', async (_e, partial: Partial<AppSettings>) => {
    const prev = settingsStore.get()
    const next = settingsStore.set(partial)
    applyChromeTheme(next.theme, getWindow())
    if (partial.enabledPlugins) {
      await pluginHost.onEnabledPluginsChanged(prev.enabledPlugins, next.enabledPlugins)
    }
    return next
  })

  ipcMain.handle('hosts:list', () => sessionStore.listHosts())
  ipcMain.handle('hosts:save', (_e, host: HostProfile) => sessionStore.saveHost(host))
  ipcMain.handle('hosts:delete', (_e, id: string) => {
    sessionStore.deleteHost(id)
  })

  ipcMain.handle('tabs:get', () => tabStore.getTabs())
  ipcMain.handle('tabs:save', (_e, tabs: TabSnapshot[]) => {
    tabStore.saveTabs(tabs)
  })

  ipcMain.handle('vault:set', (_e, vaultId: string, value: string) => {
    vault.set(vaultId, value)
  })
  ipcMain.handle('vault:get', (_e, vaultId: string) => vault.get(vaultId))
  ipcMain.handle('vault:delete', (_e, vaultId: string) => {
    vault.delete(vaultId)
  })

  ipcMain.handle('session:connect', async (_e, req: ConnectRequest) => {
    await sessions.connect(req)
  })
  ipcMain.handle('session:disconnect', (_e, tabId: string) => {
    sessions.disconnect(tabId)
  })
  ipcMain.handle('session:write', (_e, tabId: string, data: string) => {
    sessions.write(tabId, data)
  })
  ipcMain.handle('session:resize', (_e, tabId: string, cols: number, rows: number) => {
    sessions.resize(tabId, cols, rows)
  })
  ipcMain.handle(
    'session:updateConnection',
    (_e, tabId: string, partial: Partial<import('../../shared/types').ConnectionParams>) => {
      sessions.updateConnection(tabId, partial)
    }
  )
  ipcMain.handle(
    'session:respondHostKey',
    (_e, tabId: string, decision: HostKeyDecision) => {
      sessions.respondHostKey(tabId, decision)
    }
  )
  ipcMain.handle(
    'session:respondSavePassword',
    (_e, tabId: string, decision: SavePasswordDecision, hostName?: string) => {
      sessions.respondSavePassword(tabId, decision, hostName)
    }
  )

  ipcMain.handle('app:beep', () => {
    shell.beep()
  })

  ipcMain.handle('dialog:pickPrivateKey', async () => {
    const win = getWindow()
    const result = win
      ? await dialog.showOpenDialog(win, {
          title: 'Select private key',
          properties: ['openFile']
        })
      : await dialog.showOpenDialog({
          title: 'Select private key',
          properties: ['openFile']
        })
    if (result.canceled || result.filePaths.length === 0) {
      return null
    }
    return result.filePaths[0]
  })

  ipcMain.handle('serial:listPorts', () => listSerialPorts())

  ipcMain.handle('plugins:list', () => pluginHost.listPlugins())
  ipcMain.handle('plugins:activate', async (_e, tabId: string, pluginId: string) => {
    await pluginHost.activate(tabId, pluginId)
  })
  ipcMain.handle('plugins:deactivate', async (_e, tabId: string, pluginId: string) => {
    await pluginHost.deactivate(tabId, pluginId)
  })
  ipcMain.handle('plugins:getActive', (_e, tabId: string) => pluginHost.getActivePlugins(tabId))
  ipcMain.handle(
    'plugins:message',
    async (_e, tabId: string, pluginId: string, payload: unknown) => {
      await pluginHost.handleRendererMessage(tabId, pluginId, payload)
    }
  )
  ipcMain.handle(
    'plugins:queueRestore',
    (_e, tabId: string, activePluginIds: string[]) => {
      queuePluginRestore(tabId, activePluginIds)
    }
  )
  ipcMain.handle('plugins:getData', (_e, pluginId: string) => pluginData.get(pluginId))
  ipcMain.handle('plugins:setData', (_e, pluginId: string, data: unknown) => {
    pluginData.set(pluginId, data)
  })
}
