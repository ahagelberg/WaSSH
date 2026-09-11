# WaSSH

Advanced Multi-tab SSH client for Windows and macOS (Electron + React + xterm.js).

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
npm run dist        # packages for current host OS
npm run dist:win    # packages Windows NSIS + portable
npm run dist:mac    # packages macOS DMG + ZIP
```

Artifacts in `release/`:
- Windows: `WaSSH-Setup-<version>.exe` (NSIS) and `WaSSH-Portable-<version>.exe`
- macOS: `WaSSH-<version>-mac-arm64.dmg` / `.zip` (Apple Silicon) and `WaSSH-<version>-mac-x64.dmg` / `.zip` (Intel)

## Releases & auto-update

Pushing a `v*` tag runs `.github/workflows/release.yml`: it validates the tag against
`package.json` `version`, builds Windows and macOS artifacts, and publishes installers to a GitHub
Release. The version string has a single source of truth — `package.json` (shown in the
About dialog, installer filenames, and executable properties).

```bash
npm version patch      # bumps version, creates the vX.Y.Z tag
git push --follow-tags # workflow builds and publishes
```

Installed Windows NSIS builds check GitHub Releases for updates shortly after startup and via
**Help → Check for Updates…**. Portable, dev, and macOS builds currently do not auto-update.

Builds are unsigned:
- **Windows**: SmartScreen shows "Windows protected your PC" on first run — choose **More info → Run anyway**.
- **macOS**: Gatekeeper blocks unsigned apps on first launch — open **System Settings → Privacy & Security** and choose **Open Anyway** (or right-click `WaSSH.app` in `/Applications` and select **Open**).

## Data & credentials

All configuration lives in the per-user data directory:
- **Windows**: `%APPDATA%\WaSSH`
- **macOS**: `~/Library/Application Support/WaSSH`

Files stored: `hosts.json`, `settings.json`, `tabs.json`, `known_hosts.json`, `vault.json`, `plugin-*.json`.

SSH passwords, key passphrases, and API keys are encrypted with the OS credential store (Windows DPAPI / macOS Keychain via Electron `safeStorage`) in `vault.json`, and the other files only reference vault IDs. Nothing sensitive is written to the installation folder, and uninstalling the app leaves configuration intact.

