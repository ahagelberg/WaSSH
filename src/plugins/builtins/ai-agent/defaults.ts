import type { PluginSettingsSelectOption } from '@plugin-api/shared'
import {
  AI_AGENT_PROTOCOL_ANTHROPIC,
  AI_AGENT_PROTOCOL_OPENAI,
  type AiAgentProviderConfig,
  type AiAgentWebSearchProvider
} from './protocol'

export const AI_AGENT_SEARCH_PROVIDER_OPTIONS: PluginSettingsSelectOption[] = [
  { value: 'bing', label: 'Bing (Default)' },
  { value: 'brave', label: 'Brave Search' },
  { value: 'google', label: 'Google Search' },
  { value: 'duckduckgo', label: 'DuckDuckGo' },
  { value: 'custom', label: 'Custom Endpoint' }
]

export const AI_AGENT_DEFAULT_WEB_SEARCH_PROVIDER: AiAgentWebSearchProvider = 'bing'

export const AI_AGENT_OLLAMA_PROVIDER_ID = 'ollama'
export const AI_AGENT_OPENROUTER_PROVIDER_ID = 'openrouter'
export const AI_AGENT_ANTHROPIC_PROVIDER_ID = 'anthropic'
export const AI_AGENT_DEEPSEEK_PROVIDER_ID = 'deepseek'
export const AI_AGENT_GROK_PROVIDER_ID = 'grok'

export const AI_AGENT_OLLAMA_BASE_URL = 'http://127.0.0.1:11434/v1'
export const AI_AGENT_OPENROUTER_BASE_URL = 'https://openrouter.ai/api/v1'
export const AI_AGENT_ANTHROPIC_BASE_URL = 'https://api.anthropic.com'
export const AI_AGENT_DEEPSEEK_BASE_URL = 'https://api.deepseek.com'
export const AI_AGENT_GROK_BASE_URL = 'https://api.x.ai/v1'

export const AI_AGENT_OLLAMA_DEFAULT_MODELS: string[] = []
export const AI_AGENT_OPENROUTER_DEFAULT_MODELS = ['anthropic/claude-sonnet-4-20250514']
export const AI_AGENT_ANTHROPIC_DEFAULT_MODELS = [
  'claude-sonnet-4-20250514',
  'claude-opus-4-20250514',
  'claude-3-5-haiku-20241022'
]
export const AI_AGENT_DEEPSEEK_DEFAULT_MODELS = ['deepseek-chat', 'deepseek-reasoner']
export const AI_AGENT_GROK_DEFAULT_MODELS = ['grok-3', 'grok-3-mini-beta']

export const AI_AGENT_DEFAULT_PROVIDERS: AiAgentProviderConfig[] = [
  {
    id: AI_AGENT_OLLAMA_PROVIDER_ID,
    name: 'Ollama',
    protocol: AI_AGENT_PROTOCOL_OPENAI,
    baseUrl: AI_AGENT_OLLAMA_BASE_URL,
    models: [...AI_AGENT_OLLAMA_DEFAULT_MODELS]
  },
  {
    id: AI_AGENT_OPENROUTER_PROVIDER_ID,
    name: 'Open Router',
    protocol: AI_AGENT_PROTOCOL_OPENAI,
    baseUrl: AI_AGENT_OPENROUTER_BASE_URL,
    models: [...AI_AGENT_OPENROUTER_DEFAULT_MODELS]
  },
  {
    id: AI_AGENT_ANTHROPIC_PROVIDER_ID,
    name: 'Anthropic',
    protocol: AI_AGENT_PROTOCOL_ANTHROPIC,
    baseUrl: AI_AGENT_ANTHROPIC_BASE_URL,
    models: [...AI_AGENT_ANTHROPIC_DEFAULT_MODELS]
  },
  {
    id: AI_AGENT_DEEPSEEK_PROVIDER_ID,
    name: 'DeepSeek',
    protocol: AI_AGENT_PROTOCOL_OPENAI,
    baseUrl: AI_AGENT_DEEPSEEK_BASE_URL,
    models: [...AI_AGENT_DEEPSEEK_DEFAULT_MODELS]
  },
  {
    id: AI_AGENT_GROK_PROVIDER_ID,
    name: 'Grok (xAI)',
    protocol: AI_AGENT_PROTOCOL_OPENAI,
    baseUrl: AI_AGENT_GROK_BASE_URL,
    models: [...AI_AGENT_GROK_DEFAULT_MODELS]
  }
]

export const AI_AGENT_SAFE_RULES: string[] = [
  'pwd',
  'whoami',
  'id*',
  'hostname*',
  'uname*',
  'date',
  'uptime',
  'ls*',
  'lscpu*',
  'free*',
  'df*',
  'ps*',
  'git status*',
  'git log*',
  'git diff*',
  'git branch*',
  'git remote*',
  'git show*',
  'git rev-parse*'
]

export const AI_AGENT_DATA_VERSION = 2
export const AI_AGENT_DEFAULT_CHAT_TITLE = 'New chat'
export const AI_AGENT_CONVERSATIONS_PER_HOST_MAX = 40
export const AI_AGENT_TITLE_MAX_CHARS = 48

export const AI_AGENT_MAX_ATTACHMENTS = 8
export const AI_AGENT_MAX_ATTACHMENT_BYTES = 200_000
export const AI_AGENT_MAX_ATTACHMENT_CHARS = 100_000
