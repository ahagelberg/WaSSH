import type { PluginManifest } from '@plugin-api/shared'
import {
  AI_AGENT_SAFE_RULES,
  AI_AGENT_SEARCH_PROVIDER_OPTIONS,
  AI_AGENT_DEFAULT_WEB_SEARCH_PROVIDER
} from './defaults'
import { PLUGIN_ID_AI_AGENT } from './id'
import { AI_AGENT_API_METHODS, buildAiAgentBuiltinApiFields } from './apiMethods'
import {
  AI_AGENT_SETTING_DEFAULT_ALLOW_RULES,
  AI_AGENT_SETTING_DEFAULT_DENY_RULES,
  AI_AGENT_SETTING_GLOBAL_PROMPT,
  AI_AGENT_SETTING_HOST_ALLOW_RULES,
  AI_AGENT_SETTING_HOST_DENY_RULES,
  AI_AGENT_SETTING_HOST_PROMPT,
  AI_AGENT_SETTING_PROMPT_FILES_FOLDER,
  AI_AGENT_SETTING_WEB_SEARCH_API_KEY,
  AI_AGENT_SETTING_WEB_SEARCH_PROVIDER
} from './protocol'

const WEB_SEARCH_EXTRA_CHILDREN = [
  {
    key: AI_AGENT_SETTING_WEB_SEARCH_PROVIDER,
    label: 'Web search provider',
    type: 'select' as const,
    default: AI_AGENT_DEFAULT_WEB_SEARCH_PROVIDER,
    options: AI_AGENT_SEARCH_PROVIDER_OPTIONS,
    description: 'Search engine used for web searches.'
  },
  {
    key: AI_AGENT_SETTING_WEB_SEARCH_API_KEY,
    label: 'Web search API key',
    type: 'string' as const,
    default: '',
    secret: true,
    description: 'Optional API key for search engine provider (Brave, Google, or Bing API).'
  }
]

export const aiAgentManifest: PluginManifest = {
  id: PLUGIN_ID_AI_AGENT,
  name: 'AI agent',
  version: '1.0.0',
  description:
    'Chat with an LLM that can run commands on the remote host. Commands outside the safe/allow lists ask for approval.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'AI' },
    settingsHeading: 'AI agent',
    settingsSchema: [
      {
        key: 'configureProviders',
        label: 'Configure providers',
        type: 'action',
        default: null,
        action: 'configureProviders',
        description: 'Add, remove, and configure AI model providers.'
      },
      {
        key: AI_AGENT_SETTING_PROMPT_FILES_FOLDER,
        label: 'Prompt files folder',
        type: 'directory',
        default: '',
        description:
          'Local folder whose top-level YAML and text files are included in every AI agent prompt.'
      },
      {
        key: AI_AGENT_SETTING_GLOBAL_PROMPT,
        label: 'Global prompt text',
        type: 'textArea',
        default: '',
        description: 'Text included in every AI agent prompt.'
      },
      {
        key: AI_AGENT_SETTING_DEFAULT_ALLOW_RULES,
        label: 'Default allow rules',
        type: 'stringList',
        default: AI_AGENT_SAFE_RULES,
        description:
          'App-wide command patterns that run without asking (applies to every host). One per line; glob by default, prefix a line with "regex:" for a regular expression. Host allow rules are checked first.'
      },
      {
        key: AI_AGENT_SETTING_DEFAULT_DENY_RULES,
        label: 'Default deny rules',
        type: 'stringList',
        default: [],
        description:
          'App-wide command patterns that are always blocked. Deny always wins over allow.'
      },
      ...buildAiAgentBuiltinApiFields(WEB_SEARCH_EXTRA_CHILDREN)
    ],
    hostSettingsHeading: 'AI agent',
    hostSettingsSchema: [
      {
        key: AI_AGENT_SETTING_HOST_PROMPT,
        label: 'Host prompt text',
        type: 'textArea',
        default: '',
        description: 'Text included in AI agent prompts for this host or session.'
      },
      {
        key: AI_AGENT_SETTING_HOST_ALLOW_RULES,
        label: 'Allow rules',
        type: 'stringList',
        default: [],
        description:
          'Command patterns for this host that run without asking. One per line; glob by default, prefix with "regex:" for a regular expression.'
      },
      {
        key: AI_AGENT_SETTING_HOST_DENY_RULES,
        label: 'Deny rules',
        type: 'stringList',
        default: [],
        description:
          'Command patterns for this host that are always blocked. Deny always wins over allow.'
      },
      ...buildAiAgentBuiltinApiFields(WEB_SEARCH_EXTRA_CHILDREN)
    ],
    views: [{ id: 'panel', placement: 'split-right', title: 'AI agent' }],
    api: { methods: AI_AGENT_API_METHODS }
  }
}

