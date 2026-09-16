# WaSSH audit — fix list

2026-09-16. Rubric: `.github/copilot-instructions.md`; plugins additionally PLUGIN_API.md §14 (imports allowed: local, 3rd-party, `@plugin-api/{shared,main,renderer}`; violations: host resources without an API, core↔plugin internal imports, barrel bypasses).
Fix policy: low-risk fixes only (dead code, named constants, local DRY, narrow catches, small root-cause bugs); big refactors → §6. Strict plugin isolation: close all leaks, small API additions allowed.
Markers: items are unverified recon leads — confirm before fixing; `(verified)` = grep-checked; `(decision)` = needs a call on behavior. Ledger: `[ ]` open · `[x]` fixed · append `— defer` / `— rejected`.

## Verification
- `npm run typecheck` clean after every batch.
- After §5: no `../../../` imports and no `from 'electron'` under `src/plugins/builtins/**`; no `plugins/builtins/**` imports from `src/main/**` or `src/renderer/src/components/**` outside the two `builtinRegistry` composition roots.
- Smoke (`npm run dev`): boot; SSH/telnet/serial connect+reconnect; all 8 plugin panels; terminal→SFTP drag-drop upload; AI provider dialog; Options + host dialogs plugin settings.

## 1. Main process

### 1.1 Bugs
- [x] `index.ts:458-469` — double-dispose + macOS `pluginSystem` never rebuilt. Fixed: disposal happens only in `before-quit`; `window-all-closed` just quits on non-mac.
- [x] `index.ts:104-121` — `migrateLegacyUserData` ran every start. Fixed: `.legacy-data-migrated` marker written after a completed copy.
- [x] `index.ts:80-97` — error guards. Resolved per decision: the process keeps running on unexpected errors; both logs now carry a `[main]` prefix so they are identifiable.
- [x] `index.ts:123-146` — wake reconnect delay. Fixed per decision: resume reconnects immediately via `reconnectOnFocus()`; `WAKE_RECONNECT_DELAY_MS` deleted; the retry policy owns failed attempts.
- [x] `SshConnection.ts:76,481-485` — `hostKeyWait` single slot overwritten by concurrent prompts. Fixed: prompts are serialized (`hostKeyPromptChain`); `respondHostKey` answers the one outstanding prompt.
- [x] `SshConnection.ts:517-552` — `promptTerminal` never settled. Fixed: `cancelTerminalPrompt()` settles with an empty answer on transport close, `onTransportError`, and `closeClientOnly` (dispose/sleep/re-open).
- [x] `SshConnection.ts:859-898` — `startShell` had no settle path. Fixed: `connect()` resolves on `connected`; a client close before the channel opens fails the attempt (`opening` guard) and settles.
- [ ] `SshConnection.ts:689-696,722-757` — guard/connectClient late errors. — rejected: `guardClient` routes only live/proxy clients (intended reconnect path); a failed attempt's client is never assigned to `this.client`, so late errors are ignored by design.
- [x] `SshConnection.ts:149-181` — `respondSavePassword` duplicated host re-wiring. Fixed: shared `applySavedPassword()`. A decision arriving after `pendingSavePassword` was consumed still no-ops — the password is no longer available.
- [x] `SessionManager.ts:153-159` — duck-typed `'updateConnection' in conn` guard. Fixed: direct call.
- [x] `ByteSession.ts:138-152` — "Session closed" emitted for network drops. Fixed: drop branch emits a bare `disconnected`; the message is reserved for remote-ended sessions.
- [x] `TelnetConnection.ts:83-93,339-340` — NAWS frame sent twice at start. Fixed: `nawsEnabled` is set (and the size sent) only after the server's `DO NAWS`; the premature frame is gone. Re-sends on resize are intended (~9-byte frames).
- [x] `sessionStore.ts:72-83,118-120,226` — per-host `settings.json` re-reads (N+1). Fixed: one `readStoredSettings()` per load (`normalizedDocument`, `getTabs`, `saveHost`); dead `fallbackMode` variable in `loadHostsDocument` removed; `migrateReconnectModes` (hidden `new TabStore()`) replaced by explicit `tabStore.getTabs()` in index.ts. Reconciliation writes only when the org changed; the `getTabs` migration write is the intended one-shot mechanism.
- [x] `credentialVault.ts` — unencrypted fallback. Fixed per decision (warn at save time): `isEncryptionAvailable()` exposed via `vault:encryptionAvailable`; the save-password banner shows a warning, host save and AI-provider key save require confirmation, and main logs a warning when storing unencrypted. `get()` still returns null for both absent and decrypt-failed.
- [x] `pluginDataStore.ts:36-45` — corrupt JSON indistinguishable from no data (`null`). Fixed: parse failures are logged.
- [x] `listSerialPorts.ts:4-16` — catch-all returns `[]`. Fixed: failures are logged.
- [ ] `updater.ts:95-159` — module-global state; `update-available` dialog outside `checkInProgress` guard. — rejected: the prompt is modal on the parent window so overlapping checks cannot occur; `getWindow` default covers non-updatable builds. Named the `win-unpacked` literal.
- [ ] `windowBounds.ts:59-87` — close-time `persist()` overwrite; no listener cleanup on destroy. — rejected: `persist()` checks `isDestroyed`, close-time persist writes the latest bounds (correct), listeners die with the window object.

### 1.2 Dead code
- [x] `src/main/plugins/aiAgent/{permissions,providers,tools}.ts` — dead re-export shims; directory deleted. (verified)
- [x] `SessionManager.ts:148-150` `getConnection` — deleted.
- [x] `pluginDataStore.ts:8` `pluginDataFileName`, `windowBounds.ts:47` `readWindowBounds` — de-exported; `SshKeyManager.ts:76-80` `getDefaultSshDir`/`ensureSshDir` — made private.
- [ ] `ByteSession.ts:69-75` `respondHostKey`/`respondSavePassword` — no-op stubs. — rejected: intentional; keeps the `LiveSession` union uniform and avoids `instanceof` branching in `SessionManager`.
- [ ] `SshConnection.ts:443-445` — post-ready error-absorb listener on duplicate clients. — rejected: deliberate crash guard for plugin-held clients; `quietEnd` removes all listeners on dispose.
- [x] `SessionManager.ts:231-233` `reconnectOnWake` — deleted; index.ts calls `reconnectOnFocus()`.

### 1.3 DRY
- [ ] `SshConnection` re-implements `ByteSession` (`getConnection`/`updateConnection`/`setReconnectPolicy`/`wantsFocusReconnect`/`connect`/`disconnect`/`dispose`/`prepareForSleep`/`reconnectNow`/`emitStatus`/`scheduleReconnect`/`clearReconnectTimer`); dup constants `SESSION_CLOSED_MESSAGE` (`SshConnection.ts:35` vs `ByteSession.ts:22`) and `MS_PER_SECOND` (`:37` vs `:25`); backoff bodies `SshConnection.ts:1174-1201` vs `ByteSession.ts:165-194`. → §6.1
- [ ] `SshConnection.ts:722-757` ≈ `:414-451` ≈ `SshKeyManager.ts:392-423` (connect-client ×3); `forwardThrough` `SshConnection.ts:759-776` ≈ `:453-470` ≈ `SshKeyManager.ts:425-437`. → §6.4
- [ ] `execCapture` `SshConnection.ts:286-333` ≈ `probeRemoteSession` `:1063-1127` (same channel collect/`settled`/finish skeleton). → §6.4
- [ ] `TelnetConnection.ts:263-345` `open` ≈ `SerialConnection.ts:44-121` `open` (same 9-step skeleton). → §6.6
- [x] `TelnetConnection.ts:42-51` `iacEscape` ≈ `escapeWrite` — merged (`escapeWrite` delegates to `iacEscape`).
- [ ] JSON persistence tripled: `sessionStore.ts:38-60`, `pluginDataStore.ts:17-29,36-49`, `credentialVault.ts:14-42`; `JSON_INDENT` ×3 (`sessionStore.ts:36`, `credentialVault.ts:6`, `pluginDataStore.ts:5`). → §6.2
- [x] `handlers.ts` — file-dialog window/no-window branch duplication. Fixed: shared `showOpenDialog(getWindow, options)` + `firstPickedPath()` helpers.
- [x] `index.ts` `sendToRenderer` dup of `windowSend.sendToWindow` — now delegates to `sendToWindow`. (`SessionManager.send` is kept as the getWindow binder.)
- [x] Reconnect-mode derivation applied twice — removed the redundant `setReconnectPolicy` call in `SessionManager.wire()` (constructors already set it).
- [x] `TunnelManager.ts` handler storage — fields typed, all `as (...args: unknown[])` casts removed. `SshKeyManager.ts:294-322` dummy `HostProfile` — defer: replacing it needs a shared HostProfile defaults factory (§6).

### 1.4 Literals (named constants at file top)
- [x] `index.ts` `'WaSSH'` → `APP_NAME`; `updater.ts` `'win-unpacked'` → `UNPACKED_BUILD_DIR_NAME`; `handlers.ts` dialog titles → `PICK_PRIVATE_KEY_TITLE`/`PICK_DIRECTORY_TITLE`. `'win32'` — rejected (platform identifier, not a tunable literal).
- [x] `SshConnection.ts` `'ssh-hostkey'` ×2 → `SSH_HOST_KEY_TYPE`; `pwd-` prefix ×2 → `PASSWORD_VAULT_ID_PREFIX`; forward origin ×3 → `SSH_FORWARD_SOURCE_IP`/`SSH_FORWARD_SOURCE_PORT` (new shared constants; TunnelManager/SshKeyManager local duplicates deleted). `'WINCH'` — rejected (POSIX signal name).
- [x] `TelnetConnection.ts` `'xterm-256color'` → `DEFAULT_TERM_TYPE`. Raw option byte arrays — rejected (protocol constants, already named).
- [x] `TunnelManager.ts` — bind-host spellings → `REMOTE_BIND_HOST_ALIASES`; `4` → `SOCKS_CONNECT_HEADER_LEN`. SOCKS reply byte array — rejected (protocol filler inside named `socksReply()`).

### 1.5 Convoluted (simplify; large ones → §6)
- [x] Reviewed — no low-risk single-spot fixes. Routed: `handlers.registerIpc` split → §6.7; `updater.setupAutoUpdater` split → §6.8; `TunnelManager.handleSocksClient`/`SshKeyManager.runRemoteRetrieve` → §6.9; `SshConnection` open/startShell/openInteractiveChannel → §6.1/§6.4; `sessionStore` load/normalize → §6.2; `index.createWindow`/`installAppMenu` are declarative Electron setup — rejected (split adds indirection without clarity); `TelnetFilter.step` → §6.6.

### 1.6 Encapsulation
- [x] Inline `import()` types removed (`handlers.ts` → `ConnectionParams` import, `SshConnection.ts` → `ClientChannel`); TunnelManager handler casts removed (typed fields); `listSerialPorts` widening named + documented (`WindowsPortInfo`). `sessionStore.ts:333-336` raw-JSON cast — defer: typing untrusted file reads is a §6-level refactor.

## 2. Shared & preload
- [ ] `connection.ts:328-333` — deprecated `sessionStyleFrom` still called: `connection.ts:720,746`, `sessionStore.ts:134,341`, `QuickConnect.tsx:110`. Migrate callers or drop the deprecation.
- [ ] `env.d.ts:11-14` — duplicate `Window.wassh` declaration (also in `types.ts` ~712).
- [ ] `preload/index.ts:84-85` + `types.ts:696-698` — `getCommandPaletteData`/`setCommandPaletteData`: no main handler, no callers; `preload/index.ts:108-111` + `types.ts:699-700` — `onSideConnectionData`/`onSideConnectionClosed`: no subscribers. Delete.
- [ ] `pluginLayout.ts:382-409` — `edgeToPlacement`/`findPluginEdge` dead; `:181-220` `splitLeafNode` ≡ `:222-257` `wrapSplit` (identical 4-branch builders) — merge.
- [ ] `hostOrganization.ts:158-164` — private helper duplicated in `SessionsSidebar.tsx:125-130`; export/reuse one.
- [ ] `pluginLayout.ts:127` — `MIN_DOCK_SIZE_PX * 2` used in ratio math; confirm semantics/naming.

## 3. Renderer core

### 3.1 Bugs
- [ ] `App.tsx:477-497` — `setActiveTabId` called inside the `setTabs` updater (impure updater; StrictMode double-invoke hazard).
- [ ] `App.tsx:805-830` — `uploadDroppedFiles` wraps the chunk loop in one `try/catch`; failure invisible (progress banner just vanishes). Resolved by §5 sftp decoupling.
- [ ] `App.tsx:691-774` — `onPluginSettingsPatch` merges from `settingsRef` then `setSettings`; races the `settings:changed` broadcast (`:636-638`) → clobbering.
- [ ] `App.tsx:263,639-650` vs `sftp/fileDrop.ts:29,43-58` — two independent SFTP readiness sources parsing the same status; `dropEnabled` read from ref during render, kept live by a tick counter (`:1799`). Resolved by §5.
- [ ] `TabBar.tsx:283-321` — `.tab-close` is `span role="button"` nested inside the tab `<button>`; its `onKeyDown` is unreachable.
- [ ] `PluginPanelShell.tsx:20-32` — grip `role="button" tabIndex={0}` with no keyboard activation.
- [ ] `AiAgentProviderDialog.tsx:225-231` — `removeProvider` picks next selection from stale `drafts`; `:232-250` `save` is `try/finally` without `catch` → unhandled rejection, dialog silently stays open. Also §5 (moves into plugin).
- [ ] `SerialPortField.tsx:37-40` — `typeof list !== 'function'` guards a bridge method that always exists (dead branch).
- [ ] `TerminalView.tsx:102-118` — reads xterm private internals (`_charSizeService`, `_renderService.dimensions.css.canvas`); version-fragile, needs feature detection/fallback. (decision)
- [ ] `App.tsx:1204-1240` — three identical `^#[0-9A-Fa-f]{6}$` literals + three near-identical `sessionStyleDefaults` updates.
- [ ] `SessionsSidebar.tsx:111-123` — menu flip uses hard-coded 160 px height estimate; only X clamped (tall menus overflow).

### 3.2 Dead
- [ ] `App.tsx:68` — `sessionAccentStyle` imported, never used.
- [ ] `registry.ts:12-15` `viewPlacementFor`; `builtinRegistry.tsx:32` `getPluginRenderer` — no callers.
- [ ] `builtinRegistry.tsx:13` `BUILTIN_RENDERER_PLUGINS` — exported, only file-internal.

### 3.3 DRY
- [ ] `HostSessionSettingsDialog.tsx:131-154` ≡ `OptionsDialog.tsx:57-81` — `themeVarHex`, `HEX_COLOR_RE`, theme-var constants byte-identical.
- [ ] `HostSessionSettingsDialog.tsx:156-224` `ColorRow` ≈ `OptionsDialog.tsx:126-178` `DefaultsColorRow`.
- [ ] `App.tsx:141-149` `formatBytes` vs `sftp/viewUtils.ts` (plugin copy — dedupe via §5 decision).
- [ ] Provider loading `OptionsDialog.tsx:196-203` vs `AiAgentProviderDialog.tsx:80-107`; `checkSelected` ≈ `refreshSelected` (`AiAgentProviderDialog.tsx:110-209`). Also §5.
- [ ] Gap-index helpers `TabBar.tsx:54-97` ≈ `SessionsSidebar.tsx:181-199`.
- [ ] Serial format selects `QuickConnect.tsx:165-230` vs `HostSessionSettingsDialog.tsx:~480-575` (five selects each).
- [ ] Connection normalization pipeline repeated ×6: `App.tsx:309-323,341-360,449-465,1466-1478` + `connection.ts:714-731,736-756`.
- [ ] Empty-host construction ×3: `App.tsx:193-213`, `connection.ts:736-756`, `QuickConnect.tsx:88-118` (`emptyHost()` too: pure-looking factory generating a UUID).
- [ ] Dialog shell markup ×3: `AboutDialog.tsx:31-70`, `AiAgentProviderDialog.tsx:253-330`, `SettingsDialog.tsx:180-225`.
- [ ] `PluginSessionFrame.tsx:600-760` — 9 copies of the conditional drop-overlay markup → data-driven. Also §6.5.

### 3.4 Literals
- [ ] `App.tsx:809` `256 * 1024` duplicates `SFTP_UPLOAD_CHUNK_SIZE` (`sftp/fileDrop.ts:27`); `App.tsx:1196` `8`/`48` vs font-size constants.
- [ ] Plugin-id literals → use each plugin's `id.ts`: `OptionsDialog.tsx:196,284`, `AiAgentProviderDialog.tsx:18`, `renderer/src/plugins/builtinRegistry.tsx:14-21`.
- [ ] Section-id conventions coupled by convention: `plugin-${id}` (OptionsDialog) vs `plugin-host-${id}` (`App.tsx:1104`).
- [ ] `SshKeySettingsGroup.tsx:243,254,300,330` inline style gaps; `AiAgentProviderDialog.tsx:322` hard-coded ollama URL `'http://127.0.0.1:11434/v1'`; `TerminalSearchBar.tsx:8-11` `FIND_PREV_KEY = 'Enter'` misnamed (used both directions); `SessionsSidebar.tsx:~870` bare `8`.

### 3.5 Broad try/catch
- [ ] `App.tsx:805-824` catch-all no logging; `SshKeySettingsGroup.tsx:132-150,172-186,208-222` whole handlers in try, all causes collapse to a status string; `AiAgentProviderDialog.tsx:143-165,190-209` one try spans getActivePlugins+activate+send+validate; `SerialPortField.tsx:52-56` and `SshKeySettingsGroup.tsx:91-96,105-112` `.catch(() => fallback)` without empty-vs-failed distinction.

### 3.6 Convoluted (large ones → §6.3)
- [ ] `App.tsx:1261-1899` single return ~640 lines; `:1087-1243` `executeCommand`; `:558-690` 18-listener effect; `:691-774` `onPluginSettingsPatch`; `HostSessionSettingsDialog.tsx:348-~1090` `sections` useMemo ~740 lines; `OptionsDialog.tsx:276-~530`; `TerminalView.tsx:168-397`; `SessionsSidebar.tsx:300-510`; `PluginFieldEditor.tsx:107-190` `ItemListEditor`; `TunnelBuilder.tsx:220-401`; `SshKeySettingsGroup.tsx:61-220`.

## 4. Plugins

Per-plugin rubric (§14 compliance also at §5): standard quality findings; allowed imports only; `window.wassh` use is allowed for views; manifest/defaults/schema consistency; dead `protocol.ts` guards; `plugin-ui-*` CSS only; no cross-plugin imports.

### 4.1 scratchpad — audit pending (id, main, manifest, protocol, View, styles)
### 4.2 macro-pad — audit pending (defaults, id, main, manifest, protocol, View, styles)
### 4.3 connection-logger — audit pending (defaults, id, main, manifest, protocol, View, styles)
### 4.4 daemon-monitor — audit pending (defaults, id, main, manifest, protocol, remoteService, View, styles)
### 4.5 mqtt-analyser — audit pending (defaults, id, main, manifest, protocol, View, styles)
### 4.6 server-monitor — audit pending (defaults, id, main, manifest, protocol, View, styles)
### 4.7 sftp
- [ ] Relative imports bypassing aliases → `@plugin-api/*`: `main.ts:1-2`, `helpers.ts:1-2`, `fileView.ts:1-2`, `transfers.ts:13-14` (`../../../main/plugins/api`); `manifest.ts:1`, `fileDrop.ts:1` (`../../../shared/pluginApi`); `View.tsx:10`, `fileDrop.ts:2` (`../../../renderer/src/plugins/api`). `protocol.ts:5` already uses the alias.
- [ ] `transfers.ts:10` imports Electron `{BrowserWindow, dialog}` — replace with new API (§5).
- [ ] `fileDrop.ts:146-152` — per-file `catch` swallows errors (only cancel distinguished); surface failures.
- [ ] `fileDrop.ts:29-31,64-76` — module-level singletons (`readyTabs`, `activeUploads`, `trackingStarted`) + listeners registered once, never removed.
- [ ] `fileDrop.ts:111` — `cancelSftpFileDrop` exported but only file-internal.
### 4.8 ai-agent
- [ ] `View.tsx:198,202` — `isFileDrag`/`collectDroppedFiles` duplicate `sftp/fileDrop.ts:81,86` (same semantics, two copies). Decide: neutral helper in `@plugin-api/renderer` (needs doc per §14.8) or keep. (decision)
- [ ] Audit pending otherwise (`main.ts` 2437 lines, `tools.ts` ≥1308, `View.tsx` ≥1170, plus apiMethods, defaults, id, manifest, permissions, pluginApiTools, protocol, providers, rag, terminal).

## 5. Plugin isolation (strict — close now)

- [ ] Normalize sftp imports (see 4.7) to `@plugin-api/*` aliases.
- [ ] Add file-dialog capability: neutral `showOpenDialog`/`showSaveDialog` (narrow option/result types, no Electron types leaked) on `PluginMainContext` (`main/plugins/api.ts`), implemented in `PluginHost` via the existing `getWindow` dep; replace `sftp/transfers.ts` Electron usage; document in PLUGIN_API.md §5.
- [ ] Break App↔sftp coupling: remove `App.tsx:43-49` sftp imports and the duplicate upload/status logic (`:263,639-650,797-830,805`); resolve terminal file-drop generically from the renderer plugin registration (`fileDrop`, already registered for sftp) + tab `activePluginIds`; if registration becomes the single source, delete the dead `terminalFileDrop` surface (`shared/pluginApi.ts:166,191`, `sftp/manifest.ts:14`).
- [ ] Break core→ai-agent coupling: move provider-config UI ownership into the plugin (own defaults/id/protocol); add a minimal generic settings hook so `OptionsDialog` renders plugin-provided settings UI without plugin imports; remove the `populateAiAgentSchema` and `configureProviders` special cases (`OptionsDialog.tsx:34-39,196-203,284,492-505,530-540`) and `AiAgentProviderDialog.tsx:1-12` imports; keep current UX (dialog openable from Options); document in PLUGIN_API.md §11/§12.
- [ ] Barrel leak: `renderer/src/plugins/api/index.ts:38` re-exports `PluginSettingsFieldList` from `../PluginSettingsFieldList`; relocate `PluginSettingsFieldList`/`PluginFieldEditor` under `api/` (or clean re-export); verify plugin views use only `plugin-ui-*` classes.
- [ ] Renderer builtin registry uses literal ids — use each plugin's `id.ts`.
- [ ] Re-run the §Verification greps.

## 6. Proposals (ranked; do NOT execute this pass)
1. `SshConnection` on top of `ByteSession` — removes ~200 dup lines, dup constants, dup backoff.
2. Store layer — shared JSON helper; `sessionStore` I/O cleanup (N+1 reads, write-in-getter, hidden `TabStore`, reconciliation heuristics).
3. `App.tsx` decomposition (~1895 lines; state machine vs render split).
4. Connect-client ×3 / `forwardThrough` ×3 / `execCapture`≈`probeRemoteSession` unification.
5. `PluginSessionFrame` — data-driven drop overlays; split 757-line component.
6. Telnet/serial `open` skeleton unification; `TelnetFilter.step` simplification.
7. `handlers.registerIpc` — split into per-domain registration functions (ssh keys / settings / hosts / tabs / vault / sessions / dialogs / plugins).
8. `updater.setupAutoUpdater` — split listeners into named handlers; move the module-level state into one object.
9. `TunnelManager.handleSocksClient` phase machine extraction; `SshKeyManager.runRemoteRetrieve` delimiter arithmetic.

## 7. Doc drift
- [ ] PLUGIN_API.md: settings-field table lists 8 types, code defines 12 (`textArea`, `directory`, `itemList`, `action`, `permission`); `PluginApiMethod.label`/`defaultPermission` undocumented; `getPluginData(pluginId, scopeId?)` scopeId missing; §12 UI-kit list incomplete; `terminalFileDrop` (resolve with §5).
- [ ] `main/plugins/api.ts:13-14` re-exports concrete implementations (`classifySftpError`, `joinRemotePath`, `SftpSession`) through the API entry — decide intent and document.
- [ ] Update PLUGIN_API.md tables after the §5 additions.
