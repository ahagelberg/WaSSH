import { contextBridge, ipcRenderer } from 'electron'
import type {
  AppSettings,
  ConnectRequest,
  HostKeyDecision,
  HostKeyPrompt,
  HostProfile,
  SavePasswordDecision,
  SavePasswordPrompt,
  SessionStatusEvent,
  TabSnapshot,
  WasshApi
} from '../shared/types'

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
  respondHostKey: (tabId: string, decision: HostKeyDecision) =>
    ipcRenderer.invoke('session:respondHostKey', tabId, decision),
  respondSavePassword: (tabId: string, decision: SavePasswordDecision, hostName?: string) =>
    ipcRenderer.invoke('session:respondSavePassword', tabId, decision, hostName),
  pickPrivateKeyFile: () => ipcRenderer.invoke('dialog:pickPrivateKey'),
  beep: () => ipcRenderer.invoke('app:beep'),
  onSessionData: (cb) =>
    on('session:data', (tabId, data) => cb(tabId as string, data as string)),
  onSessionStatus: (cb) =>
    on('session:status', (ev) => cb(ev as SessionStatusEvent)),
  onCycleTab: (cb) => on('tabs:cycle', (delta) => cb(delta as number)),
  onOpenPreferences: (cb) => on('app:openPreferences', () => cb()),
  onHostKeyPrompt: (cb) =>
    on('session:hostKeyPrompt', (prompt) => cb(prompt as HostKeyPrompt)),
  onSavePasswordPrompt: (cb) =>
    on('session:savePasswordPrompt', (prompt) => cb(prompt as SavePasswordPrompt))
}

contextBridge.exposeInMainWorld('wassh', api)
