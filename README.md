# WaSSH

Native multi-tab SSH client for Windows (Electron + React + xterm.js).

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

## Package

```bash
npm run dist
```

Artifacts in `release/`:
- `WaSSH Setup 0.1.0.exe` (NSIS)
- `WaSSH 0.1.0.exe` (portable)
- `win-unpacked/WaSSH.exe`
