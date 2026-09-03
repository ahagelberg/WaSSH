# WaSSH Plugin API

Technical reference for the plugin system in `src/main/plugins` (main) and
`src/renderer/src/plugins` (renderer). All plugin types live in
`src/shared/plugins.ts`; the Electron bridge type lives in `src/shared/types.ts`
(`WasshApi`).

External plugins (`userData/plugins/<id>/manifest.json` + `main.js` + `ui.js`)
are **reserved but not loaded** — `loadExternalPlugins()` returns `[]`. All six
current plugins are built-ins wired at compile time. The API below is the
surface those modules and their React views use.

## 1. Architecture

```
┌─ main process ───────────────────────────────────────────────┐
│ SessionManager ── PTY stream ──> SessionDataPipeline          │
│        │                          (observe/intercept hooks)   │
│        └─ PluginSessionHandle ──> SideConnectionBroker        │
│                                         │ ssh-exec/shell/tcp  │
│                                         │ raw TCP, SFTP       │
│ PluginHost (lifecycle, ctx, activate/deactivate)              │
│        ├─ PluginMainModule (onActivate/onDeactivate/onMessage)│
│        └─ PluginDataStore / CredentialVault                   │
└───────────────────────────────────────────────────────────────┘
   IPC: plugins:* (invoke)   plugin:* (main→renderer events)
┌─ renderer ────────────────────────────────────────────────────┐
│ window.wassh.*  ←→  PluginView (React, in plugins/registry.ts)│
│ Dock/split layout: PluginSessionFrame + shared/pluginLayout   │
└───────────────────────────────────────────────────────────────┘
```

A plugin is three things:
1. a **manifest** (`contributes`: toolbar, settings schemas, views);
2. a **main module** (`PluginMainModule`) with per-tab/per-session instances;
3. an optional **React view** for each contributed view.

## 2. Source map

| File | Role |
|---|---|
| `shared/plugins.ts` | Manifest/settings/event types, side-connection messages, per-builtin protocol types, constants, settings merge helpers |
| `main/plugins/PluginHost.ts` | `PluginMainModule`, `PluginMainContext`, lifecycle (activate/deactivate/list) |
| `main/plugins/types.ts` | `PluginSessionHandle` (SessionManager→broker), `StreamTransform` |
| `main/plugins/SessionDataPipeline.ts` | Ordered observe/intercept stream transforms |
| `main/plugins/SideConnectionBroker.ts` | SSH-exec/shell/TCP side connections, raw TCP duplexes, SFTP channels |
| `main/plugins/SftpSession.ts` | Promisified SFTP wrapper |
| `main/plugins/createPluginSystem.ts` | Wiring, restore queue |
| `main/plugins/externalLoader.ts` | Future external-plugin scanner (stub) |
| `main/plugins/builtins/*` | The six built-in plugins (manifest + main module) |
| `main/store/pluginDataStore.ts` | `userData/plugin-<id>.json` storage |
| `shared/pluginLayout.ts` | Per-tab dock/split layout model |
| `renderer/src/plugins/registry.ts` | Plugin id → React component |
| `preload/index.ts` | Exposes `window.wassh` |

Built-in ids (`shared/plugins.ts`): `server-monitor`, `scratchpad`,
`macro-pad`, `mqtt-explorer`, `sftp`, `ai-agent`. All six are enabled by
default (`DEFAULT_ENABLED_PLUGINS`) and declared `activation: 'manual'`.

## 3. Manifest (`PluginManifest`)

```ts
interface PluginManifest {
  id: string; name: string; version: string; description: string
  activation: 'manual' | 'auto'     // auto = started when a session connects
  source: 'builtin' | 'external'    // external reserved
  contributes: {
    toolbar?: { label: string }                     // per-session toggle button
    settingsHeading?: string                        // Options dialog section title
    settingsSchema?: PluginSettingsField[]          // app-wide settings
    hostSettingsHeading?: string                    // Host/session dialog section title
    hostSettingsSchema?: PluginSettingsField[]      // per-host settings
    views?: PluginViewContribution[]                // [{ id, placement, title? }]
  }
}
```

`PluginListItem = PluginManifest & { enabled: boolean }` is what
`listPlugins()` returns.

### Settings field (`PluginSettingsField`)

| Field | Description |
|---|---|
| `key` | Unique within its schema; stored object key |
| `label`, `description?` | UI text |
| `type` | `boolean` \| `number` \| `string` \| `stringList` \| `macroList` |
| `default` | Used when no stored value exists |
| `secret?` | `string` fields render as a password input (value is **not** vault-encrypted; only input masking) |

`macroList` values are `PluginMacroButton[]` = `{ id, label, text, hotkey }`
(hotkey e.g. `"Ctrl+Shift+1"`, empty = none). `stringList` UI is one-per-line
text; stored as `string[]`.

### View placement (`PluginViewPlacement`)

`split-left | split-right | split-top | split-bottom | overlay`. The first
declared view's placement seeds the default dock when the plugin is activated;
the user can re-dock/split any panel by dragging its grip.

## 4. Settings resolution and storage

Resolution order (both in the main `ctx.getSettings()` and the view's
`settings` prop — identical code path):

1. defaults from the schema (`default` values);
2. app-wide stored values `settings.json → pluginSettings[pluginId]`
   (`contributes.settingsSchema`);
3. host values override same-named keys: stored
   `HostProfile.pluginSettings[pluginId]` (if the live connection has a
   `hostId`), else the live `ConnectionParams.pluginSettings[pluginId]`
   (`contributes.hostSettingsSchema`).

Schemas must not overlap keys between app and host scope; host wins on
collision. Helpers: `defaultPluginSettingsFromSchema`, `mergePluginSettings`,
`mergePluginSessionSettings`, `normalizeHostPluginSettings`.

Persistence:
- `settings.json` → `enabledPlugins: string[]`,
  `pluginSettings: Record<pluginId, values>` (app-wide).
- `hosts.json` (per `HostProfile`) and each tab's `ConnectionParams` →
  `pluginSettings` (per-host values).
- `tabs.json` → per-tab `activePluginIds: string[]` + `pluginLayout`.

When a view calls `onSettingsPatch(partial)`, the renderer routes each key to
the app schema or the host schema (unknown keys fall back to host scope). App
keys are saved via `setSettings({ pluginSettings })`. Host keys are pushed into
the connection(s) via `session:updateConnection` (all open tabs sharing the
same `hostId`) and, when the connection belongs to a saved host, also persisted
to the `HostProfile` via `hosts:save`.

## 5. Main-process plugin module

```ts
interface PluginMainModule {
  onActivate:      (ctx: PluginMainContext) => void | Promise<void>
  onDeactivate?:   (ctx: PluginMainContext) => void | Promise<void>
  onMessage?:      (ctx: PluginMainContext, payload: unknown) => void | Promise<unknown>
}
```

- `onActivate` runs once per (tab, plugin) when the module starts. A throw
  aborts activation.
- `onMessage` handles renderer→main requests; its return value resolves the
  renderer's `sendPluginMessage(...)` promise.
- `onDeactivate` runs on shutdown; **always** pair long-lived work with
  `ctx.onDeactivateCleanup(fn)` for hard cleanup (timers, sockets, intervals),
  because the SFTP module can be demoted to "headless" without `onDeactivate`
  being called (see §7).

### `PluginMainContext` — every member

| Member | Signature / semantics |
|---|---|
| `tabId`, `pluginId` | Read-only identity of this instance |
| `getSettings()` | Merged app+host settings (§4), snapshot at call time |
| `getData(): unknown` | Reads this plugin's JSON file `userData/plugin-<id>.json`; `null` if missing/corrupt |
| `setData(data)` | Synchronously writes that file (2-space JSON); id is sanitized to `[A-Za-z0-9_-]` |
| `getSecret(vaultId): string \| null` | OS-encrypted secret (safeStorage/DPAPI); `null` when absent |
| `sendToRenderer(payload)` | Push an event; renderer receives it as `{tabId, pluginId, payload}` on `onPluginMessage` |
| `openSideConnection(req): Promise<connectionId>` | Open a side channel (§6) |
| `closeSideConnection(connectionId)` | Close a side channel |
| `writeSideConnection(connectionId, data)` | Write UTF-8 to a side channel |
| `onSideData(connectionId, cb(data)): unsubscribe` | Data from a side channel (main-side listener) |
| `onSideClosed(connectionId, cb(error?)): unsubscribe` | Channel closed; both listener maps are cleared |
| `isSshSession(): boolean` | True only when the live session is SSH (not telnet/serial) |
| `openTcpStream(host, port): Promise<Duplex>` | Raw binary duplex for non-UTF-8 protocols; SSH `forwardOut` when SSH, else direct TCP. No side-data events |
| `openSftp(): Promise<SftpSession>` | SFTP channel on the live SSH connection (§8); throws for non-SSH |
| `execCapture(command): Promise<string>` | Run on the live SSH session and return trimmed stdout (e.g. `pwd`); throws for non-SSH |
| `registerStreamHandler(mode, direction, handler)` | PTY stream transform (§6); auto-removed on deactivate |
| `onDeactivateCleanup(fn)` | Register cleanup; always runs on deactivate/disable/tab close |
| `writeToSession(data)` | Write raw bytes into the session PTY, **bypassing the outbound pipeline** (no recursion into own interceptors) |

## 6. Side connections, streams, transforms

### `SessionDataPipeline`

```ts
type StreamTransform = (data: string) => string | null
registerStreamHandler(mode: 'observe'|'intercept',
                      direction: 'inbound'|'outbound', handler)
```

- **inbound**: data flowing from the remote into the terminal (remote→UI).
  Runs before `session:data` reaches the renderer.
- **outbound**: data being written into the session (UI keystrokes /
  `session:write`).
- Handlers run per-chunk **in registration order** per direction.
- `intercept`: return `string` (rewrite) or `null` (drop the chunk); receives
  the previous interceptor's output.
- `observe`: receives the current chunk, return value ignored — cannot mutate.
- Handler exceptions never break the stream: observer errors are swallowed; an
  interceptor error keeps the previous data.
- Registered handlers are torn down automatically at deactivation.

### `openSideConnection` kinds (`SideConnectionKind`)

| kind | Fields | Behavior |
|---|---|---|
| `ssh-exec` | `command` | Requires SSH. Runs `command`; stdout+stderr merged into `data` events |
| `ssh-shell` | `duplicate?` | Requires SSH. `duplicate:true` opens an isolated duplicate SSH client + shell (credentials never leave main); otherwise opens an extra shell on the live client |
| `tcp` | `host?`, `port?` | Through SSH `forwardOut` when SSH, else direct TCP. Defaults to the session's own host/port |
| `serial` | — | **Not implemented** — throws |

`data`/`close` events go **both** to the main-side `onSideData`/`onSideClosed`
listeners and to the renderer as `plugin:sideData` / `plugin:sideClosed`.
Chunks are UTF-8-decoded buffers — for binary protocols use `openTcpStream`.
Side connections for a plugin/tab are force-closed on deactivate, on session
disconnect, and on shutdown. `write`/`close` are no-ops for unknown ids.
Channel `close`/`error` emits a final `onSideClosed` (`error` = message).
Note: `ctx.writeToSession` does **not** re-enter the outbound pipeline.

### Underlying session handle (for reference)

`PluginSessionHandle` (SessionManager→broker): `tabId`, `connection`
(`ConnectionParams`), `isSsh`, `getSshClient()`, `exec(command)`,
`execCapture(command)`, `openSftp()`, `openExtraShell()`,
`forwardOut(host,port)`, `openDuplicateClient()` (`{client, dispose}`),
`openDirectTcp(host,port)`.

## 7. Lifecycle and activation

- **Global enable** = `AppSettings.enabledPlugins`. Change it with
  `setSettings({ enabledPlugins })`; the host deactivates removed plugins on
  every tab (`onEnabledPluginsChanged`).
- **Per-tab active** = an instantiated module. Toolbar buttons call
  `activatePlugin`/`deactivatePlugin`; activation succeeds only if the plugin
  is enabled.
- **On session connect** the host activates every enabled plugin with
  `activation: 'auto'`, plus any ids from `queuePluginRestore` (reconnect and
  startup-restore path) that are enabled.
- **SFTP special case**: after connect, if SFTP is enabled and the session is
  SSH, SFTP is activated **headless** (`announced=false`) even while its Files
  browser is closed — it must stay alive to serve terminal drag-and-drop
  uploads. Closing the SFTP panel while SSH simply un-announces it; the module
  keeps running (`deactivate(tabId, PLUGIN_ID_SFTP, force=false)` reflects this).
- **Announce**: `plugin:active` (`{tabId, pluginId, active}`) is sent when a
  view is shown/hidden; the renderer tracks `activePluginIds` and prunes/keeps
  the dock layout from it.
- **Deactivation order**: `module.onDeactivate` → all
  `onDeactivateCleanup` fns → pipeline handlers unregistered → side
  connections/streams/SFTP closed. `deactivateAll` runs on session removal and
  shutdown.
- Activation happens once a session reaches `connected` (auto/restore) or on
  demand; modules needing a connection gate on `isSshSession()` and re-check on
  each activation after reconnect.

## 8. SFTP (`SftpSession`)

`ctx.openSftp()` throws unless the live session is SSH. The wrapper is
promisified over one ssh2 `SFTPWrapper` (`entry.sftp`):

`list(path)` → `SftpEntry[]` · `mkdir(path)` · `rename(old, new)` ·
`chmod(path, mode)` · `unlink(path)` · `rmdir(path)` · `delete(path)`
(recursive, lstat-based, never follows symlinks) · `stat(path)` /
`lstat(path)` / `statSafe(path)` (null instead of reject) /
`realpath(path)` (OpenSSH tilde expansion when available) ·
`createReadStream(path)` · `createWriteStream(path, opts?)` ·
`close(handle)` · `end()`.

Errors reject as `{ message, kind }` with `kind: SftpErrorKind` =
`not_ssh | not_found | permission | not_dir | exists | name_in_use | io |
connection | cancelled | other` (`classifySftpError`). `SftpEntry` rows:
`{ name, path, type ('file'|'directory'|'symlink'|'other'), size, mode,
modeSymbolic, mtime (epoch ms), uid?, gid? }`. Helper:
`joinRemotePath(parent, name)`.

## 9. Renderer API (`window.wassh`)

### Plugin calls (renderer→main, `invoke`)

| Call | IPC channel | Returns |
|---|---|---|
| `listPlugins()` | `plugins:list` | `PluginListItem[]` |
| `activatePlugin(tabId, pluginId)` | `plugins:activate` | — (throws if disabled / no module) |
| `deactivatePlugin(tabId, pluginId)` | `plugins:deactivate` | — |
| `getActivePlugins(tabId)` | `plugins:getActive` | `string[]` (instantiated ids, incl. headless SFTP) |
| `sendPluginMessage(tabId, pluginId, payload)` | `plugins:message` | resolves to the main module's `onMessage` return |
| `queuePluginRestore(tabId, ids)` | `plugins:queueRestore` | applied on next `connected` status |
| `getPluginData(pluginId)` | `plugins:getData` | stored JSON value |
| `setPluginData(pluginId, data)` | `plugins:setData` | — |

### Events (main→renderer)

| Subscribe fn | Channel | Payload |
|---|---|---|
| `onPluginActive(cb)` | `plugin:active` | `{ tabId, pluginId, active }` |
| `onPluginMessage(cb)` | `plugin:message` | `{ tabId, pluginId, payload }` |
| `onSideConnectionData(cb)` | `plugin:sideData` | `{ connectionId, data }` |
| `onSideConnectionClosed(cb)` | `plugin:sideClosed` | `{ connectionId, error? }` |

Every `on*` returns an unsubscribe function. Views filter events by their own
`tabId` + `pluginId`. The full app API (session write/data/status, settings,
hosts, vault, dialogs, serial) is also reachable from views — see `WasshApi` in
`shared/types.ts`.

## 10. Renderer view contract

```ts
interface PluginViewProps {
  tabId: string
  pluginId: string
  settings: Record<string, unknown>   // merged app+host (§4)
  onSettingsPatch: (partial: Record<string, unknown>) => void
}
```

- Views are looked up in `renderer/src/plugins/registry.ts`
  (`getPluginView(pluginId)`); a plugin without a view only contributes
  settings/toolbar.
- Views mount inside `PluginSessionFrame` only while docked. A main module
  pushing events while no view is mounted loses them — have the view request a
  snapshot (`sendPluginMessage`) or read `getPluginData` on mount.
- Panel chrome (title/close/drag) is `PluginPanelShell`; the panel title is
  `views[0].title` → `toolbar.label` → `name`.
- Per-tab layout is a `TabPluginLayout` (left/right/top/bottom/overlay roots of
  `LayoutNode` `leaf`/`split` with ratio) persisted in `tabs.json`; the
  manifest placement is used only at first activation. Toolbar entries come
  from `enabledToolbarPlugins(plugins)` (enabled + `contributes.toolbar`).
- Macro-pad additionally receives `activeTab` so hotkeys fire only on the
  visible tab; it writes by messaging its main module `{type:'send', text}`,
  which calls `ctx.writeToSession`.

## 11. Built-in plugin wire protocols

Each built-in's payload shapes are concrete discriminated unions in
`shared/plugins.ts` (constants and types per plugin). Summary of their
main⇄renderer messages:

| Plugin | Renderer→main (`sendPluginMessage`) | Main→renderer (`sendToRenderer`) |
|---|---|---|
| server-monitor | `setProcessSort` / `signalProcess` (TERM\|KILL) / `refresh` | `stats` → `ServerMonitorSnapshot` |
| macro-pad | `send` → `writeToSession` | — |
| scratchpad | — (renderer-driven; main is a no-op hook) | — |
| mqtt-explorer | `publish` / `reconnect` | `status` → `MqttExplorerStatusPayload`; `message` → `MqttExplorerMessagePayload` |
| sftp | `getStatus`, `list`, `mkdir`, `rename`, `chmod`, `delete`, `download`, `uploadDialog`, `uploadStart`/`uploadChunk`/`uploadEnd`, `cancel`, `resetCwd` | `status`, `listResult`, `opResult`, `transferProgress`, `transferDone` |
| ai-agent | `sync`, `probe`, `chat`, `stop`, `resume`, `discardPaused`, `approval`, `sudoPassword`, `rulesChanged`, `select`, `providersChanged`, `newChat`, `openChat`, `deleteChat` | `state` → `AiAgentStateSnapshot` (incl. `pendingApproval`/`pendingSudo`), `delta`, `toast` |

`AiAgentRendererMessage`, `SftpRendererMessage`, `MqttExplorerRendererMessage`
(and their payload types) are exported from `shared/plugins.ts`.

## 12. Adding a built-in plugin (checklist)

1. `shared/plugins.ts`: add `PLUGIN_ID_*`, message types, constants.
2. `main/plugins/builtins/<name>.ts`: export a `PluginMainModule`.
3. `main/plugins/builtins/manifests.ts`: manifest with contributes + defaults;
   add to `BUILTIN_MANIFESTS`.
4. `main/plugins/PluginHost.ts`: import and `registerBuiltins()` the module.
5. `renderer/src/plugins/builtins/<Name>View.tsx` + `registry.ts` entry for a
   view.
6. Optional: settings fields are auto-rendered by `PluginFieldEditor`; no
   dialog work needed.
7. Add to `DEFAULT_ENABLED_PLUGINS` in `shared/plugins.ts` to ship enabled.

External plugins will follow the same manifest/main/ui split once the loader
in `externalLoader.ts` is implemented.



