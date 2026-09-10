import type { PluginManifest } from '@plugin-api/shared'
import {
  AI_AGENT_DEFAULT_DATETIME_ACCESS,
  AI_AGENT_DEFAULT_LOCAL_FS_ACCESS,
  AI_AGENT_DEFAULT_REMOTE_FS_ACCESS,
  AI_AGENT_DEFAULT_WEB_ACCESS,
  AI_AGENT_DEFAULT_WEB_SEARCH,
  AI_AGENT_DEFAULT_WEB_SEARCH_PROVIDER,
  AI_AGENT_SAFE_RULES,
  AI_AGENT_SEARCH_PROVIDER_OPTIONS
} from './defaults'
import { PLUGIN_ID_AI_AGENT } from './id'
import {
  AI_AGENT_SETTING_DEFAULT_ALLOW_RULES,
  AI_AGENT_SETTING_DEFAULT_DENY_RULES,
  AI_AGENT_SETTING_ENABLE_DATETIME_ACCESS,
  AI_AGENT_SETTING_ENABLE_LOCAL_FS_ACCESS,
  AI_AGENT_SETTING_ENABLE_REMOTE_FS_ACCESS,
  AI_AGENT_SETTING_ENABLE_WEB_ACCESS,
  AI_AGENT_SETTING_ENABLE_WEB_SEARCH,
  AI_AGENT_SETTING_HOST_ALLOW_RULES,
  AI_AGENT_SETTING_HOST_DENY_RULES,
  AI_AGENT_SETTING_WEB_SEARCH_API_KEY,
  AI_AGENT_SETTING_WEB_SEARCH_PROVIDER
} from './protocol'

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
      {
        key: AI_AGENT_SETTING_ENABLE_WEB_ACCESS,
        label: 'Enable web access / download',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_WEB_ACCESS,
        description: 'Allow agent to fetch web pages, convert to text/markdown, and download resources.'
      },
      {
        key: AI_AGENT_SETTING_ENABLE_WEB_SEARCH,
        label: 'Enable web search',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_WEB_SEARCH,
        description: 'Allow agent to search the web (Bing by default).'
      },
      {
        key: AI_AGENT_SETTING_WEB_SEARCH_PROVIDER,
        label: 'Web search provider',
        type: 'select',
        default: AI_AGENT_DEFAULT_WEB_SEARCH_PROVIDER,
        options: AI_AGENT_SEARCH_PROVIDER_OPTIONS,
        description: 'Default search engine used for web searches.'
      },
      {
        key: AI_AGENT_SETTING_WEB_SEARCH_API_KEY,
        label: 'Web search API key',
        type: 'string',
        default: '',
        secret: true,
        description: 'Optional API key for search engine provider (Brave, Google, or Bing API).'
      },
      {
        key: AI_AGENT_SETTING_ENABLE_REMOTE_FS_ACCESS,
        label: 'Enable remote filesystem access',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_REMOTE_FS_ACCESS,
        description: 'Allow agent to read/list files on the connected remote SSH server via SFTP (destructive edits ask for approval).'
      },
      {
        key: AI_AGENT_SETTING_ENABLE_LOCAL_FS_ACCESS,
        label: 'Enable local filesystem access',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_LOCAL_FS_ACCESS,
        description: 'Allow agent to read/list files on your local computer running WaSSH (writes ask for approval).'
      },
      {
        key: AI_AGENT_SETTING_ENABLE_DATETIME_ACCESS,
        label: 'Enable date & time access',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_DATETIME_ACCESS,
        description: 'Allow agent to query the current local time, date, and timezone.'
      }
    ],
    hostSettingsHeading: 'AI agent',
    hostSettingsSchema: [
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
      {
        key: AI_AGENT_SETTING_ENABLE_WEB_ACCESS,
        label: 'Enable web access / download',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_WEB_ACCESS,
        description: 'Allow agent to fetch web pages and resources for this host/session.'
      },
      {
        key: AI_AGENT_SETTING_ENABLE_WEB_SEARCH,
        label: 'Enable web search',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_WEB_SEARCH,
        description: 'Allow agent to search the web for this host/session.'
      },
      {
        key: AI_AGENT_SETTING_WEB_SEARCH_PROVIDER,
        label: 'Web search provider',
        type: 'select',
        default: AI_AGENT_DEFAULT_WEB_SEARCH_PROVIDER,
        options: AI_AGENT_SEARCH_PROVIDER_OPTIONS,
        description: 'Search engine for this host/session.'
      },
      {
        key: AI_AGENT_SETTING_ENABLE_REMOTE_FS_ACCESS,
        label: 'Enable remote filesystem access',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_REMOTE_FS_ACCESS,
        description: 'Allow agent to read/list files on this remote server via SFTP.'
      },
      {
        key: AI_AGENT_SETTING_ENABLE_LOCAL_FS_ACCESS,
        label: 'Enable local filesystem access',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_LOCAL_FS_ACCESS,
        description: 'Allow agent to read/list files on your local computer.'
      },
      {
        key: AI_AGENT_SETTING_ENABLE_DATETIME_ACCESS,
        label: 'Enable date & time access',
        type: 'boolean',
        default: AI_AGENT_DEFAULT_DATETIME_ACCESS,
        description: 'Allow agent to query current date/time.'
      }
    ],
    views: [{ id: 'panel', placement: 'split-right', title: 'AI agent' }]
  }
}
