# WaSSH

Advanced Multi-tab SSH client for Windows (Electron + React + xterm.js).

## Develop

```bash
npm install
npm run setup-native   # if `npm run dev` says "Electron uninstall"
npm run dev
```

If npm blocked install scripts (`install-scripts not yet covered by allowScripts`):

```bash
npm install-scripts approve electron ssh2
npm run setup-native
```

## Package locally

```bash
npm run dist
```

Artifacts in `release/`:
- `WaSSH-Setup-<version>.exe` (NSIS installer)
- `WaSSH-Portable-<version>.exe` (portable, no installation)
- `win-unpacked/WaSSH.exe`

## Releases & auto-update

Pushing a `v*` tag runs `.github/workflows/release.yml`: it validates the tag against
`package.json` `version`, builds, and publishes installers + update metadata to a GitHub
Release. The version string has a single source of truth — `package.json` (shown in the
About dialog, the installer filename and Windows file properties).

```bash
npm version patch      # bumps version, creates the vX.Y.Z tag
git push --follow-tags # workflow builds and publishes
```

Installed NSIS builds check GitHub Releases for updates shortly after startup and via
**Help → Check for Updates…**. Portable, `win-unpacked` and dev builds do not auto-update.

Builds are unsigned: Windows SmartScreen shows "Windows protected your PC" on first run —
choose **More info → Run anyway**. Downloaded updates are still checksum-verified
(SHA-512 from `latest.yml`) before they are installed.

## Data & credentials

All configuration lives in the per-user data dir `%APPDATA%\WaSSH`
(`hosts.json`, `settings.json`, `tabs.json`, `known_hosts.json`, `vault.json`, `plugin-*.json`).

SSH passwords, key passphrases and API keys are never stored in plain text: they are
encrypted with the OS credential store (Windows DPAPI via Electron `safeStorage`) in
`vault.json`, and the other files only reference vault IDs. Nothing sensitive is written
to the installation folder, and uninstalling the app leaves `%APPDATA%\WaSSH` intact.

