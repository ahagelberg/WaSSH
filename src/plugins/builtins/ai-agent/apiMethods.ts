import type { PluginApiMethod, PluginSettingsField } from '@plugin-api/shared'
import { PLUGIN_ID_AI_AGENT } from './id'
import {
  AI_AGENT_GROUP_PERMISSIONS_ROOT,
  buildApiPermissionGroup,
  buildContainerGroup,
  buildPermissionField
} from './pluginApiTools'
import {
  AI_AGENT_TOOL_GET_CURRENT_TIME,
  AI_AGENT_TOOL_LOCAL_FS_LIST,
  AI_AGENT_TOOL_LOCAL_FS_READ,
  AI_AGENT_TOOL_LOCAL_FS_WRITE,
  AI_AGENT_TOOL_REMOTE_FS_DELETE,
  AI_AGENT_TOOL_REMOTE_FS_LIST,
  AI_AGENT_TOOL_REMOTE_FS_READ,
  AI_AGENT_TOOL_REMOTE_FS_WRITE,
  AI_AGENT_TOOL_RUN_COMMAND,
  AI_AGENT_TOOL_WEB_FETCH,
  AI_AGENT_TOOL_WEB_SEARCH
} from './protocol'

/**
 * Declared shape of every method AI Agent exposes via `contributes.api` /
 * `onApiCall`. Pure data (no Node/Electron dependencies) so it can be
 * imported from `manifest.ts`, which is also compiled for the renderer.
 */
export const TOOL_DEF_RUN_COMMAND: PluginApiMethod = {
  name: AI_AGENT_TOOL_RUN_COMMAND,
  description:
    'Run a single non-interactive shell command on the remote SSH host. Output and exit code are returned. Avoid interactive tools (vim, less, top, etc.). Use several calls to work step by step.',
  parameters: {
    type: 'object',
    properties: {
      command: {
        type: 'string',
        description: 'The shell command to execute on the remote host'
      }
    },
    required: ['command']
  }
}

export const TOOL_DEF_GET_CURRENT_TIME: PluginApiMethod = {
  name: AI_AGENT_TOOL_GET_CURRENT_TIME,
  description:
    'Get the current date, time, timezone, and Unix timestamp from the local client system.',
  parameters: {
    type: 'object',
    properties: {}
  }
}

export const TOOL_DEF_WEB_FETCH: PluginApiMethod = {
  name: AI_AGENT_TOOL_WEB_FETCH,
  description:
    'Fetch the content of a web page (HTTP/HTTPS URL) and extract readable text/markdown or download the text content.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      url: {
        type: 'string',
        description: 'The HTTP or HTTPS URL to fetch'
      },
      maxChars: {
        type: 'number',
        description: 'Optional maximum number of characters to return (default 16000, max 32000)'
      }
    },
    required: ['url']
  }
}

export const TOOL_DEF_WEB_SEARCH: PluginApiMethod = {
  name: AI_AGENT_TOOL_WEB_SEARCH,
  description:
    'Search the web using a search engine (defaults to Bing). Returns top search result titles, snippets, and URLs.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      query: {
        type: 'string',
        description: 'The search query to search the web for'
      },
      limit: {
        type: 'number',
        description: 'Optional maximum number of results to return (default 5, max 10)'
      }
    },
    required: ['query']
  }
}

export const TOOL_DEF_REMOTE_FS_READ: PluginApiMethod = {
  name: AI_AGENT_TOOL_REMOTE_FS_READ,
  description:
    'Read a text file from the REMOTE SSH host filesystem via SFTP. Use this to inspect remote files, configs, and scripts.',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Absolute or relative path on the remote SSH host'
      },
      maxChars: {
        type: 'number',
        description: 'Optional max characters to read (default 32000)'
      }
    },
    required: ['path']
  }
}

export const TOOL_DEF_REMOTE_FS_WRITE: PluginApiMethod = {
  name: AI_AGENT_TOOL_REMOTE_FS_WRITE,
  description:
    'Write or overwrite a file on the REMOTE SSH host filesystem via SFTP. Note: Mutating files on the remote server may require user approval.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Absolute or relative path on the remote SSH host'
      },
      content: {
        type: 'string',
        description: 'The full text content to write to the file'
      }
    },
    required: ['path', 'content']
  }
}

export const TOOL_DEF_REMOTE_FS_LIST: PluginApiMethod = {
  name: AI_AGENT_TOOL_REMOTE_FS_LIST,
  description:
    'List files and directories in a directory on the REMOTE SSH host filesystem via SFTP.',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Directory path on the remote SSH host (default: current directory or "/")'
      }
    }
  }
}

export const TOOL_DEF_REMOTE_FS_DELETE: PluginApiMethod = {
  name: AI_AGENT_TOOL_REMOTE_FS_DELETE,
  description:
    'Delete a file or directory on the REMOTE SSH host filesystem via SFTP. This destructive action requires user approval.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Absolute or relative path on the remote SSH host to delete'
      }
    },
    required: ['path']
  }
}

export const TOOL_DEF_LOCAL_FS_READ: PluginApiMethod = {
  name: AI_AGENT_TOOL_LOCAL_FS_READ,
  description:
    'Read a text file from the LOCAL client machine running WaSSH (NOT the remote SSH server). Use this when the user asks to inspect a file on their local PC/Mac.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'File path on the local user machine'
      },
      maxChars: {
        type: 'number',
        description: 'Optional max characters to read (default 32000)'
      }
    },
    required: ['path']
  }
}

export const TOOL_DEF_LOCAL_FS_WRITE: PluginApiMethod = {
  name: AI_AGENT_TOOL_LOCAL_FS_WRITE,
  description:
    'Write or overwrite a text file on the LOCAL client machine running WaSSH (NOT the remote SSH server). This mutating action requires user approval.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'File path on the local user machine'
      },
      content: {
        type: 'string',
        description: 'The full text content to write to the file'
      }
    },
    required: ['path', 'content']
  }
}

export const TOOL_DEF_LOCAL_FS_LIST: PluginApiMethod = {
  name: AI_AGENT_TOOL_LOCAL_FS_LIST,
  description:
    'List files and directories in a folder on the LOCAL client machine running WaSSH (NOT the remote SSH server).',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Directory path on the local user machine'
      }
    },
    required: ['path']
  }
}

export const AI_AGENT_GROUP_WEB_ACCESS = 'web-access'
export const AI_AGENT_GROUP_WEB_SEARCH = 'web-search'
export const AI_AGENT_GROUP_REMOTE_FS = 'remote-fs'
export const AI_AGENT_GROUP_LOCAL_FS = 'local-fs'

export const AI_AGENT_WEB_ACCESS_METHODS = [TOOL_DEF_WEB_FETCH]
export const AI_AGENT_WEB_SEARCH_METHODS = [TOOL_DEF_WEB_SEARCH]
export const AI_AGENT_REMOTE_FS_METHODS = [
  TOOL_DEF_REMOTE_FS_READ,
  TOOL_DEF_REMOTE_FS_LIST,
  TOOL_DEF_REMOTE_FS_WRITE,
  TOOL_DEF_REMOTE_FS_DELETE
]
export const AI_AGENT_LOCAL_FS_METHODS = [
  TOOL_DEF_LOCAL_FS_READ,
  TOOL_DEF_LOCAL_FS_LIST,
  TOOL_DEF_LOCAL_FS_WRITE
]
export const AI_AGENT_DATETIME_METHOD = TOOL_DEF_GET_CURRENT_TIME

/**
 * Every method AI Agent exposes via `contributes.api` / `onApiCall`, dispatched
 * uniformly through `ctx.callPluginApi(PLUGIN_ID_AI_AGENT, method, params)` -
 * including `run_command`, whose own allow/deny rule-list gate (unchanged)
 * replaces the generic per-method permission below.
 */
export const AI_AGENT_API_METHODS = [
  TOOL_DEF_RUN_COMMAND,
  ...AI_AGENT_WEB_ACCESS_METHODS,
  ...AI_AGENT_WEB_SEARCH_METHODS,
  ...AI_AGENT_REMOTE_FS_METHODS,
  ...AI_AGENT_LOCAL_FS_METHODS,
  AI_AGENT_DATETIME_METHOD
]

/**
 * Settings fields for AI Agent's own built-in capability groups. Used for
 * both `settingsSchema` (app-wide) and `hostSettingsSchema` (per-host
 * override) - identical field/key definitions in both, matching how other
 * per-host plugin settings override same-named app-wide keys.
 *
 * All categories nest under one root "Permissions" group (see
 * `AI_AGENT_PERMISSIONS_ROOT_KEY`), so a single master toggle gates every
 * tool permission beneath it; `main/plugins/builtinRegistry.ts` injects each
 * other plugin's own API group as an additional child of that same root
 * (three levels deep: Permissions > plugin/category group > method).
 */
export function buildAiAgentBuiltinApiFields(webSearchExtraChildren: PluginSettingsField[]): PluginSettingsField[] {
  return [
    buildContainerGroup(
      PLUGIN_ID_AI_AGENT,
      AI_AGENT_GROUP_PERMISSIONS_ROOT,
      'Permissions',
      [
        buildApiPermissionGroup(
          PLUGIN_ID_AI_AGENT,
          AI_AGENT_GROUP_WEB_ACCESS,
          'Web access',
          AI_AGENT_WEB_ACCESS_METHODS,
          { groupDefault: true, description: 'Allow the agent to fetch web pages and resources.' }
        ),
        buildApiPermissionGroup(
          PLUGIN_ID_AI_AGENT,
          AI_AGENT_GROUP_WEB_SEARCH,
          'Web search',
          AI_AGENT_WEB_SEARCH_METHODS,
          {
            groupDefault: true,
            description: 'Allow the agent to search the web.',
            extraChildren: webSearchExtraChildren
          }
        ),
        buildApiPermissionGroup(
          PLUGIN_ID_AI_AGENT,
          AI_AGENT_GROUP_REMOTE_FS,
          'Remote filesystem',
          AI_AGENT_REMOTE_FS_METHODS,
          { groupDefault: true, description: 'Allow the agent to read/write files on the connected remote SSH server via SFTP.' }
        ),
        buildApiPermissionGroup(
          PLUGIN_ID_AI_AGENT,
          AI_AGENT_GROUP_LOCAL_FS,
          'Local filesystem',
          AI_AGENT_LOCAL_FS_METHODS,
          { groupDefault: true, description: 'Allow the agent to read/write files on your local computer running WaSSH.' }
        ),
        buildPermissionField(PLUGIN_ID_AI_AGENT, AI_AGENT_DATETIME_METHOD)
      ],
      { description: 'Master switch for every tool the agent can call, including other plugins\u2019 APIs below.' }
    )
  ]
}
