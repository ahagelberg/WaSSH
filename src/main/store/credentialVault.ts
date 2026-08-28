import { app, safeStorage } from 'electron'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs'
import { join } from 'path'

const VAULT_FILE = 'vault.json'
const JSON_INDENT = 2

type VaultFile = Record<string, string>

export class CredentialVault {
  private path: string
  private cache: VaultFile = {}

  constructor() {
    const dir = app.getPath('userData')
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true })
    }
    this.path = join(dir, VAULT_FILE)
    this.load()
  }

  private load(): void {
    if (!existsSync(this.path)) {
      this.cache = {}
      return
    }
    try {
      this.cache = JSON.parse(readFileSync(this.path, 'utf8')) as VaultFile
    } catch {
      this.cache = {}
    }
  }

  private persist(): void {
    writeFileSync(this.path, JSON.stringify(this.cache, null, JSON_INDENT), 'utf8')
  }

  set(vaultId: string, plaintext: string): void {
    if (!safeStorage.isEncryptionAvailable()) {
      this.cache[vaultId] = Buffer.from(plaintext, 'utf8').toString('base64')
      this.persist()
      return
    }
    const encrypted = safeStorage.encryptString(plaintext)
    this.cache[vaultId] = encrypted.toString('base64')
    this.persist()
  }

  get(vaultId: string): string | null {
    const stored = this.cache[vaultId]
    if (!stored) {
      return null
    }
    const buf = Buffer.from(stored, 'base64')
    if (!safeStorage.isEncryptionAvailable()) {
      return buf.toString('utf8')
    }
    try {
      return safeStorage.decryptString(buf)
    } catch {
      return null
    }
  }

  delete(vaultId: string): void {
    delete this.cache[vaultId]
    this.persist()
  }
}
