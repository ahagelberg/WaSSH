const { spawnSync } = require('child_process')
const { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } = require('fs')
const { homedir, platform } = require('os')
const { join, resolve } = require('path')

/** Project root */
const ROOT = resolve(__dirname, '..')
/** Electron package dir */
const ELECTRON_DIR = join(ROOT, 'node_modules', 'electron')
/** ssh2 package dir */
const SSH2_DIR = join(ROOT, 'node_modules', 'ssh2')
/** Windows electron exe name */
const ELECTRON_EXE = 'electron.exe'

function electronReady() {
  const pathFile = join(ELECTRON_DIR, 'path.txt')
  if (!existsSync(pathFile)) {
    return false
  }
  const rel = readFileSync(pathFile, 'utf8').trim()
  return existsSync(join(ELECTRON_DIR, 'dist', rel))
}

function runNodeScript(scriptPath) {
  if (!existsSync(scriptPath)) {
    return true
  }
  const result = spawnSync(process.execPath, [scriptPath], {
    cwd: require('path').dirname(scriptPath),
    stdio: 'inherit',
    env: process.env
  })
  return result.status === 0
}

function extractElectronZipWindows() {
  if (platform() !== 'win32') {
    return false
  }
  const version = require(join(ELECTRON_DIR, 'package.json')).version
  const cacheRoot = join(homedir(), 'AppData', 'Local', 'electron', 'Cache')
  if (!existsSync(cacheRoot)) {
    return false
  }
  let zipPath = ''
  const walk = (dir) => {
    if (zipPath) {
      return
    }
    for (const name of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, name.name)
      if (name.isDirectory()) {
        walk(full)
      } else if (name.name === `electron-v${version}-win32-x64.zip`) {
        zipPath = full
      }
    }
  }
  walk(cacheRoot)
  if (!zipPath) {
    return false
  }
  const dist = join(ELECTRON_DIR, 'dist')
  rmSync(dist, { recursive: true, force: true })
  mkdirSync(dist, { recursive: true })
  const ps = spawnSync(
    'powershell',
    [
      '-NoProfile',
      '-Command',
      `Expand-Archive -Path '${zipPath.replace(/'/g, "''")}' -DestinationPath '${dist.replace(/'/g, "''")}' -Force`
    ],
    { stdio: 'inherit' }
  )
  if (ps.status !== 0) {
    return false
  }
  writeFileSync(join(ELECTRON_DIR, 'path.txt'), ELECTRON_EXE)
  return electronReady()
}

function ensureElectron() {
  if (electronReady()) {
    return
  }
  if (!existsSync(ELECTRON_DIR)) {
    return
  }
  runNodeScript(join(ELECTRON_DIR, 'install.js'))
  if (electronReady()) {
    return
  }
  if (extractElectronZipWindows()) {
    return
  }
  console.error(
    'Electron binary missing. Run: npm run setup-native\n' +
      'If install scripts were blocked: npm install-scripts approve electron && npm rebuild electron'
  )
  process.exit(1)
}

function ensureSsh2() {
  if (!existsSync(SSH2_DIR)) {
    return
  }
  runNodeScript(join(SSH2_DIR, 'install.js'))
}

ensureElectron()
ensureSsh2()
