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
- [x] `SshConnection.ts:689-696,722-757` — guard/connectClient late errors. — rejected: `guardClient` routes only live/proxy clients (intended reconnect path); a failed attempt's client is never assigned to `this.client`, so late errors are ignored by design.
- [x] `SshConnection.ts:149-181` — `respondSavePassword` duplicated host re-wiring. Fixed: shared `applySavedPassword()`. A decision arriving after `pendingSavePassword` was consumed still no-ops — the password is no longer available.
- [x] `SessionManager.ts:153-159` — duck-typed `'updateConnection' in conn` guard. Fixed: direct call.
- [x] `ByteSession.ts:138-152` — "Session closed" emitted for network drops. Fixed: drop branch emits a bare `disconnected`; the message is reserved for remote-ended sessions.
- [x] `TelnetConnection.ts:83-93,339-340` — NAWS frame sent twice at start. Fixed: `nawsEnabled` is set (and the size sent) only after the server's `DO NAWS`; the premature frame is gone. Re-sends on resize are intended (~9-byte frames).
- [x] `sessionStore.ts:72-83,118-120,226` — per-host `settings.json` re-reads (N+1). Fixed: one `readStoredSettings()` per load (`normalizedDocument`, `getTabs`, `saveHost`); dead `fallbackMode` variable in `loadHostsDocument` removed; `migrateReconnectModes` (hidden `new TabStore()`) replaced by explicit `tabStore.getTabs()` in index.ts. Reconciliation writes only when the org changed; the `getTabs` migration write is the intended one-shot mechanism.
- [x] `credentialVault.ts` — unencrypted fallback. Fixed per decision (warn at save time): `isEncryptionAvailable()` exposed via `vault:encryptionAvailable`; the save-password banner shows a warning, host save and AI-provider key save require confirmation, and main logs a warning when storing unencrypted. `get()` still returns null for both absent and decrypt-failed.
- [x] `pluginDataStore.ts:36-45` — corrupt JSON indistinguishable from no data (`null`). Fixed: parse failures are logged.
- [x] `listSerialPorts.ts:4-16` — catch-all returns `[]`. Fixed: failures are logged.
- [x] `updater.ts:95-159` — module-global state; `update-available` dialog outside `checkInProgress` guard. — rejected: the prompt is modal on the parent window so overlapping checks cannot occur; `getWindow` default covers non-updatable builds. Named the `win-unpacked` literal.
- [x] `windowBounds.ts:59-87` — close-time `persist()` overwrite; no listener cleanup on destroy. — rejected: `persist()` checks `isDestroyed`, close-time persist writes the latest bounds (correct), listeners die with the window object.

### 1.2 Dead code
- [x] `src/main/plugins/aiAgent/{permissions,providers,tools}.ts` — dead re-export shims; directory deleted. (verified)
- [x] `SessionManager.ts:148-150` `getConnection` — deleted.
- [x] `pluginDataStore.ts:8` `pluginDataFileName`, `windowBounds.ts:47` `readWindowBounds` — de-exported; `SshKeyManager.ts:76-80` `getDefaultSshDir`/`ensureSshDir` — made private.
- [x] `ByteSession.ts:69-75` `respondHostKey`/`respondSavePassword` — no-op stubs. — rejected: intentional; keeps the `LiveSession` union uniform and avoids `instanceof` branching in `SessionManager`.
- [x] `SshConnection.ts:443-445` — post-ready error-absorb listener on duplicate clients. — rejected: deliberate crash guard for plugin-held clients; `quietEnd` removes all listeners on dispose.
- [x] `SessionManager.ts:231-233` `reconnectOnWake` — deleted; index.ts calls `reconnectOnFocus()`.

### 1.3 DRY
- [x] `SshConnection` re-implements `ByteSession` (`getConnection`/`updateConnection`/`setReconnectPolicy`/`wantsFocusReconnect`/`connect`/`disconnect`/`dispose`/`prepareForSleep`/`reconnectNow`/`emitStatus`/`scheduleReconnect`/`clearReconnectTimer`); dup constants `SESSION_CLOSED_MESSAGE` (`SshConnection.ts:35` vs `ByteSession.ts:22`) and `MS_PER_SECOND` (`:37` vs `:25`); backoff bodies `SshConnection.ts:1174-1201` vs `ByteSession.ts:165-194`. → §6.1 (routed)
- [x] `SshConnection.ts:722-757` ≈ `:414-451` ≈ `SshKeyManager.ts:392-423` (connect-client ×3); `forwardThrough` `SshConnection.ts:759-776` ≈ `:453-470` ≈ `SshKeyManager.ts:425-437`. → §6.4 (routed)
- [x] `execCapture` `SshConnection.ts:286-333` ≈ `probeRemoteSession` `:1063-1127` (same channel collect/`settled`/finish skeleton). → §6.4 (routed)
- [x] `TelnetConnection.ts:263-345` `open` ≈ `SerialConnection.ts:44-121` `open` (same 9-step skeleton). → §6.6 (routed)
- [x] `TelnetConnection.ts:42-51` `iacEscape` ≈ `escapeWrite` — merged (`escapeWrite` delegates to `iacEscape`).
- [x] JSON persistence tripled: `sessionStore.ts:38-60`, `pluginDataStore.ts:17-29,36-49`, `credentialVault.ts:14-42`; `JSON_INDENT` ×3 (`sessionStore.ts:36`, `credentialVault.ts:6`, `pluginDataStore.ts:5`). → §6.2 (routed)
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
- [x] `connection.ts:328-333` — deprecated `sessionStyleFrom` still called. Fixed: all 5 call sites migrated to `sessionStyleOverridesFrom`; the deprecated wrapper is deleted.
- [x] `env.d.ts:11-14` — duplicate `Window.wassh` declaration. Fixed: `env.d.ts` keeps only `queryLocalFonts`; `Window.wassh` is declared once in `types.ts`.
- [x] `preload/index.ts:84-85` + `types.ts:696-698` — `getCommandPaletteData`/`setCommandPaletteData` (no handler, no callers) and `onSideConnectionData`/`onSideConnectionClosed` (no subscribers) deleted from preload, `WasshApi`, and the `pluginApi` import list. (verified)
- [x] `pluginLayout.ts:382-409` — `edgeToPlacement`/`findPluginEdge` dead (deleted); `splitLeafNode` ≡ `wrapSplit` — merged into `wrapSplit`.
- [x] `hostOrganization.ts:158-164` — private helper duplicated in `SessionsSidebar.tsx`. Fixed: `hostIdsForSection` exported from `hostOrganization.ts`; the sidebar copy is deleted.
- [x] `pluginLayout.ts:127` — `MIN_DOCK_SIZE_PX * 2` named `MIN_SPLIT_CONTAINER_PX` (smallest container fitting two panes at minimum).

## 3. Renderer core

### 3.1 Bugs
- [x] `App.tsx:477-497` — `setActiveTabId` called inside the `setTabs` updater. Fixed: `closeTab` computes the next list from `tabsRef.current`, then calls `setTabs` and `setActiveTabId` separately (pure updater).
- [x] `App.tsx:805-830` — `uploadDroppedFiles` wraps the chunk loop in one `try/catch`; failure invisible (progress banner just vanishes). Resolved: the whole function was deleted with the terminal-drop feature (§5).
- [x] `App.tsx:691-774` — `onPluginSettingsPatch` clobbered by the `settings:changed` broadcast. Fixed: the `.then(setSettings)` echo is dropped in `updateSettings` and `onPluginSettingsPatch`; the main process broadcast is the single authoritative source (`updateSettings` still refreshes plugins).
- [x] `App.tsx:263,639-650` vs `sftp/fileDrop.ts:29,43-58` — two independent SFTP readiness sources parsing the same status; `dropEnabled` read from ref during render, kept live by a tick counter (`:1799`). Resolved: both readiness trackers were deleted with the terminal-drop feature (§5).
- [x] `TabBar.tsx:283-321` — `.tab-close` was `span role="button"` nested inside the tab `<button>` (invalid HTML; `onKeyDown` unreachable). Fixed: now a presentational span (`aria-hidden`); keyboard close stays via the tab context menu / `close-active` command.
- [x] `PluginPanelShell.tsx:20-32` — grip `role="button" tabIndex={0}` with no keyboard activation. Fixed: pointer-only drag handle (role/tabIndex/aria-label removed).
- [x] `AiAgentProviderDialog.tsx:225-231` — `removeProvider` picked the next selection from stale `drafts`. Fixed: selects from the filtered list. `:232-250` `save` `try/finally` without `catch` → unhandled rejection. Fixed: `saveError` state surfaced in the footer. (UI still moves into the plugin per §5.)
- [x] `SerialPortField.tsx:37-40` — `typeof list !== 'function'` guarded a bridge method that always exists. Fixed: dead branch removed.
- [x] `TerminalView.tsx:102-118` — reads xterm private internals (`_charSizeService`, `_renderService.dimensions.css.canvas`). Reviewed: already feature-detected via optional chaining + `?? 0` fallback, typed by the documented `XtermCoreInternals` interface; a renamed internal degrades (no height pin) rather than crashing. No change needed.
- [x] `App.tsx:1204-1240` — three identical `^#[0-9A-Fa-f]{6}$` literals + three near-identical `sessionStyleDefaults` updates. Fixed: `HEX_COLOR_RE` at file top (the three updates remain distinct by key).
- [x] `SessionsSidebar.tsx:111-123` — menu flip uses hard-coded 160 px height estimate; only X clamped (tall menus overflow). → §6 (needs menu measurement) (routed).

### 3.2 Dead
- [x] `App.tsx:68` — `sessionAccentStyle` imported, never used. Fixed: import removed.
- [x] `registry.ts:12-15` `viewPlacementFor`; `builtinRegistry.tsx:32` `getPluginRenderer` — deleted (no callers); unused `PluginViewPlacement` import removed.
- [x] `builtinRegistry.tsx:13` `BUILTIN_RENDERER_PLUGINS` — de-exported (file-internal).

### 3.3 DRY
- [x] `HostSessionSettingsDialog.tsx:131-154` ≡ `OptionsDialog.tsx:57-81` — `themeVarHex`, `HEX_COLOR_RE`, theme-var constants byte-identical. Fixed: extracted to `components/settingsColor.ts`.
- [x] `HostSessionSettingsDialog.tsx:156-224` `ColorRow` ≈ `OptionsDialog.tsx:126-178` `DefaultsColorRow`. Fixed: unified into `components/SettingsColorRow.tsx` (`defaultLabel` + optional `resolvedFallback` cover both).
- [x] `App.tsx:141-149` `formatBytes` vs `sftp/viewUtils.ts` (plugin copy). — defer: the four copies (also server-monitor, daemon-monitor) differ in zero-case output (`'—'` vs `'0 B'`) and decimal digits, so unifying changes displayed values; needs a deliberate shared contract with a `zeroLabel`/precision parameter — §6-scale, not a local dedupe.
- [x] Provider loading `OptionsDialog.tsx:196-203` vs `AiAgentProviderDialog.tsx:80-107`; `checkSelected` ≈ `refreshSelected` (`AiAgentProviderDialog.tsx:110-209`). Resolved by §5 core→ai-agent decoupling (routed).
- [x] Gap-index helpers `TabBar.tsx:54-97` ≈ `SessionsSidebar.tsx:181-199`. Fixed: `insertIndexFromGap` extracted to `renderer/src/dragReorder.ts`; the sidebar wrapper is gone.
- [x] Serial format selects `QuickConnect.tsx:165-230` vs `HostSessionSettingsDialog.tsx:~480-575`. Fixed: option lists extracted to `components/serialOptions.tsx` (`SERIAL_DATA_BITS_OPTIONS`, `SERIAL_PARITY_OPTIONS`, `SERIAL_STOP_BITS_OPTIONS`, `serialFlowOptions(verbose)`).
- [x] Connection normalization pipeline repeated ×6: `App.tsx:309-323,341-360,449-465,1466-1478` + `connection.ts:714-731,736-756`. Fixed: `normalizeConnectionParams()` added to `connection.ts` (generic over `ConnectionParams`/`HostProfile`, `clearEphemeral` flag); the three App.tsx `ConnectionParams` sites and `refreshHosts` now call it.
- [x] Empty-host construction ×3: `App.tsx:193-213`, `connection.ts:736-756`, `QuickConnect.tsx:88-118`. Fixed: `emptyConnectionParams(type)` + `emptyHostProfile(id)` added to `connection.ts`; QuickConnect spreads the former, App.tsx `emptyHost()` wraps the latter (UUID stays at the call site).
- [x] Dialog shell markup ×3: `AboutDialog.tsx:31-70`, `AiAgentProviderDialog.tsx:253-330`, `SettingsDialog.tsx:180-225`. Fixed: `components/DialogShell.tsx` (overlay + header + optional footer; `baseClass`, `focusable`, `closeOnBackdrop`, `closeOnEscape` props). All three dialogs use it; `SettingsDialog` keeps its capture-phase Escape + focus effect via `closeOnEscape={false}`.
- [x] `PluginSessionFrame.tsx:600-760` — 9 copies of the conditional drop-overlay markup. Fixed: `DropZone` component + `isDropZone()` helper; the outer/inner overlays and `renderDock` class logic use them.

### 3.4 Literals
- [x] `App.tsx:1196` `8`/`48` → `FONT_SIZE_MIN_PX`/`FONT_SIZE_MAX_PX`; `App.tsx:1204` three `^#[0-9A-Fa-f]{6}$` literals → file-top `HEX_COLOR_RE`.
- [x] Plugin-id literals → `builtinRegistry.tsx` now uses each plugin's `id.ts`. (`OptionsDialog.tsx`/`AiAgentProviderDialog.tsx` handled with §5.)
- [x] `App.tsx:809` `256 * 1024` duplicates `SFTP_UPLOAD_CHUNK_SIZE` (`sftp/fileDrop.ts:27`) — resolved: the App.tsx copy was deleted with the terminal-drop feature (§5); `SFTP_UPLOAD_CHUNK_SIZE` remains the single definition.
- [x] Section-id conventions coupled by convention: `plugin-${id}` (OptionsDialog) vs `plugin-host-${id}` (`App.tsx:1104`). Fixed: `pluginSettingsSectionId()`/`pluginHostSettingsSectionId()` added to `shared/pluginApi.ts`; all four set/read sites use them.
- [x] `SshKeySettingsGroup.tsx:243,254,300,330` inline style gaps → `.ssh-key-row` CSS class (inline styles removed); `AiAgentProviderDialog.tsx:322` ollama URL → `AI_AGENT_OLLAMA_BASE_URL`; `TerminalSearchBar.tsx:8-11` `FIND_PREV_KEY` → `FIND_SUBMIT_KEY`/`FIND_SUBMIT_FKEY` (Shift inverts); `SessionsSidebar.tsx:~870` bare `8` → `GROUP_COLOR_POP_VIEWPORT_MARGIN_PX`.

### 3.5 Broad try/catch
- [x] `App.tsx:805-824` catch-all no logging; `SshKeySettingsGroup.tsx:132-150,172-186,208-222` whole handlers in try; `AiAgentProviderDialog.tsx:143-165,190-209` one try spans getActivePlugins+activate+send+validate; `SerialPortField.tsx:52-56` and `SshKeySettingsGroup.tsx:91-96,105-112` `.catch(() => fallback)`. — reviewed: the `SshKeySettingsGroup` handlers wrap only the `await` and map failures to a status line (intended UX); the `.catch(() => fallback)` sites have intentional empty fallbacks. No renderer logging/toast convention exists, so adding one here would be a new pattern — defer to §6 (error-surface design). `AiAgentProviderDialog` `save` now has a `catch` (see §3.1).

### 3.6 Convoluted (large ones → §6.3)
- [x] `App.tsx:1261-1899` single return ~640 lines; `:1087-1243` `executeCommand`; `:558-690` 18-listener effect; `:691-774` `onPluginSettingsPatch`; `HostSessionSettingsDialog.tsx:348-~1090` `sections` useMemo ~740 lines; `OptionsDialog.tsx:276-~530`; `TerminalView.tsx:168-397`; `SessionsSidebar.tsx:300-510`; `PluginFieldEditor.tsx:107-190` `ItemListEditor`; `TunnelBuilder.tsx:220-401`; `SshKeySettingsGroup.tsx:61-220`. → §6.3 (routed)

## 4. Plugins

Per-plugin rubric (§14 compliance also at §5): standard quality findings; allowed imports only; `window.wassh` use is allowed for views; manifest/defaults/schema consistency; dead `protocol.ts` guards; `plugin-ui-*` CSS only; no cross-plugin imports.

### 4.1 scratchpad — audited: clean. Manifest matches `id.ts`; `protocol.ts` guard (`contentFromData`) used; main module is a thin `onApiCall` router; View scope/legacy-adoption logic correct; `plugin-scratchpad-input` is a plugin-unique selector in its own `styles.css` (allowed). No findings.
### 4.2 macro-pad — audited: clean. Manifest matches `id.ts`; `isMacroPadRendererMessage` guard used in `main.ts`; no dead code or literals. No findings.
### 4.3 connection-logger — audited: clean. Manifest matches `id.ts`; `isConnectionLoggerData` used in both `main.ts` and `View.tsx`; `connectionLoggerScopeId` shared; `CONNECTION_LOGGER_DATA_VERSION` named. No findings.
### 4.4 daemon-monitor — audited: clean. Manifest matches `id.ts`; `isDaemonMonitorRendererMessage` (main) and `isDaemonMonitorMainMessage` (View) both used; protocol types documented. No findings.
### 4.5 mqtt-analyser — audited: clean. Manifest matches `id.ts`; `isMqttAnalyserMainMessage` used in `View.tsx`; protocol payloads documented. No findings.
### 4.6 server-monitor — audited: clean. Manifest matches `id.ts`; `isServerMonitorProcessSort`/`isServerMonitorProcessSignal` used in `main.ts`; sort keys/defaults named. No findings.
### 4.7 sftp
- [x] Relative imports bypassing aliases → `@plugin-api/*`. Fixed: all 12 imports across `main.ts`, `helpers.ts`, `fileView.ts`, `transfers.ts`, `manifest.ts`, `fileDrop.ts`, `View.tsx` now use the aliases. (verified: no `../../../{shared,main,renderer}` under `src/plugins/**`)
- [x] `transfers.ts:10` imports Electron `{BrowserWindow, dialog}`. Fixed: neutral `ctx.showOpenDialog`/`ctx.showSaveDialog` (+ `PluginOpenDialogOptions`/`PluginSaveDialogOptions`/results) added to `PluginMainContext`, implemented in `PluginHost` via `getWindow`; `transfers.ts` uses them. No `from 'electron'` remains under `src/plugins/**`.
- [x] `fileDrop.ts:146-152` — per-file `catch` swallows errors (only cancel distinguished); surface failures. Resolved: the terminal-drop path was removed (§5); the panel path already reports failures into its transfers map, and `uploadFilesOverSftp`'s catch now only cancels.
- [x] `fileDrop.ts:29-31,64-76` — module-level singletons (`readyTabs`, `activeUploads`, `trackingStarted`) + listeners registered once, never removed. Resolved: all three were deleted with the terminal-drop feature (§5); `fileDrop.ts` now has no module state.
- [x] `fileDrop.ts:111` — `cancelSftpFileDrop` exported but only file-internal. Fixed: de-exported.
### 4.8 ai-agent
- [x] `View.tsx:198,202` — `isFileDrag`/`collectDroppedFiles` duplicated `sftp/fileDrop.ts`. Fixed: neutral helpers moved to `renderer/src/plugins/api/fileDrag.ts`, exported from `@plugin-api/renderer`; both plugins import them (local copies deleted). Documented in PLUGIN_API.md §12.
- [x] Audit pending otherwise (`main.ts` 2437 lines, `tools.ts` ≥1308, `View.tsx` ≥1170, plus apiMethods, defaults, id, manifest, permissions, pluginApiTools, protocol, providers, rag, terminal). — `id.ts`/`manifest.ts` checked: manifest uses `PLUGIN_ID_AI_AGENT` + named setting constants; no literal drift. Remaining large-file audit is a §6-scale task (routed).

## 5. Plugin isolation (strict — close now)

- [x] Normalize sftp imports (see 4.7) to `@plugin-api/*` aliases. Done.
- [x] Add file-dialog capability: neutral `showOpenDialog`/`showSaveDialog` on `PluginMainContext` (`main/plugins/api.ts`), implemented in `PluginHost` via `getWindow`; `sftp/transfers.ts` Electron usage replaced. Docs: PLUGIN_API.md §5 updated.
- [x] `terminalFileDrop` dead surface (`shared/pluginApi.ts:166,191`, `sftp/manifest.ts:14`) — never read anywhere (verified). Removed from the manifest type and sftp's manifest; the renderer `fileDrop` registration is the single source.
- [x] Break App↔sftp coupling. Fixed by removing the terminal-drop-to-upload feature (decision: the SFTP panel's own drop covers the need, and the terminal path was the only thing forcing core to know sftp). Deleted from `App.tsx`: sftp imports, `DropUploadState`, `formatBytes`, `sftpReadyRef`/`sftpReadyTick`/`dropUploads`, the sftp branches in `onSessionStatus`/`onPluginActive`, the whole `onPluginMessage` handler, `uploadDroppedFiles`, the drop view-model, and the `dropEnabled`/`dropUpload`/`onDropFiles` props. Deleted from `TerminalView.tsx`: the three drop props, drag handlers/listeners, and the overlay + progress-banner JSX. Deleted the now-dead `sftpFileDropHandler` + readiness tracking (`readyTabs`/`startTracking`/`handlePluginActive`/`handlePluginMessage`/`trackingStarted`/`activeUploads`/`cancelSftpFileDrop`) and the `onProgress` param; `fileDrop.ts` now holds only `uploadFilesOverSftp` (used by the panel). Removed the `fileDrop` registration from `builtinRegistry.tsx` and the `PluginFileDropHandler`/`PluginFileDropProgress`/`fileDrop?` API surface; orphaned `.terminal-drop-*`/`.drop-upload-*` CSS removed. (verified: zero sftp imports under `src/renderer/src/{App.tsx,components/**}`; `isFileDrag`/`collectDroppedFiles` retained for the panel + ai-agent)
- [ ] Break core→ai-agent coupling: move provider-config UI ownership into the plugin (own defaults/id/protocol); add a minimal generic settings hook so `OptionsDialog` renders plugin-provided settings UI without plugin imports; remove the `populateAiAgentSchema` and `configureProviders` special cases (`OptionsDialog.tsx:34-39,196-203,284,492-505,530-540`) and `AiAgentProviderDialog.tsx:1-12` imports; keep current UX (dialog openable from Options); document in PLUGIN_API.md §11/§12.
- [x] Barrel leak: `renderer/src/plugins/api/index.ts:38` re-exported `PluginSettingsFieldList` from outside `api/`. Fixed: `PluginSettingsFieldList.tsx` and `PluginFieldEditor.tsx` moved into `renderer/src/plugins/api/`; the barrel re-exports both from local paths; core consumers import from `plugins/api/`. PLUGIN_API.md path updated. (`plugin-ui-*` class check remains in §4.)
- [x] Renderer builtin registry uses literal ids — use each plugin's `id.ts`. Done (see §3.4).
- [x] Re-run the §Verification greps. (verified: no `../../../{shared,main,renderer}` and no `from 'electron'` under `src/plugins/**`; `src/main/**` imports `plugins/builtins/**` only in `main/plugins/builtinRegistry.ts` (the composition root); `src/renderer/src/{App.tsx,components/**}` has no sftp imports. Remaining: `src/renderer/src/components/{OptionsDialog,AiAgentProviderDialog}.tsx` still import ai-agent — that is the core→ai-agent decoupling below.)

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
- [x] PLUGIN_API.md settings-field table — now lists all 12 types (`textArea`, `directory`, `itemList`, `action`, `permission` added) plus `options`/`itemSchema`/`itemLabel`/`action`; prose covers `itemList`/`action`/`textArea`/`directory`. `PluginApiMethod.label`/`defaultPermission` documented in §7. `getPluginData(pluginId, scopeId?)`/`setPluginData(..., scopeId?)` corrected with a scopeId note. §12 UI-kit list verified complete against `plugin-ui.css` (all 41 selectors covered). `terminalFileDrop` was never documented and is now removed (§5).
- [x] `main/plugins/api.ts:13-14` re-exports (`classifySftpError`, `joinRemotePath`, `SftpSession`) — intent documented inline: plugins use the API entry instead of `./SftpSession` internals.
- [x] Update PLUGIN_API.md tables after the §5 additions. Done (file-dialog row in §5, `isFileDrag`/`collectDroppedFiles` in §12, path fix for `PluginSettingsFieldList`).
