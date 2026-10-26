/**
 * Copy the Electron app's configuration into the native (Rust) app's data directory.
 *
 * The legacy files are only read. Transformed documents are written to
 * `%APPDATA%\wassh-native`, existing files there are kept as `<name>.bak`.
 *
 * Host passwords and key passphrases are decrypted from the legacy credential vault
 * (Chromium `os_crypt`: AES-256-GCM under a DPAPI-wrapped key) and re-sealed into the
 * native `secrets.json` with the native `secret.key`.
 *
 * Usage: node scripts/migrate-legacy-config.js [--dry-run]
 */

const crypto = require('node:crypto')
const fs = require('node:fs')
const path = require('node:path')
const { execFileSync } = require('node:child_process')

const LEGACY_DIR =
  process.env.WASSH_LEGACY_DIR || path.join(process.env.APPDATA || '', 'WaSSH')
const NATIVE_DIR =
  process.env.WASSH_NATIVE_DIR || path.join(process.env.APPDATA || '', 'wassh-native')

/** Legacy serial stop bits are numbers; the native schema spells them out. */
const STOP_BITS = { 1: '1', 1.5: '1.5', 2: '2' }
/** Legacy flow-control values are lower case; the native enum is camelCase. */
const FLOW_CONTROL = { none: 'none', rtscts: 'rtsCts', xonxoff: 'xonXoff' }

const SECRET_NONCE_LEN = 12
const SECRET_KEY_LEN = 32
const CHROMIUM_PREFIX = 'v10'
const DPAPI_PREFIX_LEN = 5

const dryRun = process.argv.includes('--dry-run')

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'))
}

function writeJson(name, value) {
  const target = path.join(NATIVE_DIR, name)
  const body = `${JSON.stringify(value, null, 2)}\n`
  if (dryRun) {
    console.log(`  would write ${target} (${body.length} bytes)`)
    return
  }
  fs.mkdirSync(NATIVE_DIR, { recursive: true })
  if (fs.existsSync(target)) {
    fs.copyFileSync(target, `${target}.bak`)
  }
  fs.writeFileSync(target, body)
  console.log(`  wrote ${target}`)
}

function emptyToNull(value) {
  return typeof value === 'string' && value.trim() !== '' ? value : null
}

function pluginSettings(value) {
  const result = {}
  for (const [id, settings] of Object.entries(value || {})) {
    if (settings && typeof settings === 'object' && !Array.isArray(settings)) {
      result[id] = settings
    }
  }
  return result
}

function tunnel(legacy) {
  return {
    id: legacy.id || '',
    enabled: legacy.enabled !== false,
    kind: legacy.type || 'local',
    listenHost: legacy.listenHost || '',
    listenPort: Number(legacy.listenPort) || 0,
    destHost: legacy.destHost || '',
    destPort: Number(legacy.destPort) || 0
  }
}

/** Legacy stores connection fields flat; the native schema nests them. */
function connection(flat) {
  return {
    type: flat.connectionType || 'ssh',
    host: flat.host || '',
    port: Number(flat.port) || 0,
    username: flat.username || '',
    auth: flat.authMethod || 'password',
    privateKeyPath: emptyToNull(flat.privateKeyPath),
    jumpHostId: emptyToNull(flat.proxyHostId),
    tunnels: (flat.tunnels || []).map(tunnel),
    x11: flat.x11Forwarding === true,
    remoteSession: {
      enabled: flat.openInScreen === true,
      multiplexer: flat.remoteSessionKind || 'screen',
      name: flat.screenSessionName || 'WaSSH',
      busy: flat.screenBusyHandling || 'doNotAttach'
    },
    serial: {
      path: flat.connectionType === 'serial' ? flat.host || '' : '',
      baud: Number(flat.serialBaudRate) || 115200,
      dataBits: Number(flat.serialDataBits) || 8,
      parity: flat.serialParity || 'none',
      stopBits: STOP_BITS[flat.serialStopBits] || '1',
      flow: FLOW_CONTROL[flat.serialFlowControl] || 'none'
    },
    reconnect: flat.reconnectMode || 'onFocus',
    style: {
      tabColor: flat.tabColor || '',
      termBackground: flat.termBackground || '',
      termForeground: flat.termForeground || '',
      fontSize: flat.fontSizePx ?? null,
      fontFamily: emptyToNull(flat.fontFamily),
      bell: flat.bellMode ?? null,
      cursor: flat.cursorStyle ?? null,
      cursorBlink: flat.cursorBlink ?? null,
      scrollback: flat.scrollbackLines ?? null
    }
  }
}

function migrateSettings(legacy) {
  const style = legacy.sessionStyleDefaults || {}
  return {
    theme: legacy.theme || 'system',
    termType: legacy.termType || 'xterm-256color',
    reconnectOnStartup: legacy.reconnectOnStartup !== false,
    sidebarCollapsed: legacy.sidebarCollapsed === true,
    enabledPlugins: legacy.enabledPlugins || [],
    pluginSettings: pluginSettings(legacy.pluginSettings),
    tunnelTemplates: (legacy.tunnelTemplates || []).map((template) => ({
      id: template.id,
      name: template.name,
      tunnel: tunnel(template.tunnel || {})
    })),
    sessionStyleDefaults: {
      tabColor: style.tabColor || '',
      termBackground: style.termBackground || '',
      termForeground: style.termForeground || '',
      fontSize: style.fontSizePx ?? 14,
      fontFamily: style.fontFamily || 'JetBrains Mono',
      bell: style.bellMode || 'system',
      cursor: style.cursorStyle || 'block',
      cursorBlink: style.cursorBlink !== false,
      scrollback: style.scrollbackLines ?? 10000
    },
    windowBounds: legacy.windowBounds ?? null
  }
}

/** Legacy groups list their members; the native schema stores a host's group id. */
function migrateHosts(legacy) {
  const groupOf = new Map()
  for (const group of legacy.groups || []) {
    for (const hostId of group.hostIds || []) {
      groupOf.set(hostId, group.id)
    }
  }
  return {
    groups: (legacy.groups || []).map((group) => ({
      id: group.id,
      name: group.name,
      color: group.color || '',
      collapsed: group.collapsed === true
    })),
    ungroupedHostIds: legacy.ungroupedHostIds || [],
    ungroupedCollapsed: legacy.ungroupedCollapsed === true,
    hosts: (legacy.hosts || []).map((host) => ({
      id: host.id,
      name: host.name || '',
      tags: host.tags || [],
      groupId: groupOf.get(host.id) || null,
      connection: connection(host),
      pluginSettings: pluginSettings(host.pluginSettings)
    }))
  }
}

function migrateTabs(legacy) {
  return (legacy || []).map((tab) => ({
    id: tab.id,
    hostId: tab.connection?.hostId || null,
    connection: connection(tab.connection || {}),
    active: tab.active === true,
    activePluginIds: tab.activePluginIds || [],
    pluginLayout: tab.pluginLayout || null
  }))
}

function migrateKnownHosts(legacy) {
  return (legacy || []).map((entry) => ({
    host: entry.host,
    port: entry.port,
    fingerprint: entry.fingerprint
  }))
}

/** Chromium wraps the AES key of `v10` blobs in DPAPI; only PowerShell can unwrap it. */
function chromiumKey(localStateFile) {
  const script = [
    "$ProgressPreference = 'SilentlyContinue'",
    'Add-Type -AssemblyName System.Security',
    `$state = Get-Content -LiteralPath '${localStateFile}' -Raw | ConvertFrom-Json`,
    '$blob = [Convert]::FromBase64String($state.os_crypt.encrypted_key)',
    '$wrapped = [byte[]]($blob[' + DPAPI_PREFIX_LEN + '..($blob.Length-1)])',
    '$key = [System.Security.Cryptography.ProtectedData]::Unprotect(' +
      '$wrapped, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser)',
    "[BitConverter]::ToString($key).Replace('-','')"
  ].join('; ')
  const encoded = Buffer.from(script, 'utf16le').toString('base64')
  const output = execFileSync(
    'powershell',
    ['-NoProfile', '-NonInteractive', '-EncodedCommand', encoded],
    { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }
  )
  const hex = /[0-9a-f]{64}/i.exec(output)
  if (!hex) {
    throw new Error('could not read the legacy vault key (is the vault from another machine?)')
  }
  return Buffer.from(hex[0], 'hex')
}

function decryptLegacy(sealed, key) {
  const blob = Buffer.from(sealed, 'base64')
  if (blob.subarray(0, CHROMIUM_PREFIX.length).toString() !== CHROMIUM_PREFIX) {
    return null
  }
  const nonce = blob.subarray(CHROMIUM_PREFIX.length, CHROMIUM_PREFIX.length + SECRET_NONCE_LEN)
  const tag = blob.subarray(blob.length - 16)
  const body = blob.subarray(CHROMIUM_PREFIX.length + SECRET_NONCE_LEN, blob.length - 16)
  try {
    const decipher = crypto.createDecipheriv('aes-256-gcm', key, nonce)
    decipher.setAuthTag(tag)
    return Buffer.concat([decipher.update(body), decipher.final()]).toString('utf8')
  } catch {
    return null
  }
}

function seal(plain, key) {
  const nonce = crypto.randomBytes(SECRET_NONCE_LEN)
  const cipher = crypto.createCipheriv('aes-256-gcm', key, nonce)
  const body = Buffer.concat([cipher.update(plain, 'utf8'), cipher.final()])
  return Buffer.concat([nonce, body, cipher.getAuthTag()]).toString('base64')
}

/** Native `secrets.json`: one sealed entry per host id. */
function migrateSecrets(vault, hosts, tabs) {
  const keyFile = path.join(NATIVE_DIR, 'secret.key')
  if (!fs.existsSync(keyFile)) {
    throw new Error(`missing ${keyFile}; run the native app once first`)
  }
  const key = fs.readFileSync(keyFile)
  if (key.length !== SECRET_KEY_LEN) {
    throw new Error(`${keyFile} is not ${SECRET_KEY_LEN} bytes`)
  }

  const legacyKey = chromiumKey(path.join(LEGACY_DIR, 'Local State'))
  const plain = new Map()
  for (const [id, sealed] of Object.entries(vault)) {
    const value = decryptLegacy(sealed, legacyKey)
    if (value !== null && value !== '') {
      plain.set(id, value)
    }
  }
  const undecrypted = Object.keys(vault).length - plain.size
  if (undecrypted > 0) {
    console.warn(`  ${undecrypted} vault entr(y/ies) could not be decrypted; skipped`)
  }

  const entries = {}
  const remember = (hostId, field, vaultId) => {
    const value = plain.get(vaultId)
    if (!hostId || !value) {
      return
    }
    entries[hostId] = entries[hostId] || {}
    entries[hostId][field] = seal(value, key)
  }

  for (const host of hosts) {
    remember(host.id, 'password', host.passwordVaultId || `pwd-${host.id}`)
    if (host.passphraseVaultId) {
      remember(host.id, 'passphrase', host.passphraseVaultId)
    }
  }
  for (const tab of tabs) {
    if (tab.connection.hostId && tab.connection.passwordVaultId) {
      remember(tab.connection.hostId, 'password', tab.connection.passwordVaultId)
    }
  }

  return { version: 2, entries, pluginSecrets: {} }
}

function main() {
  console.log(`legacy: ${LEGACY_DIR}`)
  console.log(`native: ${NATIVE_DIR}`)
  if (!fs.existsSync(LEGACY_DIR)) {
    throw new Error(`no legacy configuration in ${LEGACY_DIR}`)
  }

  const settings = readJson(path.join(LEGACY_DIR, 'settings.json'))
  const hosts = readJson(path.join(LEGACY_DIR, 'hosts.json'))
  const tabs = readJson(path.join(LEGACY_DIR, 'tabs.json'))
  const knownHostsFile = path.join(LEGACY_DIR, 'known_hosts.json')
  const knownHosts = fs.existsSync(knownHostsFile) ? readJson(knownHostsFile) : []

  writeJson('settings.json', migrateSettings(settings))
  writeJson('hosts.json', migrateHosts(hosts))
  writeJson('tabs.json', migrateTabs(tabs))
  writeJson('known_hosts.json', migrateKnownHosts(knownHosts))

  const vaultFile = path.join(LEGACY_DIR, 'vault.json')
  if (fs.existsSync(vaultFile)) {
    const vault = readJson(vaultFile)
    const secrets = migrateSecrets(vault, hosts.hosts || [], tabs)
    writeJson('secrets.json', secrets)
    console.log(`  ${Object.keys(secrets.entries).length} host credential(s)`)
  } else {
    console.log('  no legacy vault; secrets skipped')
  }

  const migrated = migrateHosts(hosts)
  console.log(
    `hosts: ${migrated.hosts.length} in ${migrated.groups.length} group(s), ` +
      `tabs: ${tabs.length}, known hosts: ${knownHosts.length}`
  )
}

main()
