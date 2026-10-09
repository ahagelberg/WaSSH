const { spawn } = require('node:child_process')

const electronViteArgs = ['dev']
const env = { ...process.env }
if (process.platform === 'linux') {
  for (const key of [
    'GTK_PATH',
    'GTK_EXE_PREFIX',
    'GTK_IM_MODULE_FILE',
    'GSETTINGS_SCHEMA_DIR',
    'GIO_MODULE_DIR'
  ]) {
    delete env[key]
  }
  if (process.env.WAYLAND_DISPLAY) {
    electronViteArgs.push('--', '--ozone-platform=x11')
  }
}

const isSnapConfined =
  process.platform === 'linux' &&
  (Boolean(process.env.SNAP) ||
    process.env.GTK_PATH?.startsWith('/snap/') ||
    process.env.GTK_EXE_PREFIX?.startsWith('/snap/'))

let command = 'electron-vite'
let args = electronViteArgs
let childEnv = env
let shell = process.platform === 'win32'

if (isSnapConfined) {
  const sessionVariables = [
    'HOME',
    'USER',
    'LOGNAME',
    'LANG',
    'DISPLAY',
    'WAYLAND_DISPLAY',
    'XDG_RUNTIME_DIR',
    'DBUS_SESSION_BUS_ADDRESS',
    'XDG_CURRENT_DESKTOP',
    'XDG_SESSION_TYPE'
  ]
  const systemdArgs = [
    '--user',
    '--wait',
    '--pipe',
    '--collect',
    `--unit=wassh-dev-${process.pid}`,
    `--property=WorkingDirectory=${process.cwd()}`
  ]

  for (const key of sessionVariables) {
    if (env[key]) {
      systemdArgs.push(`--setenv=${key}=${env[key]}`)
    }
  }

  const hostPath = [
    process.env.PATH?.split(':').filter((part) => !part.startsWith('/snap/')).join(':'),
    `${process.cwd()}/node_modules/.bin`,
    '/usr/bin',
    '/bin'
  ]
    .filter(Boolean)
    .join(':')
  systemdArgs.push(`--setenv=PATH=${hostPath}`, '--', 'electron-vite', ...electronViteArgs)
  command = 'systemd-run'
  args = systemdArgs
  childEnv = process.env
  shell = false
}

const child = spawn(command, args, { env: childEnv, stdio: 'inherit', shell })

child.on('error', (error) => {
  console.error(`Failed to start electron-vite: ${error.message}`)
  process.exitCode = 1
})

child.on('exit', (code) => {
  process.exitCode = code ?? 1
})