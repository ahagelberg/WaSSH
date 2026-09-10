export const PLUGIN_ID_AI_AGENT = 'ai-agent'

export const AI_AGENT_KEY_VAULT_PREFIX = `${PLUGIN_ID_AI_AGENT}:`

export function aiAgentVaultId(providerId: string): string {
  return `${AI_AGENT_KEY_VAULT_PREFIX}${providerId}`
}
