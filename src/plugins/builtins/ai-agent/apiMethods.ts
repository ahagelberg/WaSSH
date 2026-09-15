import type { PluginApiMethod, PluginSettingsField } from '@plugin-api/shared'
import { PLUGIN_ID_AI_AGENT } from './id'
import {
  AI_AGENT_GROUP_PERMISSIONS_ROOT,
  buildApiPermissionGroup,
  buildContainerGroup,
  buildPermissionField
} from './pluginApiTools'
import {
  AI_AGENT_TOOL_DEV_CREATE_FILE,
  AI_AGENT_TOOL_DEV_DIFF_FILE,
  AI_AGENT_TOOL_DEV_EDIT_FILE,
  AI_AGENT_TOOL_DEV_FIND_FILES,
  AI_AGENT_TOOL_DEV_GREP,
  AI_AGENT_TOOL_DEV_LIST_DIR,
  AI_AGENT_TOOL_DEV_VIEW_FILE,
  AI_AGENT_TOOL_GET_CURRENT_TIME,
  AI_AGENT_TOOL_LOCAL_FS_LIST,
  AI_AGENT_TOOL_LOCAL_FS_READ,
  AI_AGENT_TOOL_LOCAL_FS_WRITE,
  AI_AGENT_TOOL_LOCAL_FS_EDIT,
  AI_AGENT_TOOL_REMOTE_FS_DELETE,
  AI_AGENT_TOOL_REMOTE_FS_EDIT,
  AI_AGENT_TOOL_REMOTE_FS_LIST,
  AI_AGENT_TOOL_REMOTE_FS_READ,
  AI_AGENT_TOOL_REMOTE_FS_WRITE,
  AI_AGENT_TOOL_RAG_SEARCH,
  AI_AGENT_TOOL_RUN_COMMAND,
  AI_AGENT_TOOL_TERMINAL_KEYS,
  AI_AGENT_TOOL_TERMINAL_READ,
  AI_AGENT_TOOL_TERMINAL_WAIT,
  AI_AGENT_TOOL_TERMINAL_WRITE,
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

export const TOOL_DEF_RAG_SEARCH: PluginApiMethod = {
  name: AI_AGENT_TOOL_RAG_SEARCH,
  description:
    'Search the local knowledge base (app-wide and/or per-host folders configured in AI agent settings) for text chunks relevant to a query, using embeddings from the active provider. Returns the most relevant chunks with their source file and similarity score.',
  parameters: {
    type: 'object',
    properties: {
      query: {
        type: 'string',
        description: 'The search query to find relevant knowledge base content for'
      },
      limit: {
        type: 'number',
        description: 'Optional maximum number of chunks to return (default 5, max 10)'
      }
    },
    required: ['query']
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

export const TOOL_DEF_REMOTE_FS_EDIT: PluginApiMethod = {
  name: AI_AGENT_TOOL_REMOTE_FS_EDIT,
  description:
    'Edit a text file on the REMOTE SSH host filesystem via SFTP by replacing an exact block of existing text with new text. ' +
    'The oldText must match exactly once in the file (including whitespace/indentation); use enough surrounding context to make it unique. ' +
    'Use this instead of rewriting whole files for small changes. Mutating files may require user approval.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Absolute or relative path on the remote SSH host'
      },
      oldText: {
        type: 'string',
        description: 'The exact, unique block of existing text to find and replace'
      },
      newText: {
        type: 'string',
        description: 'The text to replace oldText with'
      }
    },
    required: ['path', 'oldText', 'newText']
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
  defaultPermission: 'ask',
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

export const TOOL_DEF_LOCAL_FS_EDIT: PluginApiMethod = {
  name: AI_AGENT_TOOL_LOCAL_FS_EDIT,
  description:
    'Edit a text file on the LOCAL client machine running WaSSH by replacing an exact block of existing text with new text. ' +
    'The oldText must match exactly once in the file (including whitespace/indentation); use enough surrounding context to make it unique. ' +
    'Use this instead of rewriting whole files for small changes. This mutating action requires user approval.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'File path on the local user machine'
      },
      oldText: {
        type: 'string',
        description: 'The exact, unique block of existing text to find and replace'
      },
      newText: {
        type: 'string',
        description: 'The text to replace oldText with'
      }
    },
    required: ['path', 'oldText', 'newText']
  }
}

export const TOOL_DEF_DEV_VIEW_FILE: PluginApiMethod = {
  name: AI_AGENT_TOOL_DEV_VIEW_FILE,
  description:
    'View the contents of a file on the remote host with line numbers. Optionally specify startLine and endLine (1-based) to inspect sections of large files.',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'File path on the remote host (relative to working directory or absolute)'
      },
      startLine: {
        type: 'number',
        description: 'Optional 1-based start line number'
      },
      endLine: {
        type: 'number',
        description: 'Optional 1-based end line number'
      }
    },
    required: ['path']
  }
}

export const TOOL_DEF_DEV_EDIT_FILE: PluginApiMethod = {
  name: AI_AGENT_TOOL_DEV_EDIT_FILE,
  description:
    'Edit a file on the remote host by replacing an exact, unique block of text (oldText) with newText. ' +
    'The oldText must match exactly once in the file (including indentation and line breaks). Mutating action.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'File path on the remote host (relative to working directory or absolute)'
      },
      oldText: {
        type: 'string',
        description: 'The exact, unique block of existing text to find and replace'
      },
      newText: {
        type: 'string',
        description: 'The text to replace oldText with'
      }
    },
    required: ['path', 'oldText', 'newText']
  }
}

export const TOOL_DEF_DEV_CREATE_FILE: PluginApiMethod = {
  name: AI_AGENT_TOOL_DEV_CREATE_FILE,
  description:
    'Create a new file on the remote host with the specified content. Fails if the file already exists unless overwrite is set to true. Mutating action.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'File path on the remote host (relative to working directory or absolute)'
      },
      content: {
        type: 'string',
        description: 'The full text content to write to the file'
      },
      overwrite: {
        type: 'boolean',
        description: 'Whether to overwrite an existing file (default: false)'
      }
    },
    required: ['path', 'content']
  }
}

export const TOOL_DEF_DEV_LIST_DIR: PluginApiMethod = {
  name: AI_AGENT_TOOL_DEV_LIST_DIR,
  description:
    'List files and subdirectories in a directory on the remote host with file types, sizes, and permissions.',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Directory path on the remote host (defaults to current working directory)'
      },
      recursive: {
        type: 'boolean',
        description: 'Whether to recursively list directory tree up to depth 3 (default: false)'
      }
    }
  }
}

export const TOOL_DEF_DEV_GREP: PluginApiMethod = {
  name: AI_AGENT_TOOL_DEV_GREP,
  description:
    'Search for text or regular expression patterns across files in the remote host workspace. Returns matching lines with line numbers and file paths.',
  parameters: {
    type: 'object',
    properties: {
      pattern: {
        type: 'string',
        description: 'The regular expression or text pattern to search for'
      },
      path: {
        type: 'string',
        description: 'Directory or file to search in (defaults to current working directory)'
      },
      caseSensitive: {
        type: 'boolean',
        description: 'Whether the search is case-sensitive (default: false)'
      },
      glob: {
        type: 'string',
        description: 'Optional file name pattern filter, e.g. "*.ts" or "*.py"'
      }
    },
    required: ['pattern']
  }
}

export const TOOL_DEF_DEV_FIND_FILES: PluginApiMethod = {
  name: AI_AGENT_TOOL_DEV_FIND_FILES,
  description:
    'Find files by name or wildcard pattern on the remote host (e.g. "*.json", "config.*", "test_*.py").',
  parameters: {
    type: 'object',
    properties: {
      pattern: {
        type: 'string',
        description: 'File name or wildcard pattern to search for (e.g. "*.ts", "package.json")'
      },
      path: {
        type: 'string',
        description: 'Directory to search in (defaults to current working directory)'
      }
    },
    required: ['pattern']
  }
}

export const TOOL_DEF_DEV_DIFF_FILE: PluginApiMethod = {
  name: AI_AGENT_TOOL_DEV_DIFF_FILE,
  description:
    'Diff a file on the remote host against expected text, or apply a unified patch, without modifying it unless apply is set. ' +
    'Two modes: (1) pass the exact block of text you expect the file to contain as oldText to get a unified diff of expectation vs. reality, ' +
    'plus a summary of lines added, removed, and kept - use this to verify a file\u2019s current state or confirm an edit took effect; ' +
    '(2) pass a unified diff (git diff / diff -u / git format-patch output, including multi-hunk and partial-hunk patches) as patch ' +
    'to check whether it applies cleanly, with apply: true to write it. ' +
    'In oldText mode, apply: true replaces the matched oldText with newText. ' +
    'Patch mode fails with a diagnostic when a hunk does not match, so you can re-read the file and regenerate.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'File path on the remote host (relative to working directory or absolute)'
      },
      oldText: {
        type: 'string',
        description:
          'The exact block of text expected to be present in the file (including whitespace/indentation). May be empty to diff against an empty file. Not used in patch mode.'
      },
      newText: {
        type: 'string',
        description: 'Replacement text. Only used in oldText mode when apply is true.'
      },
      patch: {
        type: 'string',
        description:
          'A unified diff to apply to the file. Accepts full git diffs with headers and multiple @@ hunks. ' +
          'Only the hunks for this file are used; file paths in the patch header are ignored in favour of path.'
      },
      apply: {
        type: 'boolean',
        description:
          'Whether to write the change instead of only reporting the diff (default: false). In patch mode this writes the patched content; in oldText mode it writes newText.'
      },
      contextLines: {
        type: 'number',
        description: 'Number of unchanged context lines shown around each change (default 3, max 20)'
      }
    },
    required: ['path']
  }
}

export const TOOL_DEF_TERMINAL_READ: PluginApiMethod = {
  name: AI_AGENT_TOOL_TERMINAL_READ,
  description:
    'Read what is currently shown in the user\u2019s live terminal, with ANSI colour and cursor codes stripped. ' +
    'Use this to see the state of the shell the user is working in, or the output of a command they ran themselves. ' +
    'Only available when terminal access is enabled for this host.',
  parameters: {
    type: 'object',
    properties: {
      maxChars: {
        type: 'number',
        description: 'Optional maximum characters to return, taken from the end of the buffer (default 24000)'
      }
    }
  }
}

export const TOOL_DEF_TERMINAL_WRITE: PluginApiMethod = {
  name: AI_AGENT_TOOL_TERMINAL_WRITE,
  description:
    'Type text into the user\u2019s live terminal, as if the user typed it. ' +
    'Set pressEnter to run it as a command (default true); set it to false to fill in a prompt without submitting. ' +
    'This shares the user\u2019s shell and prompt, so anything typed appears in their session. ' +
    'Prefer run_command for ordinary non-interactive commands - use this for interactive programs (vim, top, ssh, REPLs) ' +
    'or when you must act in the user\u2019s own shell. Follow with terminal_wait to observe the result.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      text: {
        type: 'string',
        description: 'The text to type into the terminal'
      },
      pressEnter: {
        type: 'boolean',
        description: 'Whether to press Enter after typing the text (default: true)'
      }
    },
    required: ['text']
  }
}

export const TOOL_DEF_TERMINAL_KEYS: PluginApiMethod = {
  name: AI_AGENT_TOOL_TERMINAL_KEYS,
  description:
    'Send named keys to the user\u2019s live terminal, for navigating interactive programs. ' +
    'Supports enter, tab, escape, backspace, space, arrow keys, home/end/pageup/pagedown, and ctrl+a through ctrl+z ' +
    '(e.g. ctrl+c to interrupt, ctrl+d to end input, ctrl+x to exit an editor). ' +
    'Keys are sent in order in a single write.',
  defaultPermission: 'ask',
  parameters: {
    type: 'object',
    properties: {
      keys: {
        type: 'array',
        items: { type: 'string' },
        description: 'Key names to send in order, e.g. ["ctrl+x"] or ["down", "down", "enter"]'
      }
    },
    required: ['keys']
  }
}

export const TOOL_DEF_TERMINAL_WAIT: PluginApiMethod = {
  name: AI_AGENT_TOOL_TERMINAL_WAIT,
  description:
    'Wait for the user\u2019s live terminal to stop producing output, then return its contents. ' +
    'Use this after terminal_write or terminal_send_keys to observe the result of an interactive command. ' +
    'Returns as soon as output has been quiet briefly, or when the timeout is reached.',
  parameters: {
    type: 'object',
    properties: {
      timeoutMs: {
        type: 'number',
        description: 'Maximum time to wait in milliseconds (default 5000, max 60000)'
      },
      maxChars: {
        type: 'number',
        description: 'Optional maximum characters of terminal contents to return (default 24000)'
      }
    }
  }
}

export const AI_AGENT_GROUP_WEB_ACCESS = 'web-access'
export const AI_AGENT_GROUP_REMOTE_FS = 'remote-fs'
export const AI_AGENT_GROUP_LOCAL_FS = 'local-fs'
export const AI_AGENT_GROUP_KNOWLEDGE_BASE = 'knowledge-base'
export const AI_AGENT_GROUP_TERMINAL = 'terminal'

export const AI_AGENT_DEV_TOOLS_METHODS = [
  TOOL_DEF_DEV_VIEW_FILE,
  TOOL_DEF_DEV_EDIT_FILE,
  TOOL_DEF_DEV_CREATE_FILE,
  TOOL_DEF_DEV_LIST_DIR,
  TOOL_DEF_DEV_GREP,
  TOOL_DEF_DEV_FIND_FILES,
  TOOL_DEF_DEV_DIFF_FILE
]
export const AI_AGENT_WEB_ACCESS_METHODS = [TOOL_DEF_WEB_FETCH]
export const AI_AGENT_WEB_SEARCH_METHODS = [TOOL_DEF_WEB_SEARCH]
export const AI_AGENT_WEB_ACCESS_ALL_METHODS = [
  ...AI_AGENT_WEB_ACCESS_METHODS,
  ...AI_AGENT_WEB_SEARCH_METHODS
]
export const AI_AGENT_REMOTE_FS_METHODS = [
  TOOL_DEF_REMOTE_FS_READ,
  TOOL_DEF_REMOTE_FS_LIST,
  TOOL_DEF_REMOTE_FS_WRITE,
  TOOL_DEF_REMOTE_FS_EDIT,
  TOOL_DEF_REMOTE_FS_DELETE
]
export const AI_AGENT_REMOTE_FILESYSTEM_METHODS = [
  ...AI_AGENT_DEV_TOOLS_METHODS,
  ...AI_AGENT_REMOTE_FS_METHODS
]
export const AI_AGENT_LOCAL_FS_METHODS = [
  TOOL_DEF_LOCAL_FS_READ,
  TOOL_DEF_LOCAL_FS_LIST,
  TOOL_DEF_LOCAL_FS_WRITE,
  TOOL_DEF_LOCAL_FS_EDIT
]
export const AI_AGENT_KNOWLEDGE_BASE_METHODS = [TOOL_DEF_RAG_SEARCH]
export const AI_AGENT_TERMINAL_METHODS = [
  TOOL_DEF_TERMINAL_READ,
  TOOL_DEF_TERMINAL_WAIT,
  TOOL_DEF_TERMINAL_WRITE,
  TOOL_DEF_TERMINAL_KEYS
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
  ...AI_AGENT_DEV_TOOLS_METHODS,
  ...AI_AGENT_WEB_ACCESS_METHODS,
  ...AI_AGENT_WEB_SEARCH_METHODS,
  ...AI_AGENT_REMOTE_FS_METHODS,
  ...AI_AGENT_LOCAL_FS_METHODS,
  ...AI_AGENT_KNOWLEDGE_BASE_METHODS,
  ...AI_AGENT_TERMINAL_METHODS,
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
          AI_AGENT_WEB_ACCESS_ALL_METHODS,
          {
            groupDefault: true,
            description: 'Allow the agent to fetch web pages and search the web.',
            extraChildren: webSearchExtraChildren
          }
        ),
        buildApiPermissionGroup(
          PLUGIN_ID_AI_AGENT,
          AI_AGENT_GROUP_REMOTE_FS,
          'Remote filesystem',
          AI_AGENT_REMOTE_FILESYSTEM_METHODS,
          {
            groupDefault: true,
            description:
              'Allow the agent to inspect, search, create, and edit files on the connected remote SSH server.'
          }
        ),
        buildApiPermissionGroup(
          PLUGIN_ID_AI_AGENT,
          AI_AGENT_GROUP_LOCAL_FS,
          'Local filesystem',
          AI_AGENT_LOCAL_FS_METHODS,
          { groupDefault: true, description: 'Allow the agent to read/write files on your local computer running WaSSH.' }
        ),
        buildApiPermissionGroup(
          PLUGIN_ID_AI_AGENT,
          AI_AGENT_GROUP_KNOWLEDGE_BASE,
          'Knowledge base',
          AI_AGENT_KNOWLEDGE_BASE_METHODS,
          {
            groupDefault: true,
            description:
              'Allow the agent to search the local knowledge base folder(s) configured below (app-wide and/or per-host).'
          }
        ),
        buildApiPermissionGroup(
          PLUGIN_ID_AI_AGENT,
          AI_AGENT_GROUP_TERMINAL,
          'Live terminal',
          AI_AGENT_TERMINAL_METHODS,
          {
            groupDefault: false,
            description:
              'Allow the agent to read and type into your live terminal session. This shares your shell and prompt, so the agent acts as if it were you typing.'
          }
        ),
        buildPermissionField(PLUGIN_ID_AI_AGENT, AI_AGENT_DATETIME_METHOD)
      ],
      { description: 'Master switch for every tool the agent can call, including other plugins\u2019 APIs below.' }
    )
  ]
}
