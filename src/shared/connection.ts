import type { ConnectionParams, HostProfile } from './types'

export function hostDisplayName(host: Pick<HostProfile, 'name' | 'host'>): string {
  const trimmed = (host.name ?? '').trim()
  if (trimmed) {
    return trimmed
  }
  return host.host ?? ''
}

export function hostToConnection(host: HostProfile): ConnectionParams {
  return {
    hostId: host.id,
    name: host.name,
    host: host.host,
    port: host.port,
    username: host.username,
    passwordVaultId: host.passwordVaultId,
    privateKeyPath: host.privateKeyPath,
    passphraseVaultId: host.passphraseVaultId,
    authMethod: host.authMethod,
    proxyHostId: host.proxyHostId || '',
    ephemeralPassword: '',
    ephemeralPassphrase: ''
  }
}

/** Outermost proxy first, target last */
export function resolveProxyChain(
  target: ConnectionParams,
  hosts: HostProfile[]
): ConnectionParams[] {
  const byId = new Map(hosts.map((h) => [h.id, h]))
  const hops: ConnectionParams[] = []
  const seen = new Set<string>()
  let proxyId = target.proxyHostId
  while (proxyId) {
    if (seen.has(proxyId)) {
      throw new Error('Circular proxy host reference')
    }
    seen.add(proxyId)
    const profile = byId.get(proxyId)
    if (!profile) {
      throw new Error(`Proxy host not found (${proxyId})`)
    }
    hops.unshift(hostToConnection(profile))
    proxyId = profile.proxyHostId || ''
  }
  return [...hops, target]
}

export function proxyLabel(chain: ConnectionParams[]): string | null {
  if (chain.length < 2) {
    return null
  }
  return chain
    .slice(0, -1)
    .map((h) => h.name || h.host)
    .join(' → ')
}
