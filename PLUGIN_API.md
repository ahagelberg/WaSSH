# WaSSH Plugin API

Technical reference for the plugin system. Stable contracts live in
`src/shared/pluginApi.ts`, `src/main/plugins/api.ts`, and
`src/renderer/src/plugins/api`; the Electron bridge type lives in
`src/shared/types.ts` (`WasshApi`).

External plugins (`userData/plugins/<id>/manifest.json` + `main.js` + `ui.js`)
are **reserved but not loaded** — `loadExternalPlugins()` returns `[]`. All
current plugins are self-contained built-ins wired at compile time. The API
below is the only application surface their modules and React views may use.

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
| `shared/pluginApi.ts` | Neutral contracts used across process boundaries or by more than one plugin: manifests, settings, events, placement, side connections, remote-file metadata, and settings merging |
| `main/plugins/api.ts` | `PluginMainModule`, `PluginMainContext`, lifecycle/session hooks, main registration metadata |
| `renderer/src/plugins/api` | View contract, renderer registration, file-drop contract, neutral React UI components and styles |
| `main/plugins/PluginHost.ts` | Private lifecycle and capability implementation |
| `main/plugins/types.ts` | `PluginSessionHandle` (SessionManager→broker), `StreamTransform` |
| `main/plugins/SessionDataPipeline.ts` | Ordered observe/intercept stream transforms |
| `main/plugins/SideConnectionBroker.ts` | SSH-exec/shell/TCP side connections, raw TCP duplexes, SFTP channels |
| `main/plugins/SftpSession.ts` | Promisified SFTP wrapper |
| `main/plugins/createPluginSystem.ts` | Wiring, restore queue |
| `main/plugins/externalLoader.ts` | Future external-plugin scanner (stub) |
| `plugins/builtins/<id>/*` | Self-contained built-ins: manifest, protocol, main module, view, helpers, and private CSS |
| `main/store/pluginDataStore.ts` | `userData/plugin-<id>.json` storage |
| `shared/pluginLayout.ts` | Per-tab dock/split layout model |
| `main/plugins/builtinRegistry.ts` | Compile-time main-process composition root; also the one place that aggregates every other plugin's `contributes.api` into AI Agent's settings (§7) |
| `renderer/src/plugins/builtinRegistry.ts` | Compile-time renderer composition root |
| `preload/index.ts` | Exposes `window.wassh` |

Built-in ids are owned by each plugin's `id.ts`. The main composition root
derives the default enabled-id list from the registered manifests instead of
duplicating plugin ids in shared code.

`shared/` is not a second home for plugin implementations. It contains
process-neutral application models and stable plugin API contracts consumed by
the main process, preload, renderer, or multiple plugins. Plugin ids, defaults,
wire messages, feature helpers, and feature-specific types belong under
`plugins/builtins/<id>/`.

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
    api?: { methods: PluginApiMethod[] }            // methods callable via ctx.callPluginApi (§7)
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
| `type` | `boolean` \| `number` \| `string` \| `select` \| `stringList` \| `commandList` \| `group` \| `permission` |
| `default` | Used when no stored value exists |
| `secret?` | `string` fields render as a password input (value is **not** vault-encrypted; only input masking) |
| `children?` | Only for `type: 'group'` - nested fields shown under the master boolean toggle |

`commandList` values are `PluginCommand[]` = `{ id, label, text, hotkey }`
(hotkey e.g. `"Ctrl+Shift+1"`, empty = none). `stringList` UI is one-per-line
text; stored as `string[]`.

`group` is a boolean master toggle (its own `default` is `boolean`) whose
`children` are rendered indented underneath, visible but disabled while the
master is off. `permission` is a trinary control with a fixed value space
`PluginPermissionDecision = 'allow' | 'deny' | 'ask'` (its `default` is one of
those three strings) - used for AI Agent's own and other plugins' API-method
permissions (§7). Both are generic, reusable field types, not private to any
one plugin. `renderer/src/plugins/PluginSettingsFieldList.tsx` (exported as
`PluginSettingsFieldList` from `@plugin-api/renderer`) renders a whole schema
recursively, including `group` nesting; `PluginFieldEditor` renders one field's
control (used internally by `PluginSettingsFieldList`, and directly by plugins
with a custom settings view).

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
`mergePluginSessionSettings`, `normalizeHostPluginSettings`, `flattenFields`.
`flattenFields` recurses into `group` fields' `children` so nested keys are
read/written/merged exactly like top-level ones in the flat stored settings
map - schemas with `group` fields need no special-casing elsewhere.

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

A main-process module can also persist one of its own app-wide settings keys
directly with `ctx.setSettingValue(key, value)` (e.g. AI Agent's "Approve
always" flipping a permission from `ask` to `allow`). This calls
`SettingsStore.set` the same way `settings:set` does, and both paths broadcast
`settings:changed` to the renderer (§10) so any open dialog stays in sync.

## 5. Main-process plugin module

```ts
interface PluginMainModule {
  onActivate:      (ctx: PluginMainContext) => void | Promise<void>
  onDeactivate?:   (ctx: PluginMainContext) => void | Promise<void>
  onMessage?:      (ctx: PluginMainContext, payload: unknown) => void | Promise<unknown>
  onSessionStatus?: (ctx: PluginMainContext, event: PluginSessionStatusEvent) => void | Promise<void>
  onApiCall?:      (ctx: PluginMainContext, method: string, params: unknown) => unknown | Promise<unknown>
}
```

- `onActivate` runs once per (tab, plugin) when the module starts. A throw
  aborts activation.
- `onMessage` handles renderer→main requests; its return value resolves the
  renderer's `sendPluginMessage(...)` promise.
- `onDeactivate` runs on shutdown; **always** pair long-lived work with
  `ctx.onDeactivateCleanup(fn)` for hard cleanup (timers, sockets, intervals),
  because the SFTP module can be demoted to "headless" without `onDeactivate`
  being called (see §8).
- `onApiCall` handles `ctx.callPluginApi(...)` invocations from another plugin
  (or from this plugin's own module) targeting one of the methods declared in
  `contributes.api.methods` (§7). Required only if the manifest declares `api`.

### `PluginMainContext` — every member

| Member | Signature / semantics |
|---|---|
| `tabId`, `pluginId` | Read-only identity of this instance |
| `getSettings()` | Merged app+host settings (§4), snapshot at call time |
| `getData(scopeId?)` / `setData(data, scopeId?)` | Read/write plugin-owned JSON, optionally scoped to `plugin-<id>.<scope>.json` |
| `getSessionScopeId()` | Saved host id, or a stable `tab:<id>` scope for an unsaved session |
| `getSecret(vaultId): string \| null` | OS-encrypted secret (safeStorage/DPAPI); `null` when absent |
| `sendToRenderer(payload)` | Push an event; renderer receives it as `{tabId, pluginId, payload}` on `onPluginMessage` |
| `openSideConnection(req): Promise<connectionId>` | Open a side channel (§6) |
| `closeSideConnection(connectionId)` | Close a side channel |
| `writeSideConnection(connectionId, data)` | Write UTF-8 to a side channel |
| `onSideData(connectionId, cb(data)): unsubscribe` | Data from a side channel (main-side listener) |
| `onSideClosed(connectionId, cb(error?)): unsubscribe` | Channel closed; both listener maps are cleared |
| `isSshSession(): boolean` | True only when the live session is SSH (not telnet/serial) |
| `openTcpStream(host, port): Promise<Duplex>` | Raw binary duplex for non-UTF-8 protocols; SSH `forwardOut` when SSH, else direct TCP. No side-data events |
| `openSftp(): Promise<SftpSession>` | SFTP channel on the live SSH connection (§9); throws for non-SSH |
| `execCapture(command): Promise<string>` | Run on the live SSH session and return trimmed stdout (e.g. `pwd`); throws for non-SSH |
| `registerStreamHandler(mode, direction, handler)` | PTY stream transform (§6); auto-removed on deactivate |
| `onDeactivateCleanup(fn)` | Register cleanup; always runs on deactivate/disable/tab close |
| `writeToSession(data)` | Write raw bytes into the session PTY, **bypassing the outbound pipeline** (no recursion into own interceptors) |
| `listPluginApis(): PluginApiListing[]` | Declared API methods of every *other* plugin currently active on this tab (§7) |
| `callPluginApi(pluginId, method, params): Promise<unknown>` | Call another plugin's (or, for uniformity, this plugin's own) declared API method on this tab (§7) |
| `setSettingValue(key, value)` | Persist one key of this plugin's own app-wide stored settings (§4) |

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

## 7. Plugin-to-plugin API calls

A plugin can expose callable methods for *other* plugins to use (this is also
how the AI Agent turns capabilities into LLM tools - it is not AI-Agent
specific). Declare them statically in the manifest, and handle them at
runtime:

```ts
contributes: {
  api: {
    methods: [
      {
        name: 'get_snapshot',
        description: 'Return the latest sampled stats snapshot.',
        parameters: { type: 'object', properties: {} }   // optional, JSON-schema-ish
      }
    ]
  }
}
```
```ts
onApiCall(ctx, method, params) {
  if (method === 'get_snapshot') { return currentSnapshotFor(ctx) }
  throw new Error(`Unknown method: ${method}`)
}
```

- `ctx.listPluginApis()` returns the declared methods of every *other* plugin
  **currently active on the same tab** (the caller's own plugin id is
  excluded, and inactive plugins are not listed - the list changes live as
  panels open/close).
- `ctx.callPluginApi(pluginId, method, params)` calls that plugin's
  `onApiCall`. It throws if `pluginId` has no active instance on this tab, or
  doesn't declare `method` - there is **no auto-activation** of the target.
- `callPluginApi` performs **no permission gating of its own** - gating (if
  any) is entirely the caller's responsibility. The AI Agent gates in its own
  run loop before calling; a plugin that calls another plugin's API directly
  bypasses whatever gating the target would otherwise apply to that action
  through its own UI/renderer path. Keep this in mind before declaring
  mutating/destructive methods.
- Manifests only ever reference their **own** methods; nothing here requires
  importing another plugin's module (the checklist rule in §14 still holds).

### AI Agent as a consumer (and how its own tools reuse the same shape)

The AI Agent plugin turns every declared method - both other plugins' and its
own built-ins (`run_command`, `web_fetch`, `web_search`, `remote_fs_*`,
`local_fs_*`, `get_current_time`) - into an LLM tool, and dispatches **all**
of them through `ctx.callPluginApi(pluginId, method, args)`, including calls
to itself (`pluginId === PLUGIN_ID_AI_AGENT`, always active while its own run
loop executes). This keeps one code path and one declaration shape
(`PluginApiMethod`) for every tool the model can call, instead of a separate
ad hoc mechanism for "built-in" vs "other plugin" capabilities.

- Every method except `run_command` is gated by a `permission` settings field
  (`allow | deny | ask`, §3); `run_command` keeps its own, more granular
  allow/deny command-pattern rule lists (unchanged) since a single per-tool
  toggle would be coarser than what it already has.
- `ask` triggers an approval prompt in the AI Agent view; choosing "Approve
  always" calls `ctx.setSettingValue` to flip that permission to `allow`
  going forward (§4).
- Tool results are stringified (`JSON.stringify` for non-string returns) and
  capped at a generous size before being fed back to the model.
- Other plugins' methods are exposed under a wire tool name
  `plugin_api__<pluginId>__<method>` (parsed back to route the call); AI
  Agent's own built-in tools keep their original unprefixed names for
  backward compatibility with existing conversations.
- `main/plugins/builtinRegistry.ts` is the one place allowed to aggregate
  every *other* plugin's `contributes.api` into a generated permission
  section appended to AI Agent's `settingsSchema` (one `group` per plugin,
  default off, one `permission` child per method) - see §2's note on that
  file. Individual plugins never reference each other directly.

## 8. Lifecycle and activation

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

## 9. SFTP (`SftpSession`)

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

## 10. Renderer API (`window.wassh`)

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
| `onSettingsChanged(cb)` | `settings:changed` | `AppSettings` - fired whenever settings are persisted from any source (renderer `setSettings` or a main-process plugin's `ctx.setSettingValue`) |

Every `on*` returns an unsubscribe function. Views filter events by their own
`tabId` + `pluginId`. The full app API (session write/data/status, settings,
hosts, vault, dialogs, serial) is also reachable from views — see `WasshApi` in
`shared/types.ts`.

## 11. Renderer view contract

```ts
interface PluginViewProps {
  tabId: string
  pluginId: string
  hostId: string | null
  active: boolean                       // this tab is currently visible
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
- Plugins that register global shortcuts or timers must use `active` to suppress
  work while their tab is not visible.

## 12. Renderer UI kit

Import renderer components from `@plugin-api/renderer` and load no application
component directly. `plugin-ui.css` is loaded by the host and provides the
stable, neutral styling contract below.

| API | Purpose |
|---|---|
| `PluginButton` | Standard button; `variant="primary" \| "danger"` and `compact` |
| `PluginField` | Label, control, and optional hint layout |
| `PluginColorInput` | Validated six-digit hex color input |
| `PluginSettingsFieldList` | Recursively renders a `PluginSettingsField[]` schema (including `group`/`permission` nesting, §3); used by the Options and Host dialogs, and available to a plugin's own custom settings view |
| `.plugin-ui-panel` | Full-height plugin view root |
| `.plugin-ui-toolbar`, `.plugin-ui-actions`, `.plugin-ui-spacer` | Flexible action rows |
| `.plugin-ui-button` with `--primary`, `--danger`, `--compact` | Button classes for non-component use |
| `.plugin-ui-field`, `.plugin-ui-label`, `.plugin-ui-input`, `.plugin-ui-select`, `.plugin-ui-textarea` | Form controls |
| `.plugin-ui-card`, `.plugin-ui-heading`, `.plugin-ui-muted`, `.plugin-ui-hint` | Content grouping and typography |
| `.plugin-ui-error`, `.plugin-ui-success`, `.plugin-ui-warning`, `.plugin-ui-empty` | Semantic states |
| `.plugin-ui-badge`, `.plugin-ui-table-wrap`, `.plugin-ui-table`, `.plugin-ui-progress` | Common data display |
| `.plugin-ui-modal-backdrop`, `.plugin-ui-modal` | Accessible modal structure |
| `.plugin-ui-visually-hidden` | Screen-reader-only text |

Public custom properties use the `--plugin-ui-*` prefix. Public selectors must
use `plugin-ui-*` and remain neutral; selectors, variables, animations, and
responsive rules unique to a plugin belong in that plugin's `styles.css`.
Host-owned dock, splitter, toolbar, and panel chrome styles are not part of the
plugin styling API.

```tsx
import { PluginButton, PluginField } from '@plugin-api/renderer'

<div className="plugin-ui-panel">
  <PluginField label="Topic" hint="MQTT wildcard syntax is supported.">
    <input className="plugin-ui-input" value={topic} onChange={onTopicChange} />
  </PluginField>
  <PluginButton variant="primary" onClick={submit}>Apply</PluginButton>
</div>
```

Controls must retain a visible keyboard focus state, icon-only buttons need an
accessible name, status updates should use an appropriate live region, and
color must not be the only way state is communicated.

## 13. Built-in plugin wire protocols

Each built-in owns its concrete discriminated unions in
`plugins/builtins/<id>/protocol.ts`. Those protocols are private to that plugin,
not part of the shared plugin API. Summary of their main-renderer messages:

| Plugin | Renderer→main (`sendPluginMessage`) | Main→renderer (`sendToRenderer`) |
|---|---|---|
| server-monitor | `setProcessSort` / `signalProcess` (TERM\|KILL) / `refresh` | `stats` → `ServerMonitorSnapshot` |
| macro-pad | `send` → `writeToSession` | — |
| scratchpad | — (renderer-driven; main is a no-op hook) | — |
| mqtt-analyser | `publish` / `reconnect` | `status` → `MqttAnalyserStatusPayload`; `message` → `MqttAnalyserMessagePayload` |
| sftp | `getStatus`, `list`, `mkdir`, `rename`, `chmod`, `delete`, `download`, `viewFile`, `uploadDialog`, `uploadStart`/`uploadChunk`/`uploadEnd`, `cancel`, `resetCwd` | `status`, `listResult`, `opResult`, `transferProgress`, `transferDone`, `viewFileResult` → `SftpViewFilePayload` |
| ai-agent | `sync`, `probe`, `chat`, `stop`, `resume`, `discardPaused`, `approval` (`{requestId, kind: 'command'\|'permission', decision}`), `sudoPassword`, `rulesChanged`, `select`, `providersChanged`, `refreshModels`, `newChat`, `openChat`, `deleteChat` | `state` → `AiAgentStateSnapshot` (incl. `pendingApproval`/`pendingSudo`), `delta`, `toast` |

`contributes.api` methods (§7) declared by server-monitor (`get_snapshot`),
mqtt-analyser (`get_topics`, `get_topic_value`, `publish`), macro-pad
(`execute_macro`), scratchpad (`read_notes`, `write_notes`, `append_notes`),
and AI Agent's own built-in tools are called via `ctx.callPluginApi`, not
`sendPluginMessage` - they are a separate, cross-plugin call surface.

## 14. Adding a built-in plugin (checklist)

1. Create `plugins/builtins/<id>/` with `manifest.ts`, `protocol.ts`, `main.ts`,
   optional `View.tsx`, helpers, and `styles.css`.
2. Import only local files, third-party packages, `@plugin-api/shared`,
   `@plugin-api/main`, or `@plugin-api/renderer`. Never import another plugin,
   `PluginHost`, application components, or private stores/brokers.
3. Add the manifest and default-enabled metadata to the shared built-in
   composition registry.
4. Add the main module and any generic background activation metadata to
   `main/plugins/builtinRegistry.ts`.
5. Add the view and optional terminal file-drop handler to
   `renderer/src/plugins/builtinRegistry.ts`; import the plugin's private CSS
   from its renderer entry.
6. Use manifest settings schemas for automatically rendered settings.
   Set `settingsPresentation: 'view'` when the plugin owns a custom editor.
7. To expose methods to other plugins, declare `contributes.api.methods` and
   implement `onApiCall` (§7). Keep the method's own `parameters`/description
   self-sufficient - never import a consuming plugin (e.g. AI Agent) to learn
   its shape.
8. Document any new generic capability before adding it to a stable API entry
   point. Keep plugin protocols, defaults, and unique styles local.

External plugins will follow the same manifest/main/ui split once the loader
in `externalLoader.ts` is implemented.
