import { contextBridge, ipcRenderer } from 'electron'
import type {
  AppSettings,
  ConnectRequest,
  ConnectionParams,
  HostKeyDecision,
  HostKeyPrompt,
  HostProfile,
  SavePasswordDecision,
  SavePasswordPrompt,
  SessionStatusEvent,
  TabSnapshot,
  WasshApi
} from '../shared/types'
import type {
  PluginActiveStateEvent,
  PluginMessageEvent,
  SideConnectionClosedEvent,
  SideConnectionDataEvent
} from '../shared/plugins'

function on(
  channel: string,
  cb: (...args: unknown[]) => void
): () => void {
  const listener = (_event: Electron.IpcRendererEvent, ...args: unknown[]): void => {
    cb(...args)
  }
  ipcRenderer.on(channel, listener)
  return () => {
    ipcRenderer.removeListener(channel, listener)
  }
}

const api: WasshApi = {
  getSettings: () => ipcRenderer.invoke('settings:get'),
  setSettings: (partial: Partial<AppSettings>) => ipcRenderer.invoke('settings:set', partial),
  listHosts: () => ipcRenderer.invoke('hosts:list'),
  saveHost: (host: HostProfile) => ipcRenderer.invoke('hosts:save', host),
  deleteHost: (id: string) => ipcRenderer.invoke('hosts:delete', id),
  getTabSnapshot: () => ipcRenderer.invoke('tabs:get'),
  saveTabSnapshot: (tabs: TabSnapshot[]) => ipcRenderer.invoke('tabs:save', tabs),
  setSecret: (vaultId: string, value: string) => ipcRenderer.invoke('vault:set', vaultId, value),
  getSecret: (vaultId: string) => ipcRenderer.invoke('vault:get', vaultId),
  deleteSecret: (vaultId: string) => ipcRenderer.invoke('vault:delete', vaultId),
  connect: (req: ConnectRequest) => ipcRenderer.invoke('session:connect', req),
  disconnect: (tabId: string) => ipcRenderer.invoke('session:disconnect', tabId),
  write: (tabId: string, data: string) => ipcRenderer.invoke('session:write', tabId, data),
  resize: (tabId: string, cols: number, rows: number) =>
    ipcRenderer.invoke('session:resize', tabId, cols, rows),
  updateConnection: (tabId: string, partial: Partial<ConnectionParams>) =>
    ipcRenderer.invoke('session:updateConnection', tabId, partial),
  respondHostKey: (tabId: string, decision: HostKeyDecision) =>
    ipcRenderer.invoke('session:respondHostKey', tabId, decision),
  respondSavePassword: (tabId: string, decision: SavePasswordDecision, hostName?: string) =>
    ipcRenderer.invoke('session:respondSavePassword', tabId, decision, hostName),
  pickPrivateKeyFile: () => ipcRenderer.invoke('dialog:pickPrivateKey'),
  listSerialPorts: () => ipcRenderer.invoke('serial:listPorts'),
  beep: () => ipcRenderer.invoke('app:beep'),
  listPlugins: () => ipcRenderer.invoke('plugins:list'),
  activatePlugin: (tabId, pluginId) => ipcRenderer.invoke('plugins:activate', tabId, pluginId),
  deactivatePlugin: (tabId, pluginId) =>
    ipcRenderer.invoke('plugins:deactivate', tabId, pluginId),
  getActivePlugins: (tabId) => ipcRenderer.invoke('plugins:getActive', tabId),
  sendPluginMessage: (tabId, pluginId, payload) =>
    ipcRenderer.invoke('plugins:message', tabId, pluginId, payload),
  queuePluginRestore: (tabId, activePluginIds) =>
    ipcRenderer.invoke('plugins:queueRestore', tabId, activePluginIds),
  getPluginData: (pluginId) => ipcRenderer.invoke('plugins:getData', pluginId),
  setPluginData: (pluginId, data) => ipcRenderer.invoke('plugins:setData', pluginId, data),
  onSessionData: (cb) =>
    on('session:data', (tabId, data) => cb(tabId as string, data as string)),
  onSessionStatus: (cb) =>
    on('session:status', (ev) => cb(ev as SessionStatusEvent)),
  onCycleTab: (cb) => on('tabs:cycle', (delta) => cb(delta as number)),
  onCloseActiveTab: (cb) => on('tabs:closeActive', () => cb()),
  onReopenClosedTab: (cb) => on('tabs:reopenClosed', () => cb()),
  onOpenPreferences: (cb) => on('app:openPreferences', () => cb()),
  onOpenAbout: (cb) => on('app:openAbout', () => cb()),
  onOpenFind: (cb) => on('app:openFind', () => cb()),
  onReconnectActive: (cb) => on('session:reconnectActive', () => cb()),
  onReconnectAll: (cb) => on('session:reconnectAll', () => cb()),
  onOpenSessionSettings: (cb) => on('session:openSettings', () => cb()),
  onHostKeyPrompt: (cb) =>
    on('session:hostKeyPrompt', (prompt) => cb(prompt as HostKeyPrompt)),
  onSavePasswordPrompt: (cb) =>
    on('session:savePasswordPrompt', (prompt) => cb(prompt as SavePasswordPrompt)),
  onPluginActive: (cb) =>
    on('plugin:active', (ev) => cb(ev as PluginActiveStateEvent)),
  onPluginMessage: (cb) =>
    on('plugin:message', (ev) => cb(ev as PluginMessageEvent)),
  onSideConnectionData: (cb) =>
    on('plugin:sideData', (ev) => cb(ev as SideConnectionDataEvent)),
  onSideConnectionClosed: (cb) =>
    on('plugin:sideClosed', (ev) => cb(ev as SideConnectionClosedEvent))
}

contextBridge.exposeInMainWorld('wassh', api)
