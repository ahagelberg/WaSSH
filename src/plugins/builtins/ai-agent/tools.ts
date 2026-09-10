import fs from 'fs'
import path from 'path'
import {
  AI_AGENT_TOOL_RUN_COMMAND,
  AI_AGENT_TOOL_WEB_FETCH,
  AI_AGENT_TOOL_WEB_SEARCH,
  AI_AGENT_TOOL_REMOTE_FS_READ,
  AI_AGENT_TOOL_REMOTE_FS_WRITE,
  AI_AGENT_TOOL_REMOTE_FS_LIST,
  AI_AGENT_TOOL_REMOTE_FS_DELETE,
  AI_AGENT_TOOL_LOCAL_FS_READ,
  AI_AGENT_TOOL_LOCAL_FS_WRITE,
  AI_AGENT_TOOL_LOCAL_FS_LIST,
  AI_AGENT_TOOL_GET_CURRENT_TIME,
  type AiAgentWebSearchProvider
} from './protocol'
import type { PluginMainContext } from '@plugin-api/main'
import type { SftpSession } from '@plugin-api/main'

/** Max characters returned for web fetch or file read to prevent token overflow */
export const MAX_FETCH_CHARS = 32_000
export const MAX_FILE_READ_CHARS = 48_000
export const MAX_DIR_ENTRIES = 200

export interface ToolDefinition {
  name: string
  description: string
  parameters: {
    type: 'object'
    properties: Record<string, unknown>
    required?: string[]
  }
}

export const TOOL_DEF_RUN_COMMAND: ToolDefinition = {
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

export const TOOL_DEF_GET_CURRENT_TIME: ToolDefinition = {
  name: AI_AGENT_TOOL_GET_CURRENT_TIME,
  description:
    'Get the current date, time, timezone, and Unix timestamp from the local client system.',
  parameters: {
    type: 'object',
    properties: {}
  }
}

export const TOOL_DEF_WEB_FETCH: ToolDefinition = {
  name: AI_AGENT_TOOL_WEB_FETCH,
  description:
    'Fetch the content of a web page (HTTP/HTTPS URL) and extract readable text/markdown or download the text content.',
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

export const TOOL_DEF_WEB_SEARCH: ToolDefinition = {
  name: AI_AGENT_TOOL_WEB_SEARCH,
  description:
    'Search the web using a search engine (defaults to Bing). Returns top search result titles, snippets, and URLs.',
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

export const TOOL_DEF_REMOTE_FS_READ: ToolDefinition = {
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

export const TOOL_DEF_REMOTE_FS_WRITE: ToolDefinition = {
  name: AI_AGENT_TOOL_REMOTE_FS_WRITE,
  description:
    'Write or overwrite a file on the REMOTE SSH host filesystem via SFTP. Note: Mutating files on the remote server may require user approval.',
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

export const TOOL_DEF_REMOTE_FS_LIST: ToolDefinition = {
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

export const TOOL_DEF_REMOTE_FS_DELETE: ToolDefinition = {
  name: AI_AGENT_TOOL_REMOTE_FS_DELETE,
  description:
    'Delete a file or directory on the REMOTE SSH host filesystem via SFTP. This destructive action requires user approval.',
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

export const TOOL_DEF_LOCAL_FS_READ: ToolDefinition = {
  name: AI_AGENT_TOOL_LOCAL_FS_READ,
  description:
    'Read a text file from the LOCAL client machine running WaSSH (NOT the remote SSH server). Use this when the user asks to inspect a file on their local PC/Mac.',
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

export const TOOL_DEF_LOCAL_FS_WRITE: ToolDefinition = {
  name: AI_AGENT_TOOL_LOCAL_FS_WRITE,
  description:
    'Write or overwrite a text file on the LOCAL client machine running WaSSH (NOT the remote SSH server). This mutating action requires user approval.',
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

export const TOOL_DEF_LOCAL_FS_LIST: ToolDefinition = {
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

/** Check if a tool is mutating/destructive and therefore requires user confirmation */
export function isMutatingTool(toolName: string): boolean {
  switch (toolName) {
    case AI_AGENT_TOOL_REMOTE_FS_WRITE:
    case AI_AGENT_TOOL_REMOTE_FS_DELETE:
    case AI_AGENT_TOOL_LOCAL_FS_WRITE:
      return true
    default:
      return false
  }
}

/** Build the list of active tool definitions according to current settings */
export function getActiveTools(options: {
  enableWebAccess?: boolean
  enableWebSearch?: boolean
  enableRemoteFsAccess?: boolean
  enableLocalFsAccess?: boolean
  enableDateTimeAccess?: boolean
}): ToolDefinition[] {
  const tools: ToolDefinition[] = [TOOL_DEF_RUN_COMMAND]

  if (options.enableDateTimeAccess ?? true) {
    tools.push(TOOL_DEF_GET_CURRENT_TIME)
  }
  if (options.enableWebAccess ?? true) {
    tools.push(TOOL_DEF_WEB_FETCH)
  }
  if (options.enableWebSearch ?? true) {
    tools.push(TOOL_DEF_WEB_SEARCH)
  }
  if (options.enableRemoteFsAccess ?? true) {
    tools.push(
      TOOL_DEF_REMOTE_FS_READ,
      TOOL_DEF_REMOTE_FS_WRITE,
      TOOL_DEF_REMOTE_FS_LIST,
      TOOL_DEF_REMOTE_FS_DELETE
    )
  }
  if (options.enableLocalFsAccess ?? true) {
    tools.push(TOOL_DEF_LOCAL_FS_READ, TOOL_DEF_LOCAL_FS_WRITE, TOOL_DEF_LOCAL_FS_LIST)
  }

  return tools
}

/** Execute Date/Time query */
export function executeDateTime(): string {
  const now = new Date()
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone
  return JSON.stringify(
    {
      iso: now.toISOString(),
      local: now.toString(),
      date: now.toLocaleDateString(),
      time: now.toLocaleTimeString(),
      timezone,
      timestamp: now.getTime(),
      utcOffsetMinutes: -now.getTimezoneOffset()
    },
    null,
    2
  )
}

/** Strip HTML tags and collapse excess whitespace */
function sanitizeHtmlToText(html: string): string {
  // Remove script and style elements with content
  let text = html.replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, ' ')
  text = text.replace(/<style\b[^<]*(?:(?!<\/style>)<[^<]*)*<\/style>/gi, ' ')
  text = text.replace(/<noscript\b[^<]*(?:(?!<\/noscript>)<[^<]*)*<\/noscript>/gi, ' ')
  // Convert standard line-breaking tags to newlines
  text = text.replace(/<\/(p|div|h[1-6]|li|tr|blockquote|article|section)>/gi, '\n')
  text = text.replace(/<br\s*\/?>/gi, '\n')
  // Strip remaining HTML tags
  text = text.replace(/<[^>]+>/g, ' ')
  // Decode common HTML entities
  text = text
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&#x27;/g, "'")
    .replace(/&#x2F;/g, '/')
    .replace(/&mdash;/g, '—')
    .replace(/&ndash;/g, '–')
  // Collapse whitespace
  text = text.replace(/[ \t]+/g, ' ')
  text = text.replace(/\n\s*\n\s*\n+/g, '\n\n')
  return text.trim()
}

/** Execute Web Fetch */
export async function executeWebFetch(urlStr: string, maxChars?: number): Promise<string> {
  const limit = Math.min(Math.max(Number(maxChars) || 16_000, 1_000), MAX_FETCH_CHARS)
  let parsedUrl: URL
  try {
    parsedUrl = new URL(urlStr)
    if (parsedUrl.protocol !== 'http:' && parsedUrl.protocol !== 'https:') {
      return `Error: Only HTTP and HTTPS URLs are supported (got ${parsedUrl.protocol})`
    }
  } catch (err) {
    return `Error: Invalid URL "${urlStr}": ${err instanceof Error ? err.message : String(err)}`
  }

  try {
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(), 20_000)
    const res = await fetch(parsedUrl.toString(), {
      method: 'GET',
      headers: {
        'User-Agent':
          'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36 WaSSH-Agent/1.0',
        Accept: 'text/html,application/xhtml+xml,application/json,text/plain;q=0.9,*/*;q=0.8'
      },
      signal: controller.signal
    }).finally(() => clearTimeout(timer))

    if (!res.ok) {
      return `HTTP error ${res.status} ${res.statusText} fetching ${urlStr}`
    }

    const contentType = res.headers.get('content-type') || ''
    const rawBody = await res.text()

    let content = rawBody
    if (contentType.includes('html') || /<html\b/i.test(rawBody.slice(0, 1000))) {
      content = sanitizeHtmlToText(rawBody)
    }

    if (content.length > limit) {
      return `${content.slice(0, limit)}\n\n[Content truncated at ${limit} characters of ${content.length}]`
    }
    return content || '(Empty response body)'
  } catch (err) {
    return `Failed to fetch URL ${urlStr}: ${err instanceof Error ? err.message : String(err)}`
  }
}

export interface SearchResult {
  title: string
  url: string
  snippet: string
}

/** Search Bing via web scraping or Bing API */
async function searchBing(query: string, limit: number, apiKey?: string): Promise<SearchResult[]> {
  if (apiKey) {
    const endpoint = `https://api.bing.microsoft.com/v7.0/search?q=${encodeURIComponent(query)}&count=${limit}`
    const res = await fetch(endpoint, {
      headers: { 'Ocp-Apim-Subscription-Key': apiKey }
    })
    if (res.ok) {
      const data = (await res.json()) as {
        webPages?: { value?: Array<{ name?: string; url?: string; snippet?: string }> }
      }
      const items = data.webPages?.value || []
      return items.slice(0, limit).map((item) => ({
        title: item.name || '',
        url: item.url || '',
        snippet: item.snippet || ''
      }))
    }
  }

  // Fallback: Bing HTML search
  const url = `https://www.bing.com/search?q=${encodeURIComponent(query)}`
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 15_000)
  const res = await fetch(url, {
    headers: {
      'User-Agent':
        'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36',
      Accept: 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8'
    },
    signal: controller.signal
  }).finally(() => clearTimeout(timer))

  if (!res.ok) {
    throw new Error(`Bing returned HTTP ${res.status}`)
  }

  const html = await res.text()
  const results: SearchResult[] = []

  // Extract <li class="b_algo"> blocks
  const algoBlocks = html.split(/<li\s+class="b_algo"[^>]*>/i)
  for (let i = 1; i < algoBlocks.length && results.length < limit; i++) {
    const block = algoBlocks[i].split('</li>')[0]
    const titleMatch = /<h2[^>]*><a[^>]+href="([^"]+)"[^>]*>(.*?)<\/a>/i.exec(block)
    if (!titleMatch) continue

    const resultUrl = titleMatch[1]
    const rawTitle = titleMatch[2].replace(/<[^>]+>/g, '')
    const snippetMatch = /<p[^>]*>(.*?)<\/p>/i.exec(block) || /<div class="b_caption"[^>]*>(.*?)<\/div>/i.exec(block)
    const rawSnippet = snippetMatch ? snippetMatch[1].replace(/<[^>]+>/g, '') : ''

    results.push({
      title: rawTitle.trim(),
      url: resultUrl,
      snippet: rawSnippet.trim()
    })
  }

  return results
}

/** Search Brave Search API */
async function searchBrave(query: string, limit: number, apiKey?: string): Promise<SearchResult[]> {
  if (!apiKey) {
    throw new Error('Brave search requires an API key. Please configure webSearchApiKey in settings.')
  }
  const url = `https://api.search.brave.com/res/v1/web/search?q=${encodeURIComponent(query)}&count=${limit}`
  const res = await fetch(url, {
    headers: {
      Accept: 'application/json',
      'X-Subscription-Token': apiKey
    }
  })
  if (!res.ok) {
    throw new Error(`Brave Search API error ${res.status}: ${await res.text()}`)
  }
  const data = (await res.json()) as {
    web?: { results?: Array<{ title?: string; url?: string; description?: string }> }
  }
  const items = data.web?.results || []
  return items.slice(0, limit).map((item) => ({
    title: item.title || '',
    url: item.url || '',
    snippet: item.description || ''
  }))
}

/** Search Google Custom Search */
async function searchGoogle(query: string, limit: number, apiKey?: string): Promise<SearchResult[]> {
  if (!apiKey) {
    throw new Error('Google search requires an API key in format "KEY:CX" (Custom Search Engine ID).')
  }
  const [key, cx] = apiKey.split(':')
  if (!key || !cx) {
    throw new Error('Google search API key must be in format "API_KEY:ENGINE_CX".')
  }
  const url = `https://www.googleapis.com/customsearch/v1?key=${encodeURIComponent(key)}&cx=${encodeURIComponent(cx)}&q=${encodeURIComponent(query)}&num=${limit}`
  const res = await fetch(url)
  if (!res.ok) {
    throw new Error(`Google Search API error ${res.status}: ${await res.text()}`)
  }
  const data = (await res.json()) as {
    items?: Array<{ title?: string; link?: string; snippet?: string }>
  }
  const items = data.items || []
  return items.slice(0, limit).map((item) => ({
    title: item.title || '',
    url: item.link || '',
    snippet: item.snippet || ''
  }))
}

/** Search DuckDuckGo HTML as fallback */
async function searchDuckDuckGo(query: string, limit: number): Promise<SearchResult[]> {
  const url = `https://html.duckduckgo.com/html/?q=${encodeURIComponent(query)}`
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 15_000)
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'User-Agent':
        'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36',
      'Content-Type': 'application/x-www-form-urlencoded'
    },
    body: `q=${encodeURIComponent(query)}`,
    signal: controller.signal
  }).finally(() => clearTimeout(timer))

  if (!res.ok) {
    throw new Error(`DuckDuckGo returned HTTP ${res.status}`)
  }

  const html = await res.text()
  const results: SearchResult[] = []
  const matches = html.split(/<div class="result__body">/i)

  for (let i = 1; i < matches.length && results.length < limit; i++) {
    const block = matches[i].split('</div>')[0]
    const linkMatch = /<a class="result__url"[^>]+href="([^"]+)"[^>]*>(.*?)<\/a>/i.exec(block) ||
      /<a class="result__snippet"[^>]+href="([^"]+)"[^>]*>/i.exec(block)
    const titleMatch = /<a class="result__a"[^>]+href="([^"]+)"[^>]*>(.*?)<\/a>/i.exec(block)
    const snippetMatch = /<a class="result__snippet"[^>]*>(.*?)<\/a>/i.exec(block)

    if (titleMatch) {
      const rawTitle = titleMatch[2].replace(/<[^>]+>/g, '').trim()
      const rawUrl = titleMatch[1]
      const rawSnippet = snippetMatch ? snippetMatch[1].replace(/<[^>]+>/g, '').trim() : ''
      results.push({
        title: rawTitle,
        url: rawUrl,
        snippet: rawSnippet
      })
    }
  }

  return results
}

/** Execute Web Search across configured providers */
export async function executeWebSearch(
  query: string,
  provider?: AiAgentWebSearchProvider | string,
  apiKey?: string,
  limit?: number
): Promise<string> {
  const count = Math.min(Math.max(Number(limit) || 5, 1), 10)
  const chosenProvider = (provider || 'bing').toLowerCase()

  try {
    let results: SearchResult[] = []
    if (chosenProvider === 'brave') {
      results = await searchBrave(query, count, apiKey)
    } else if (chosenProvider === 'google') {
      results = await searchGoogle(query, count, apiKey)
    } else if (chosenProvider === 'duckduckgo') {
      results = await searchDuckDuckGo(query, count)
    } else {
      // Default to Bing
      results = await searchBing(query, count, apiKey)
    }

    if (results.length === 0) {
      return `No results found for query: "${query}"`
    }

    const formatted = results
      .map(
        (r, idx) =>
          `[${idx + 1}] ${r.title}\nURL: ${r.url}\n${r.snippet ? `${r.snippet}\n` : ''}`
      )
      .join('\n')

    return `Search results for "${query}" (${results.length} results):\n\n${formatted}`
  } catch (err) {
    return `Search failed for query "${query}": ${err instanceof Error ? err.message : String(err)}`
  }
}

/** Execute Remote SFTP Read */
export async function executeRemoteFsRead(
  ctx: PluginMainContext,
  filePath: string,
  maxChars?: number
): Promise<string> {
  const limit = Math.min(Math.max(Number(maxChars) || 32_000, 1_000), MAX_FILE_READ_CHARS)
  let sftpSession: SftpSession | null = null
  try {
    sftpSession = await ctx.openSftp()
  } catch (err) {
    return `Failed to open SFTP session on remote host: ${err instanceof Error ? err.message : String(err)}`
  }

  if (!sftpSession) {
    return 'Remote SFTP is not available on this session.'
  }

  return new Promise<string>((resolve) => {
    try {
      const stream = sftpSession.createReadStream(filePath)
      let content = ''
      let truncated = false

      stream.on('data', (chunk: Buffer | string) => {
        content += chunk.toString('utf-8')
        if (content.length > limit) {
          truncated = true
          content = content.slice(0, limit)
          stream.destroy()
        }
      })

      stream.on('error', (err: Error) => {
        resolve(`Error reading remote file "${filePath}": ${err.message}`)
      })

      stream.on('close', () => {
        if (truncated) {
          resolve(`${content}\n\n[Remote file truncated at ${limit} characters]`)
        } else {
          resolve(content || '(Empty file)')
        }
      })
    } catch (err) {
      resolve(`Error reading remote file "${filePath}": ${err instanceof Error ? err.message : String(err)}`)
    }
  })
}

/** Execute Remote SFTP Write */
export async function executeRemoteFsWrite(
  ctx: PluginMainContext,
  filePath: string,
  content: string
): Promise<string> {
  let sftpSession: SftpSession | null = null
  try {
    sftpSession = await ctx.openSftp()
  } catch (err) {
    return `Failed to open SFTP session on remote host: ${err instanceof Error ? err.message : String(err)}`
  }

  if (!sftpSession) {
    return 'Remote SFTP is not available on this session.'
  }

  return new Promise<string>((resolve) => {
    try {
      const stream = sftpSession.createWriteStream(filePath)
      stream.on('error', (err: Error) => {
        resolve(`Error writing remote file "${filePath}": ${err.message}`)
      })
      stream.on('finish', () => {
        resolve(`Successfully wrote ${Buffer.byteLength(content, 'utf-8')} bytes to remote file "${filePath}".`)
      })
      stream.end(content, 'utf-8')
    } catch (err) {
      resolve(`Error writing remote file "${filePath}": ${err instanceof Error ? err.message : String(err)}`)
    }
  })
}

/** Execute Remote SFTP List */
export async function executeRemoteFsList(
  ctx: PluginMainContext,
  dirPath?: string
): Promise<string> {
  const targetDir = dirPath && dirPath.trim().length > 0 ? dirPath.trim() : '.'
  let sftpSession: SftpSession | null = null
  try {
    sftpSession = await ctx.openSftp()
  } catch (err) {
    return `Failed to open SFTP session on remote host: ${err instanceof Error ? err.message : String(err)}`
  }

  if (!sftpSession) {
    return 'Remote SFTP is not available on this session.'
  }

  try {
    const entries = await sftpSession.list(targetDir)
    if (entries.length === 0) {
      return `Remote directory "${targetDir}" is empty.`
    }
    const lines = entries.slice(0, MAX_DIR_ENTRIES).map((e: { type: string; modeSymbolic?: string; size: number; name: string }) => {
      const isDir = e.type === 'directory' ? '/' : ''
      const sizeStr = e.type === 'directory' ? '<DIR>' : `${e.size}B`
      return `${e.modeSymbolic || ''} ${sizeStr.padStart(10)} ${e.name}${isDir}`
    })
    const extra = entries.length > MAX_DIR_ENTRIES ? `\n... (${entries.length - MAX_DIR_ENTRIES} more entries truncated)` : ''
    return `Remote directory listing for "${targetDir}" (${entries.length} items):\n${lines.join('\n')}${extra}`
  } catch (err) {
    return `Error listing remote directory "${targetDir}": ${err instanceof Error ? err.message : String(err)}`
  }
}

/** Execute Remote SFTP Delete */
export async function executeRemoteFsDelete(
  ctx: PluginMainContext,
  targetPath: string
): Promise<string> {
  let sftpSession: SftpSession | null = null
  try {
    sftpSession = await ctx.openSftp()
  } catch (err) {
    return `Failed to open SFTP session on remote host: ${err instanceof Error ? err.message : String(err)}`
  }

  if (!sftpSession) {
    return 'Remote SFTP is not available on this session.'
  }

  try {
    await sftpSession.delete(targetPath)
    return `Successfully deleted remote path "${targetPath}".`
  } catch (err) {
    return `Error deleting remote path "${targetPath}": ${err instanceof Error ? err.message : String(err)}`
  }
}

/** Execute Local FS Read */
export async function executeLocalFsRead(
  filePath: string,
  maxChars?: number
): Promise<string> {
  const limit = Math.min(Math.max(Number(maxChars) || 32_000, 1_000), MAX_FILE_READ_CHARS)
  try {
    const resolvedPath = path.resolve(filePath)
    if (!fs.existsSync(resolvedPath)) {
      return `Error: Local file does not exist at "${resolvedPath}"`
    }
    const stat = await fs.promises.stat(resolvedPath)
    if (stat.isDirectory()) {
      return `Error: Local path "${resolvedPath}" is a directory, not a file. Use local_fs_list_dir instead.`
    }
    const content = await fs.promises.readFile(resolvedPath, 'utf-8')
    if (content.length > limit) {
      return `${content.slice(0, limit)}\n\n[Local file truncated at ${limit} characters of ${content.length}]`
    }
    return content || '(Empty local file)'
  } catch (err) {
    return `Error reading local file "${filePath}": ${err instanceof Error ? err.message : String(err)}`
  }
}

/** Execute Local FS Write */
export async function executeLocalFsWrite(
  filePath: string,
  content: string
): Promise<string> {
  try {
    const resolvedPath = path.resolve(filePath)
    const parentDir = path.dirname(resolvedPath)
    if (!fs.existsSync(parentDir)) {
      await fs.promises.mkdir(parentDir, { recursive: true })
    }
    await fs.promises.writeFile(resolvedPath, content, 'utf-8')
    return `Successfully wrote ${Buffer.byteLength(content, 'utf-8')} bytes to local file "${resolvedPath}".`
  } catch (err) {
    return `Error writing local file "${filePath}": ${err instanceof Error ? err.message : String(err)}`
  }
}

/** Execute Local FS List */
export async function executeLocalFsList(dirPath: string): Promise<string> {
  try {
    const resolvedPath = path.resolve(dirPath)
    if (!fs.existsSync(resolvedPath)) {
      return `Error: Local directory does not exist at "${resolvedPath}"`
    }
    const entries = await fs.promises.readdir(resolvedPath, { withFileTypes: true })
    if (entries.length === 0) {
      return `Local directory "${resolvedPath}" is empty.`
    }
    const lines = entries.slice(0, MAX_DIR_ENTRIES).map((entry) => {
      const isDir = entry.isDirectory() ? '/' : ''
      const typeLabel = entry.isDirectory() ? '<DIR>' : entry.isFile() ? 'FILE' : 'OTHER'
      return `${typeLabel.padEnd(6)} ${entry.name}${isDir}`
    })
    const extra = entries.length > MAX_DIR_ENTRIES ? `\n... (${entries.length - MAX_DIR_ENTRIES} more entries truncated)` : ''
    return `Local directory listing for "${resolvedPath}" (${entries.length} items):\n${lines.join('\n')}${extra}`
  } catch (err) {
    return `Error listing local directory "${dirPath}": ${err instanceof Error ? err.message : String(err)}`
  }
}
