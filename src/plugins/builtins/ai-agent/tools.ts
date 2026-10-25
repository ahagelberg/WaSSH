import fs from 'fs'
import path from 'path'
import { type AiAgentWebSearchProvider } from './protocol'
import type { PluginMainContext } from '@plugin-api/main'
import type { SftpSession } from '@plugin-api/main'

/** Max characters returned for web fetch or file read to prevent token overflow */
export const MAX_FETCH_CHARS = 32_000
export const MAX_FILE_READ_CHARS = 256_000
export const MAX_DIR_ENTRIES = 200
export const MAX_DEV_VIEW_LINES_DEFAULT = 300
export const MAX_DEV_GREP_MATCHES = 100
export const MAX_DEV_FIND_MATCHES = 100
export const MAX_DEV_TREE_DEPTH = 3
/** Default/limit unchanged context lines shown around each change in a diff */
export const DIFF_CONTEXT_LINES_DEFAULT = 3
export const DIFF_CONTEXT_LINES_MAX = 20
/** Cap on the LCS table size (lines x lines) for a diff */
export const MAX_DIFF_TABLE_CELLS = 4_000_000
/** SFTP operation timeout in milliseconds */
export const SFTP_TIMEOUT_MS = 30_000


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

/**
 * SFTP channel for the current AI agent run, keyed by main context.
 * `ctx.openSftp()` opens a NEW channel on every call, and a single agent run
 * makes many file calls (view/edit/grep/diff), so channels must be reused and
 * released - otherwise the SSH server's channel limit is hit and every
 * subsequent open fails with "Channel open failure: open failed".
 */
const sftpByContext = new WeakMap<PluginMainContext, Promise<SftpSession>>()

/**
 * SFTP session shared by every file tool in one agent run. The promise is
 * cached (not just the result) so concurrent tool calls await one channel.
 */
function getSftpSession(ctx: PluginMainContext): Promise<SftpSession> {
  let pending = sftpByContext.get(ctx)
  if (!pending) {
    pending = ctx.openSftp()
    sftpByContext.set(ctx, pending)
    pending.catch(() => {
      // Don't cache a failed open - the next call should retry.
      if (sftpByContext.get(ctx) === pending) {
        sftpByContext.delete(ctx)
      }
    })
  }
  return pending
}

/** Release the cached SFTP channel for a finished run. */
export function closeSftpSession(ctx: PluginMainContext): void {
  const pending = sftpByContext.get(ctx)
  if (!pending) {
    return
  }
  sftpByContext.delete(ctx)
  void pending
    .then((sftp) => sftp.end())
    .catch(() => {
      /* channel never opened */
    })
}

/** Execute Remote SFTP Read */
export async function executeRemoteFsRead(
  ctx: PluginMainContext,
  filePath: string,
  maxChars?: number,
  /** When false, read the whole file without truncating (used by edit paths). */
  truncate = true
): Promise<string> {
  const requested = Math.max(Number(maxChars) || 32_000, 1_000)
  const limit = truncate ? Math.min(requested, MAX_FILE_READ_CHARS) : Number.POSITIVE_INFINITY
  let sftpSession: SftpSession | null = null
  try {
    sftpSession = await getSftpSession(ctx)
  } catch (err) {
    return `Failed to open SFTP session on remote host: ${err instanceof Error ? err.message : String(err)}`
  }

  if (!sftpSession) {
    return 'Remote SFTP is not available on this session.'
  }

  return new Promise<string>((resolve) => {
    let settled = false
    let content = ''
    let truncated = false
    let timer: ReturnType<typeof setTimeout> | null = null

    const finish = (result: string): void => {
      if (settled) {
        return
      }
      settled = true
      if (timer) {
        clearTimeout(timer)
      }
      resolve(result)
    }

    timer = setTimeout(() => {
      try {
        stream?.destroy()
      } catch {
        /* ignore */
      }
      finish(`Error reading remote file "${filePath}": Operation timed out after ${SFTP_TIMEOUT_MS / 1000}s`)
    }, SFTP_TIMEOUT_MS)

    let stream: import('ssh2').ReadStream | null = null
    try {
      stream = sftpSession.createReadStream(filePath)

      stream.on('data', (chunk: Buffer | string) => {
        content += chunk.toString('utf-8')
        if (content.length > limit) {
          truncated = true
          content = content.slice(0, limit)
          try {
            stream?.destroy()
          } catch {
            /* ignore */
          }
          finish(`${content}\n\n[Remote file truncated at ${limit} characters]`)
        }
      })

      stream.on('error', (err: Error) => {
        finish(`Error reading remote file "${filePath}": ${err.message}`)
      })

      stream.on('end', () => {
        if (truncated) {
          finish(`${content}\n\n[Remote file truncated at ${limit} characters]`)
        } else {
          finish(content || '(Empty file)')
        }
      })

      stream.on('close', () => {
        if (truncated) {
          finish(`${content}\n\n[Remote file truncated at ${limit} characters]`)
        } else {
          finish(content || '(Empty file)')
        }
      })
    } catch (err) {
      finish(`Error reading remote file "${filePath}": ${err instanceof Error ? err.message : String(err)}`)
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
    sftpSession = await getSftpSession(ctx)
  } catch (err) {
    return `Failed to open SFTP session on remote host: ${err instanceof Error ? err.message : String(err)}`
  }

  if (!sftpSession) {
    return 'Remote SFTP is not available on this session.'
  }

  return new Promise<string>((resolve) => {
    let settled = false
    let timer: ReturnType<typeof setTimeout> | null = null

    const finish = (result: string): void => {
      if (settled) {
        return
      }
      settled = true
      if (timer) {
        clearTimeout(timer)
      }
      resolve(result)
    }

    timer = setTimeout(() => {
      try {
        stream?.destroy()
      } catch {
        /* ignore */
      }
      finish(`Error writing remote file "${filePath}": Operation timed out after ${SFTP_TIMEOUT_MS / 1000}s`)
    }, SFTP_TIMEOUT_MS)

    let stream: import('ssh2').WriteStream | null = null
    try {
      stream = sftpSession.createWriteStream(filePath)
      stream.on('error', (err: Error) => {
        finish(`Error writing remote file "${filePath}": ${err.message}`)
      })
      stream.on('finish', () => {
        finish(`Successfully wrote ${Buffer.byteLength(content, 'utf-8')} bytes to remote file "${filePath}".`)
      })
      stream.on('close', () => {
        finish(`Successfully wrote ${Buffer.byteLength(content, 'utf-8')} bytes to remote file "${filePath}".`)
      })
      stream.end(content, 'utf-8')
    } catch (err) {
      finish(`Error writing remote file "${filePath}": ${err instanceof Error ? err.message : String(err)}`)
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
    sftpSession = await getSftpSession(ctx)
  } catch (err) {
    return `Failed to open SFTP session on remote host: ${err instanceof Error ? err.message : String(err)}`
  }

  if (!sftpSession) {
    return 'Remote SFTP is not available on this session.'
  }

  try {
    let timer: ReturnType<typeof setTimeout> | null = null
    const timeoutPromise = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`Timed out listing remote directory "${targetDir}" after ${SFTP_TIMEOUT_MS / 1000}s`)), SFTP_TIMEOUT_MS)
    })
    const entries = await Promise.race([sftpSession.list(targetDir), timeoutPromise]).finally(() => {
      if (timer) {
        clearTimeout(timer)
      }
    })
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
    sftpSession = await getSftpSession(ctx)
  } catch (err) {
    return `Failed to open SFTP session on remote host: ${err instanceof Error ? err.message : String(err)}`
  }

  if (!sftpSession) {
    return 'Remote SFTP is not available on this session.'
  }

  try {
    let timer: ReturnType<typeof setTimeout> | null = null
    const timeoutPromise = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`Timed out deleting remote path "${targetPath}" after ${SFTP_TIMEOUT_MS / 1000}s`)), SFTP_TIMEOUT_MS)
    })
    await Promise.race([sftpSession.delete(targetPath), timeoutPromise]).finally(() => {
      if (timer) {
        clearTimeout(timer)
      }
    })
    return `Successfully deleted remote path "${targetPath}".`
  } catch (err) {
    return `Error deleting remote path "${targetPath}": ${err instanceof Error ? err.message : String(err)}`
  }
}

/** Apply a unique old-text -> new-text replacement, or return an error string on failure */
function applyTextReplacement(content: string, oldText: string, newText: string): string | { error: string } {
  const firstIndex = content.indexOf(oldText)
  if (firstIndex === -1) {
    return { error: 'oldText was not found in the file. Ensure it matches the file content exactly, including whitespace.' }
  }
  const lastIndex = content.lastIndexOf(oldText)
  if (firstIndex !== lastIndex) {
    return { error: 'oldText matches multiple locations in the file. Include more surrounding context to make it unique.' }
  }
  return content.slice(0, firstIndex) + newText + content.slice(firstIndex + oldText.length)
}

/** Execute Remote SFTP Edit (exact text block replace) */
export async function executeRemoteFsEdit(
  ctx: PluginMainContext,
  filePath: string,
  oldText: string,
  newText: string
): Promise<string> {
  const current = await executeRemoteFsRead(ctx, filePath, undefined, false)
  if (current.startsWith('Error') || current.startsWith('Failed to open SFTP') || current === 'Remote SFTP is not available on this session.') {
    return current
  }
  if (current.length > MAX_FILE_READ_CHARS) {
    return (
      `Error: remote file "${filePath}" is larger than the ${MAX_FILE_READ_CHARS}-character edit limit, ` +
      'so it cannot be edited safely. Edit it in smaller pieces or use a shell command instead.'
    )
  }
  const result = applyTextReplacement(current, oldText, newText)
  if (typeof result !== 'string') {
    return `Error editing remote file "${filePath}": ${result.error}`
  }
  return executeRemoteFsWrite(ctx, filePath, result).then((writeResult) =>
    writeResult.startsWith('Successfully') ? `Successfully edited remote file "${filePath}".` : writeResult
  )
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

/** Execute Local FS Edit (exact text block replace) */
export async function executeLocalFsEdit(
  filePath: string,
  oldText: string,
  newText: string
): Promise<string> {
  try {
    const resolvedPath = path.resolve(filePath)
    if (!fs.existsSync(resolvedPath)) {
      return `Error: Local file does not exist at "${resolvedPath}"`
    }
    const content = await fs.promises.readFile(resolvedPath, 'utf-8')
    const result = applyTextReplacement(content, oldText, newText)
    if (typeof result !== 'string') {
      return `Error editing local file "${resolvedPath}": ${result.error}`
    }
    await fs.promises.writeFile(resolvedPath, result, 'utf-8')
    return `Successfully edited local file "${resolvedPath}".`
  } catch (err) {
    return `Error editing local file "${filePath}": ${err instanceof Error ? err.message : String(err)}`
  }
}

function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

export function resolveRemotePath(cwd: string, targetPath?: string): string {
  if (!targetPath || targetPath.trim() === '' || targetPath.trim() === '.') {
    return cwd || '/'
  }
  const trimmed = targetPath.trim()
  if (trimmed.startsWith('/')) {
    return path.posix.normalize(trimmed)
  }
  return path.posix.normalize(path.posix.join(cwd || '/', trimmed))
}

/** Execute Developer Tool: View file with line numbers and optional range */
export async function executeDevViewFile(
  ctx: PluginMainContext,
  cwd: string,
  filePath: string,
  startLine?: number,
  endLine?: number
): Promise<string> {
  const remotePath = resolveRemotePath(cwd, filePath)
  const raw = await executeRemoteFsRead(ctx, remotePath, MAX_FILE_READ_CHARS)
  if (
    raw.startsWith('Error') ||
    raw.startsWith('Failed to open SFTP') ||
    raw === 'Remote SFTP is not available on this session.'
  ) {
    return raw
  }
  if (raw === '(Empty file)') {
    return `File: ${remotePath} (empty file)`
  }

  const lines = raw.split(/\r?\n/)
  const totalLines = lines.length

  let s = 1
  let e = totalLines
  if (startLine !== undefined || endLine !== undefined) {
    s = Math.max(1, Math.min(startLine ?? 1, totalLines))
    e = Math.max(s, Math.min(endLine ?? totalLines, totalLines))
  } else if (totalLines > MAX_DEV_VIEW_LINES_DEFAULT) {
    e = MAX_DEV_VIEW_LINES_DEFAULT
  }

  const pad = String(e).length
  const formatted = lines
    .slice(s - 1, e)
    .map((line, idx) => `${String(s + idx).padStart(pad, ' ')} | ${line}`)
    .join('\n')

  const note =
    totalLines > e - s + 1
      ? ` (${totalLines} lines total, showing lines ${s}-${e})`
      : ` (${totalLines} lines)`
  return `File: ${remotePath}${note}\n\n${formatted}`
}

/** Execute Developer Tool: Edit file by replacing exact text block */
export async function executeDevEditFile(
  ctx: PluginMainContext,
  cwd: string,
  filePath: string,
  oldText: string,
  newText: string
): Promise<string> {
  const remotePath = resolveRemotePath(cwd, filePath)
  const current = await executeRemoteFsRead(ctx, remotePath, undefined, false)
  if (
    current.startsWith('Error') ||
    current.startsWith('Failed to open SFTP') ||
    current === 'Remote SFTP is not available on this session.'
  ) {
    return current
  }
  if (current.length > MAX_FILE_READ_CHARS) {
    return (
      `Error: remote file "${remotePath}" is larger than the ${MAX_FILE_READ_CHARS}-character edit limit, ` +
      'so it cannot be edited safely. Edit it in smaller pieces or use a shell command instead.'
    )
  }
  const result = applyTextReplacement(current, oldText, newText)
  if (typeof result !== 'string') {
    return `Error editing remote file "${remotePath}": ${result.error}`
  }
  return executeRemoteFsWrite(ctx, remotePath, result).then((writeResult) =>
    writeResult.startsWith('Successfully')
      ? `Successfully edited remote file "${remotePath}".`
      : writeResult
  )
}

/** Execute Developer Tool: Create a new file with text content */
export async function executeDevCreateFile(
  ctx: PluginMainContext,
  cwd: string,
  filePath: string,
  content: string,
  overwrite = false
): Promise<string> {
  const remotePath = resolveRemotePath(cwd, filePath)
  if (!overwrite) {
    try {
      const sftpSession = await getSftpSession(ctx)
      if (sftpSession) {
        let timer: ReturnType<typeof setTimeout> | null = null
        const statTimeoutPromise = new Promise<null>((resolve) => {
          timer = setTimeout(() => resolve(null), 5000)
        })
        const stats = await Promise.race([sftpSession.statSafe(remotePath), statTimeoutPromise]).finally(() => {
          if (timer) {
            clearTimeout(timer)
          }
        })
        if (stats) {
          return `Error: File "${remotePath}" already exists. Set overwrite: true if you want to replace it.`
        }
      }
    } catch {
      // Proceed if stat check cannot be performed
    }
  }
  return executeRemoteFsWrite(ctx, remotePath, content).then((writeResult) =>
    writeResult.startsWith('Successfully')
      ? `Successfully created remote file "${remotePath}" (${Buffer.byteLength(content, 'utf-8')} bytes).`
      : writeResult
  )
}

/** Execute Developer Tool: List directory with optional recursive tree */
export async function executeDevListDir(
  ctx: PluginMainContext,
  cwd: string,
  dirPath?: string,
  recursive = false
): Promise<string> {
  const remotePath = resolveRemotePath(cwd, dirPath)
  if (recursive && ctx.isSshSession()) {
    try {
      const cmd = `find ${shellQuote(remotePath)} -maxdepth ${MAX_DEV_TREE_DEPTH} -not -path '*/.*' -not -path '*/node_modules*' -not -path '*/dist*' -not -path '*/build*' 2>/dev/null | head -n ${MAX_DIR_ENTRIES}`
      const out = (await ctx.execCapture(cmd)).trim()
      if (out) {
        return `Directory tree for "${remotePath}" (depth up to ${MAX_DEV_TREE_DEPTH}):\n${out}`
      }
    } catch {
      // Fallback to non-recursive SFTP listing
    }
  }
  return executeRemoteFsList(ctx, remotePath)
}

/** Execute Developer Tool: Search file contents using grep */
export async function executeDevGrep(
  ctx: PluginMainContext,
  cwd: string,
  pattern: string,
  searchPath?: string,
  caseSensitive = false,
  glob?: string
): Promise<string> {
  if (!ctx.isSshSession()) {
    return 'Error: Grep search requires an active SSH session.'
  }
  const remotePath = resolveRemotePath(cwd, searchPath)
  const caseFlag = caseSensitive ? '' : '-i'
  const globFlag = glob && glob.trim() ? `--include=${shellQuote(glob.trim())}` : ''
  const cmd = `grep -rnI ${caseFlag} --exclude-dir={.git,node_modules,dist,build,.cache,.next} ${globFlag} -e ${shellQuote(pattern)} ${shellQuote(remotePath)} 2>/dev/null | head -n ${MAX_DEV_GREP_MATCHES}`
  try {
    const raw = (await ctx.execCapture(cmd)).trim()
    if (!raw) {
      return `No matches found for "${pattern}" in "${remotePath}".`
    }
    const lineCount = raw.split('\n').length
    const truncatedNote =
      lineCount >= MAX_DEV_GREP_MATCHES ? `\n[Showing first ${MAX_DEV_GREP_MATCHES} matches]` : ''
    return `Grep matches for "${pattern}" in "${remotePath}":\n\n${raw}${truncatedNote}`
  } catch (err) {
    return `Error running grep in "${remotePath}": ${err instanceof Error ? err.message : String(err)}`
  }
}

/** Execute Developer Tool: Find files by name/pattern */
export async function executeDevFindFiles(
  ctx: PluginMainContext,
  cwd: string,
  pattern: string,
  searchPath?: string
): Promise<string> {
  if (!ctx.isSshSession()) {
    return 'Error: Finding files requires an active SSH session.'
  }
  const remotePath = resolveRemotePath(cwd, searchPath)
  const maxSearchDepth = 8
  const cmd = `find ${shellQuote(remotePath)} -maxdepth ${maxSearchDepth} -not -path '*/.*' -not -path '*/node_modules/*' -not -path '*/dist/*' -not -path '*/build/*' -iname ${shellQuote(pattern)} 2>/dev/null | head -n ${MAX_DEV_FIND_MATCHES}`
  try {
    const raw = (await ctx.execCapture(cmd)).trim()
    if (!raw) {
      return `No files found matching "${pattern}" in "${remotePath}".`
    }
    const matchCount = raw.split('\n').length
    const truncatedNote =
      matchCount >= MAX_DEV_FIND_MATCHES ? `\n[Showing first ${MAX_DEV_FIND_MATCHES} matches]` : ''
    return `Files matching "${pattern}" in "${remotePath}":\n\n${raw}${truncatedNote}`
  } catch (err) {
    return `Error finding files in "${remotePath}": ${err instanceof Error ? err.message : String(err)}`
  }
}

type DiffOp = 'keep' | 'add' | 'remove'

interface DiffLine {
  op: DiffOp
  text: string
}

/**
 * Line diff via longest-common-subsequence. Only used for files small enough
 * that the O(n*m) table is bounded by `MAX_DIFF_TABLE_CELLS`.
 */
function diffLines(before: string[], after: string[]): DiffLine[] {
  const n = before.length
  const m = after.length
  const table = new Uint32Array((n + 1) * (m + 1))
  const at = (i: number, j: number): number => i * (m + 1) + j
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      table[at(i, j)] =
        before[i] === after[j]
          ? table[at(i + 1, j + 1)] + 1
          : Math.max(table[at(i + 1, j)], table[at(i, j + 1)])
    }
  }
  const out: DiffLine[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (before[i] === after[j]) {
      out.push({ op: 'keep', text: before[i] })
      i += 1
      j += 1
    } else if (table[at(i + 1, j)] >= table[at(i, j + 1)]) {
      out.push({ op: 'remove', text: before[i] })
      i += 1
    } else {
      out.push({ op: 'add', text: after[j] })
      j += 1
    }
  }
  while (i < n) {
    out.push({ op: 'remove', text: before[i] })
    i += 1
  }
  while (j < m) {
    out.push({ op: 'add', text: after[j] })
    j += 1
  }
  return out
}

/**
 * Render a unified-style diff. Line numbers are tracked per side so an
 * unchanged line keeps its own number on both the "expected" and "actual"
 * side even when the two sides are misaligned.
 */
function formatUnifiedDiff(
  label: string,
  before: string[],
  after: string[],
  contextLines: number
): { text: string; added: number; removed: number } {
  const ops = diffLines(before, after)
  const added = ops.filter((o) => o.op === 'add').length
  const removed = ops.filter((o) => o.op === 'remove').length
  if (added === 0 && removed === 0) {
    return { text: '', added: 0, removed: 0 }
  }

  const keepBefore: number[] = []
  const keepAfter: number[] = []
  let beforeNo = 1
  let afterNo = 1
  for (const op of ops) {
    keepBefore.push(beforeNo)
    keepAfter.push(afterNo)
    if (op.op !== 'add') {
      beforeNo += 1
    }
    if (op.op !== 'remove') {
      afterNo += 1
    }
  }

  const changed = ops.map((o) => o.op !== 'keep')
  const lines: string[] = []
  let index = 0
  while (index < ops.length) {
    if (!changed[index]) {
      index += 1
      continue
    }
    let start = index
    for (let back = 0; back < contextLines && start > 0 && !changed[start - 1]; back += 1) {
      start -= 1
    }
    let end = index
    while (end < ops.length) {
      if (changed[end]) {
        end += 1
        continue
      }
      let gap = 0
      while (end + gap < ops.length && !changed[end + gap]) {
        gap += 1
      }
      if (gap > contextLines * 2 || end + gap >= ops.length) {
        break
      }
      end += gap
    }
    const tail = Math.min(contextLines, ops.length - end)
    const stop = end + tail

    const beforeStart = keepBefore[start]
    const afterStart = keepAfter[start]
    const beforeCount = ops.slice(start, stop).filter((o) => o.op !== 'add').length
    const afterCount = ops.slice(start, stop).filter((o) => o.op !== 'remove').length
    lines.push(
      `@@ -${beforeStart},${beforeCount} +${afterStart},${afterCount} @@`
    )
    for (let k = start; k < stop; k += 1) {
      const op = ops[k]
      if (op.op === 'keep') {
        lines.push(`  ${op.text}`)
      } else if (op.op === 'remove') {
        lines.push(`- ${op.text}`)
      } else {
        lines.push(`+ ${op.text}`)
      }
    }
    index = stop
  }
  return { text: `${label}\n${lines.join('\n')}`, added, removed }
}

function summarizeDiff(added: number, removed: number, kept: number): string {
  if (added === 0 && removed === 0) {
    return `Identical: ${kept} line${kept === 1 ? '' : 's'} match, no differences.`
  }
  return `Diff summary: +${added} added, -${removed} removed, ${kept} unchanged.`
}

/** One `@@ -a,b +c,d @@` hunk header from a unified diff. */
interface PatchHunk {
  oldStart: number
  oldCount: number
  newStart: number
  newCount: number
  /** Body lines including their leading ' ', '-' or '+' marker. */
  body: string[]
}

interface ParsedPatch {
  oldPath: string
  newPath: string
  hunks: PatchHunk[]
}

const HUNK_HEADER_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/

/** Strip a trailing tab-separated timestamp and a leading `a/` or `b/` prefix. */
function normalizePatchPath(raw: string): string {
  let value = raw.split('\t')[0].trim()
  if (value === '/dev/null') {
    return value
  }
  const quoted = /^"(.*)"$/.exec(value)
  if (quoted) {
    value = quoted[1]
  }
  return value.replace(/^[ab]\//, '')
}

/**
 * Parse a unified diff (`git diff` / `diff -u` / `git format-patch` output).
 * File headers are optional; `---`/`+++` pairs and `@@` hunks are what matter.
 * Returns an error string when the text is not a unified diff.
 */
function parseUnifiedPatch(patch: string): ParsedPatch | { error: string } {
  const lines = patch.replace(/\r\n/g, '\n').split('\n')
  const hunks: PatchHunk[] = []
  let oldPath = ''
  let newPath = ''
  let index = 0

  while (index < lines.length) {
    const line = lines[index]
    if (line.startsWith('diff --git ') || line.startsWith('index ') || line.startsWith('new file') ||
        line.startsWith('deleted file') || line.startsWith('similarity index') ||
        line.startsWith('rename ') || line.startsWith('old mode') || line.startsWith('new mode')) {
      index += 1
      continue
    }
    if (line.startsWith('--- ')) {
      oldPath = normalizePatchPath(line.slice(4))
      index += 1
      if (index < lines.length && lines[index].startsWith('+++ ')) {
        newPath = normalizePatchPath(lines[index].slice(4))
        index += 1
      }
      continue
    }
    if (line.startsWith('+++ ')) {
      newPath = normalizePatchPath(line.slice(4))
      index += 1
      continue
    }
    const header = HUNK_HEADER_RE.exec(line)
    if (!header) {
      index += 1
      continue
    }
    const hunk: PatchHunk = {
      oldStart: Number(header[1]),
      oldCount: header[2] === undefined ? 1 : Number(header[2]),
      newStart: Number(header[3]),
      newCount: header[4] === undefined ? 1 : Number(header[4]),
      body: []
    }
    index += 1
    // A zero-length side means the hunk starts at the line *before* the change.
    let oldSeen = 0
    let newSeen = 0
    while (index < lines.length && (oldSeen < hunk.oldCount || newSeen < hunk.newCount)) {
      const bodyLine = lines[index]
      if (bodyLine.startsWith('\\')) {
        // "\ No newline at end of file" annotates the previous line.
        index += 1
        continue
      }
      const marker = bodyLine.charAt(0)
      if (marker !== ' ' && marker !== '-' && marker !== '+') {
        break
      }
      if (bodyLine.length === 0 && hunk.oldCount === 0 && hunk.newCount === 0) {
        break
      }
      hunk.body.push(bodyLine)
      if (marker !== '+') {
        oldSeen += 1
      }
      if (marker !== '-') {
        newSeen += 1
      }
      index += 1
    }
    if (oldSeen !== hunk.oldCount || newSeen !== hunk.newCount) {
      return {
        error:
          `Hunk @@ -${hunk.oldStart},${hunk.oldCount} +${hunk.newStart},${hunk.newCount} @@ is truncated: ` +
          `expected ${hunk.oldCount} old / ${hunk.newCount} new lines, found ${oldSeen} / ${newSeen}.`
      }
    }
    hunks.push(hunk)
  }

  if (hunks.length === 0) {
    return { error: 'No unified diff hunks found. Expected at least one "@@ -old,count +new,count @@" header.' }
  }
  return { oldPath, newPath, hunks }
}

/** Lines a hunk removes (old side), used to locate it when the header offset drifted. */
function hunkOldLines(hunk: PatchHunk): string[] {
  const out: string[] = []
  for (const bodyLine of hunk.body) {
    const marker = bodyLine.charAt(0)
    if (marker === ' ' || marker === '-') {
      out.push(bodyLine.slice(1))
    }
  }
  return out
}

/** Lines a hunk produces (new side). */
function hunkNewLines(hunk: PatchHunk): string[] {
  const out: string[] = []
  for (const bodyLine of hunk.body) {
    const marker = bodyLine.charAt(0)
    if (marker === ' ' || marker === '+') {
      out.push(bodyLine.slice(1))
    }
  }
  return out
}

/**
 * Locate a hunk in `fileLines`. Prefers the header's line number and falls
 * back to a unique content search, so patches still apply when the file has
 * drifted by unrelated edits outside the hunk.
 */
function locateHunk(
  fileLines: string[],
  hunk: PatchHunk,
  claimedStart: number
): { at: number; fuzz: 'exact' | 'offset' | 'search' } | { error: string } {
  const oldLines = hunkOldLines(hunk)
  if (oldLines.length === 0) {
    // Pure insertion: anchor at the claimed position.
    return { at: Math.max(0, Math.min(claimedStart, fileLines.length)), fuzz: 'exact' }
  }
  const matchesAt = (start: number): boolean => {
    if (start < 0 || start + oldLines.length > fileLines.length) {
      return false
    }
    for (let k = 0; k < oldLines.length; k += 1) {
      if (fileLines[start + k] !== oldLines[k]) {
        return false
      }
    }
    return true
  }
  if (matchesAt(claimedStart)) {
    return { at: claimedStart, fuzz: 'exact' }
  }
  const found: number[] = []
  for (let start = 0; start + oldLines.length <= fileLines.length; start += 1) {
    if (matchesAt(start)) {
      found.push(start)
      if (found.length > 1) {
        break
      }
    }
  }  if (found.length === 1) {
    return { at: found[0], fuzz: 'search' }
  }
  if (found.length > 1) {
    return {
      error:
        `Hunk @@ -${hunk.oldStart},${hunk.oldCount} @@ no longer matches at line ${claimedStart + 1} and its content ` +
        'appears in multiple places. Re-read the file and regenerate the patch with more context.'
    }
  }
  return {
    error:
      `Hunk @@ -${hunk.oldStart},${hunk.oldCount} @@ does not match the file at line ${claimedStart + 1} ` +
      'or anywhere else. Re-read the file and regenerate the patch.'
  }
}

/**
 * Apply parsed hunks to file content. Hunks are applied bottom-up so earlier
 * line numbers stay valid, and each is located independently.
 */
function applyPatchHunks(
  content: string,
  hunks: PatchHunk[]
): { text: string; applied: number } | { error: string } {
  const fileLines = content.split('\n')
  const ordered = hunks
    .map((hunk, position) => ({ hunk, position }))
    .sort((a, b) => b.hunk.oldStart - a.hunk.oldStart)

  let lines = fileLines
  for (const { hunk, position } of ordered) {
    // For a zero-count old side the header points at the line before the insert.
    const claimedStart = hunk.oldCount === 0 ? hunk.oldStart : hunk.oldStart - 1
    const located = locateHunk(lines, hunk, claimedStart)
    if ('error' in located) {
      return { error: `Hunk #${position + 1}: ${located.error}` }
    }
    lines = [
      ...lines.slice(0, located.at),
      ...hunkNewLines(hunk),
      ...lines.slice(located.at + hunkOldLines(hunk).length)
    ]
  }
  return { text: lines.join('\n'), applied: hunks.length }
}

/**
 * Execute Developer Tool: diff a remote file against expected text, optionally
 * applying either a text replacement or a unified patch. Read-only unless
 * `apply` is true.
 */
export async function executeDevDiffFile(
  ctx: PluginMainContext,
  cwd: string,
  filePath: string,
  oldText: string,
  newText: string,
  apply: boolean,
  contextLines?: number,
  patch?: string
): Promise<string> {
  const remotePath = resolveRemotePath(cwd, filePath)
  const context = Math.min(Math.max(Number(contextLines) || DIFF_CONTEXT_LINES_DEFAULT, 0), DIFF_CONTEXT_LINES_MAX)
  const current = await executeRemoteFsRead(ctx, remotePath, MAX_FILE_READ_CHARS)
  if (
    current.startsWith('Error') ||
    current.startsWith('Failed to open SFTP') ||
    current === 'Remote SFTP is not available on this session.'
  ) {
    return current
  }
  const truncated = /\n\n\[Remote file truncated at \d+ characters\]$/.test(current)
  const content = truncated ? current.replace(/\n\n\[Remote file truncated at \d+ characters\]$/, '') : current
  const actual = content === '(Empty file)' ? '' : content

  if (patch !== undefined && patch.trim() !== '') {
    const parsed = parseUnifiedPatch(patch)
    if ('error' in parsed) {
      return `Error: invalid unified patch for "${remotePath}": ${parsed.error}`
    }
    if (truncated) {
      return (
        `Error: "${remotePath}" is larger than the ${MAX_FILE_READ_CHARS}-character read limit, so a patch cannot be ` +
        'applied safely. Use dev_edit_file with a unique oldText block instead.'
      )
    }
    const result = applyPatchHunks(actual, parsed.hunks)
    if ('error' in result) {
      return `Error: patch does not apply to "${remotePath}": ${result.error}`
    }
    if (!apply) {
      const preview = formatUnifiedDiff(
        `Patch preview for ${remotePath}:`,
        actual.split('\n'),
        result.text.split('\n'),
        context
      )
      const summary = summarizeDiff(
        preview.added,
        preview.removed,
        actual.split('\n').length - preview.removed
      )
      return (
        `${result.applied} hunk${result.applied === 1 ? '' : 's'} would apply cleanly to ${remotePath}.\n\n` +
        `${preview.text}\n\n${summary}\n[Preview only - pass apply: true to write the change.]`
      )
    }
    const writeResult = await executeRemoteFsWrite(ctx, remotePath, result.text)
    if (!writeResult.startsWith('Successfully')) {
      return writeResult
    }
    const applied = formatUnifiedDiff(
      `Applied to ${remotePath}:`,
      actual.split('\n'),
      result.text.split('\n'),
      context
    )
    return (
      `${writeResult}\nApplied ${result.applied} hunk${result.applied === 1 ? '' : 's'}.\n` +
      `${applied.text}\n${summarizeDiff(
        applied.added,
        applied.removed,
        actual.split('\n').length - applied.removed
      )}`
    )
  }

  if (apply) {
    if (!newText) {
      return 'Error: newText is required when apply is true.'
    }
    const replaced = applyTextReplacement(actual, oldText, newText)
    if (typeof replaced !== 'string') {
      return `Error applying diff to remote file "${remotePath}": ${replaced.error}`
    }
    const writeResult = await executeRemoteFsWrite(ctx, remotePath, replaced)
    if (!writeResult.startsWith('Successfully')) {
      return writeResult
    }
    const applied = formatUnifiedDiff(
      `Applied to ${remotePath}:`,
      actual.split('\n'),
      replaced.split('\n'),
      context
    )
    return `${writeResult}\n${applied.text}\n${summarizeDiff(
      applied.added,
      applied.removed,
      actual.split('\n').length - applied.removed
    )}`
  }

  const expectedLines = oldText === '' ? [] : oldText.split('\n')
  const actualLines = actual === '' ? [] : actual.split('\n')
  if (expectedLines.length * actualLines.length > MAX_DIFF_TABLE_CELLS) {
    return (
      `Error: "${remotePath}" is too large to diff (${actualLines.length} lines vs ${expectedLines.length} expected lines). ` +
      'Narrow oldText to the region you care about, or use dev_view_file with a line range instead.'
    )
  }
  const diff = formatUnifiedDiff(
    `Diff of ${remotePath} (expected \u2192 actual):`,
    expectedLines,
    actualLines,
    context
  )
  const summary = summarizeDiff(diff.added, diff.removed, expectedLines.length - diff.removed)
  if (!diff.text) {
    return `${remotePath}: ${summary}${
      truncated ? `\n[Note: file was truncated at ${MAX_FILE_READ_CHARS} characters before diffing]` : ''
    }`
  }
  return `${diff.text}\n\n${summary}${
    truncated ? `\n[Note: file was truncated at ${MAX_FILE_READ_CHARS} characters before diffing]` : ''
  }`
}

/* ------------------------------------------------------------------------- *
 * System inspection tools (CPU, memory, disk, processes)
 *
 * These are read-only and remote-only: they parse /proc and POSIX tooling on
 * the SSH host, returning structured JSON instead of locale-dependent text so
 * the model never has to assemble or interpret df/free/ps output itself.
 * ------------------------------------------------------------------------- */

/** Default and maximum rows returned by `system_process_list`. */
export const MAX_SYSTEM_PROCESS_ROWS_DEFAULT = 15
export const MAX_SYSTEM_PROCESS_ROWS_LIMIT = 50
/** Default and maximum top consumers included in `system_memory_report`. */
export const MAX_MEMORY_CONSUMERS_DEFAULT = 5
export const MAX_MEMORY_CONSUMERS_LIMIT = 15
/** Command text kept per process row. */
const MAX_PROCESS_COMMAND_CHARS = 200
/** Filesystems listed in the overview; the rest are summarised in a count. */
const MAX_OVERVIEW_FILESYSTEMS = 10
/** Window used for the two-sample CPU reading in the overview (seconds). */
const CPU_SAMPLE_SECONDS = 0.4

export type SystemProcessSort = 'cpu' | 'mem' | 'pid' | 'time'

/** Filesystem types that never represent user data worth reporting. */
const PSEUDO_FS_TYPES = new Set([
  'autofs',
  'binfmt_misc',
  'bpf',
  'cgroup',
  'cgroup2',
  'configfs',
  'debugfs',
  'devpts',
  'devtmpfs',
  'efivarfs',
  'fusectl',
  'hugetlbfs',
  'mqueue',
  'nsfs',
  'proc',
  'pstore',
  'ramfs',
  'rpc_pipefs',
  'securityfs',
  'sysfs',
  'tmpfs',
  'tracefs'
])

/** Fallback pseudo-mount detection when `df -T` is unavailable. */
const PSEUDO_MOUNT_PREFIXES = ['/proc', '/sys', '/dev', '/run']

interface CpuTotals {
  total: number
  idle: number
}

interface DiskEntry {
  filesystem: string
  type: string | null
  mount: string
  sizeBytes: number
  usedBytes: number
  availableBytes: number
  usedPercent: number
  inodes: { total: number; used: number; free: number; usedPercent: number } | null
}

interface ProcessRow {
  pid: number
  user: string
  state: string | null
  nice: number | null
  threads: number | null
  cpuPercent: number | null
  memPercent: number | null
  rssBytes: number
  elapsed: string | null
  cpuTimeSeconds: number | null
  command: string
}

/** Run a read-only stats script on the connected SSH host. */
async function execStats(ctx: PluginMainContext, script: string): Promise<string> {
  if (!ctx.isSshSession()) {
    throw new Error('System inspection requires an active SSH session')
  }
  return ctx.execCapture(script)
}

/** Split `@@SECTION@@`-delimited script output into named blocks. */
function statSections(output: string): Record<string, string> {
  const sections: Record<string, string> = {}
  let current: string | null = null
  for (const rawLine of output.split('\n')) {
    const line = rawLine.replace(/\r$/, '')
    const marker = /^@@([A-Z0-9_]+)@@$/.exec(line.trim())
    if (marker) {
      current = marker[1]
      sections[current] = ''
      continue
    }
    if (current !== null) {
      sections[current] += `${line}\n`
    }
  }
  return sections
}

/**
 * Split a whitespace-aligned table row into `prefixCount` single-token
 * columns plus one remainder column (the command or mount point).
 */
function splitColumns(line: string, prefixCount: number): string[] | null {
  const parts: string[] = []
  let rest = line.trim()
  for (let i = 0; i < prefixCount; i += 1) {
    const match = /^(\S+)\s+(.*)$/.exec(rest)
    if (!match) {
      return null
    }
    parts.push(match[1])
    rest = match[2]
  }
  parts.push(rest)
  return parts
}

function toNumber(value: string | undefined): number | null {
  if (value === undefined) {
    return null
  }
  const parsed = Number(value.replace(/,/g, ''))
  return Number.isFinite(parsed) ? parsed : null
}

function toPercent(value: string | undefined): number | null {
  if (!value) {
    return null
  }
  const parsed = Number(value.replace('%', ''))
  return Number.isFinite(parsed) ? parsed : null
}

function round1(value: number): number {
  return Math.round(value * 10) / 10
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max)
}

function parseCpuLine(line: string | undefined): CpuTotals | null {
  if (!line) {
    return null
  }
  const parts = line.trim().split(/\s+/)
  if (parts[0] !== 'cpu') {
    return null
  }
  // Only the first eight counters count real time: guest time is already
  // included in user/nice, so adding it would inflate the total.
  const counters = parts.slice(1, 9).map(Number)
  if (counters.length < 4 || counters.some((n) => !Number.isFinite(n))) {
    return null
  }
  // idle + iowait are the only non-busy counters
  const idle = (counters[3] ?? 0) + (counters[4] ?? 0)
  const total = counters.reduce((sum, n) => sum + n, 0)
  return { total, idle }
}

function cpuUsedPercent(first: CpuTotals | null, second: CpuTotals | null): number | null {
  if (!first || !second) {
    return null
  }
  const totalDelta = second.total - first.total
  const idleDelta = second.idle - first.idle
  if (totalDelta <= 0) {
    return null
  }
  return round1(clamp((1 - idleDelta / totalDelta) * 100, 0, 100))
}

function parseLoadavg(section: string | undefined): {
  load1: number
  load5: number
  load15: number
  runnable: number | null
  totalThreads: number | null
} | null {
  const parts = (section ?? '').trim().split(/\s+/)
  if (parts.length < 3) {
    return null
  }
  const [load1, load5, load15] = parts.slice(0, 3).map(Number)
  if (![load1, load5, load15].every((n) => Number.isFinite(n))) {
    return null
  }
  let runnable: number | null = null
  let totalThreads: number | null = null
  const counts = parts[3]?.split('/')
  if (counts && counts.length === 2) {
    runnable = toNumber(counts[0])
    totalThreads = toNumber(counts[1])
  }
  return { load1, load5, load15, runnable, totalThreads }
}

function meminfoValues(section: string | undefined): Map<string, number> {
  const values = new Map<string, number>()
  for (const line of (section ?? '').split('\n')) {
    const match = /^([A-Za-z_()0-9]+):\s+(\d+)/.exec(line.trim())
    if (match) {
      values.set(match[1], Number(match[2]))
    }
  }
  return values
}

/**
 * Parse one `df` layout. `columns` is the number of data columns, not header
 * tokens: the header ends with "Mounted on", which is two tokens for one
 * column, so counting header words overestimates by one.
 */
function parseDfSpaceLayout(lines: string[], columns: number): DiskEntry[] {
  const hasType = columns === 7
  const sizeIndex = hasType ? 2 : 1
  const mountIndex = columns - 1
  const entries: DiskEntry[] = []
  for (const line of lines.slice(1)) {
    const cols = splitColumns(line, columns - 1)
    if (!cols) {
      continue
    }
    const sizeKb = toNumber(cols[sizeIndex])
    const usedKb = toNumber(cols[sizeIndex + 1])
    const availableKb = toNumber(cols[sizeIndex + 2])
    if (sizeKb === null || usedKb === null || availableKb === null) {
      continue
    }
    entries.push({
      filesystem: cols[0],
      type: hasType ? cols[1] : null,
      mount: cols[mountIndex],
      sizeBytes: sizeKb * 1024,
      usedBytes: usedKb * 1024,
      availableBytes: availableKb * 1024,
      usedPercent:
        toPercent(cols[sizeIndex + 3]) ?? (sizeKb > 0 ? round1((usedKb / sizeKb) * 100) : 0),
      inodes: null
    })
  }
  return entries
}

function parseDfSpace(section: string | undefined): DiskEntry[] {
  const lines = (section ?? '')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
  if (lines.length < 2) {
    return []
  }
  const header = lines[0].split(/\s+/)
  const withType = header[1]?.toLowerCase() === 'type'
  // Try the header's layout first, then the other: `df -T` is not guaranteed
  // to be supported, so the header and rows can disagree after a fallback.
  for (const columns of withType ? [7, 6] : [6, 7]) {
    const entries = parseDfSpaceLayout(lines, columns)
    if (entries.length > 0) {
      return entries
    }
  }
  return []
}

function parseDfInodes(
  section: string | undefined
): Map<string, { total: number; used: number; free: number; usedPercent: number }> {
  const out = new Map<string, { total: number; used: number; free: number; usedPercent: number }>()
  const lines = (section ?? '')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
  if (lines.length < 2) {
    return out
  }
  // Filesystem, Inodes, IUsed, IFree, IUse%, Mounted on
  const columns = 6
  for (const line of lines.slice(1)) {
    const cols = splitColumns(line, columns - 1)
    if (!cols) {
      continue
    }
    const total = toNumber(cols[1])
    const used = toNumber(cols[2])
    const free = toNumber(cols[3])
    if (total === null || used === null || free === null) {
      continue
    }
    out.set(cols[columns - 1], {
      total,
      used,
      free,
      usedPercent: toPercent(cols[4]) ?? (total > 0 ? round1((used / total) * 100) : 0)
    })
  }
  return out
}

function isPseudoMount(entry: DiskEntry): boolean {
  const type = (entry.type ?? '').toLowerCase()
  if (type && PSEUDO_FS_TYPES.has(type)) {
    return true
  }
  if (!type) {
    return PSEUDO_MOUNT_PREFIXES.some(
      (prefix) => entry.mount === prefix || entry.mount.startsWith(`${prefix}/`)
    )
  }
  return false
}

/** Deduplicate by mount point and apply the default pseudo-filesystem filter. */
function normalizeDisks(
  entries: DiskEntry[],
  inodes: Map<string, { total: number; used: number; free: number; usedPercent: number }> | null,
  includeAllFilesystems: boolean
): DiskEntry[] {
  const byMount = new Map<string, DiskEntry>()
  for (const entry of entries) {
    byMount.set(entry.mount, entry)
  }
  const rows = [...byMount.values()].filter(
    (entry) => includeAllFilesystems || !isPseudoMount(entry)
  )
  if (inodes) {
    for (const row of rows) {
      row.inodes = inodes.get(row.mount) ?? null
    }
  }
  rows.sort((a, b) => b.usedPercent - a.usedPercent)
  return rows
}

function parseProcessTable(section: string | undefined): ProcessRow[] {
  const lines = (section ?? '')
    .split('\n')
    .map((line) => line.replace(/\s+$/, ''))
    .filter((line) => line.trim().length > 0)
  if (lines.length < 2) {
    return []
  }
  const header = lines[0].trim().split(/\s+/)
  const names = header.map((name) => name.toLowerCase().replace(/%/g, ''))
  const indexOf = (...candidates: string[]): number =>
    names.findIndex((name) => candidates.includes(name))
  const pidIndex = indexOf('pid')
  if (pidIndex === -1) {
    return []
  }
  const rssIndex = indexOf('rss')
  const cpuIndex = indexOf('cpu')
  const memIndex = indexOf('mem')
  const userIndex = indexOf('user', 'uid')
  const statIndex = indexOf('stat', 's')
  const niceIndex = indexOf('ni', 'nice')
  const threadsIndex = indexOf('nlwp', 'thr', 'threads')
  const elapsedIndex = indexOf('etime', 'elapsed')
  const timeIndex = indexOf('time', 'cputime')
  const commandIndex = header.length - 1

  const rows: ProcessRow[] = []
  for (const line of lines.slice(1)) {
    const cols = splitColumns(line, header.length - 1)
    if (!cols || cols.length <= commandIndex) {
      continue
    }
    const pid = toNumber(cols[pidIndex])
    if (pid === null) {
      continue
    }
    const rssKb = rssIndex >= 0 ? toNumber(cols[rssIndex]) : null
    rows.push({
      pid,
      user: userIndex >= 0 ? cols[userIndex] : '',
      state: statIndex >= 0 ? cols[statIndex] : null,
      nice: niceIndex >= 0 ? toNumber(cols[niceIndex]) : null,
      threads: threadsIndex >= 0 ? toNumber(cols[threadsIndex]) : null,
      cpuPercent: cpuIndex >= 0 ? toNumber(cols[cpuIndex]) : null,
      memPercent: memIndex >= 0 ? toNumber(cols[memIndex]) : null,
      rssBytes: Math.max(0, rssKb ?? 0) * 1024,
      elapsed: elapsedIndex >= 0 ? cols[elapsedIndex] : null,
      cpuTimeSeconds: timeIndex >= 0 ? parseCpuTime(cols[timeIndex]) : null,
      command: cols[commandIndex].slice(0, MAX_PROCESS_COMMAND_CHARS)
    })
  }
  return rows
}

function parseCpuTime(value: string): number | null {
  const match = /^(?:(\d+)-)?(?:(\d+):)?(\d+):(\d+)$/.exec(value.trim())
  if (!match) {
    return null
  }
  const days = Number(match[1] ?? 0)
  const hours = Number(match[2] ?? 0)
  const minutes = Number(match[3])
  const seconds = Number(match[4])
  return days * 86400 + hours * 3600 + minutes * 60 + seconds
}

/** Shape a process table the same way for `system_process_list` and memory consumers. */
function selectProcesses(
  rows: ProcessRow[],
  sort: SystemProcessSort,
  limit: number,
  user?: string,
  namePattern?: string
): ProcessRow[] {
  const needle = namePattern?.trim().toLowerCase()
  const filtered = rows.filter((row) => {
    if (user && row.user !== user) {
      return false
    }
    if (needle && !row.command.toLowerCase().includes(needle)) {
      return false
    }
    return true
  })
  filtered.sort((a, b) => {
    if (sort === 'pid') {
      return a.pid - b.pid
    }
    if (sort === 'mem') {
      return b.rssBytes - a.rssBytes
    }
    if (sort === 'time') {
      return (b.cpuTimeSeconds ?? 0) - (a.cpuTimeSeconds ?? 0)
    }
    return (b.cpuPercent ?? 0) - (a.cpuPercent ?? 0)
  })
  return filtered.slice(0, limit)
}

/** JSON error helper shared by the system tools. */
function systemError(tool: string, err: unknown): string {
  return `Error running ${tool}: ${err instanceof Error ? err.message : String(err)}`
}

/** Execute System Tool: one-call CPU / memory / disk / load baseline. */
export async function executeSystemOverview(ctx: PluginMainContext): Promise<string> {
  const script = [
    'COLUMNS=4096; export COLUMNS',
    "echo '@@UNAME@@'",
    'uname -srm 2>/dev/null || true',
    "echo '@@HOSTNAME@@'",
    'uname -n 2>/dev/null || cat /proc/sys/kernel/hostname 2>/dev/null || true',
    "echo '@@OSRELEASE@@'",
    "(grep -m1 '^PRETTY_NAME=' /etc/os-release 2>/dev/null || true)",
    "echo '@@UPTIME@@'",
    'cat /proc/uptime 2>/dev/null || true',
    "echo '@@LOADAVG@@'",
    'cat /proc/loadavg 2>/dev/null || true',
    "echo '@@NPROC@@'",
    "(nproc 2>/dev/null || grep -c '^processor' /proc/cpuinfo 2>/dev/null || echo 0)",
    "echo '@@CPU1@@'",
    "(grep -m1 '^cpu ' /proc/stat 2>/dev/null || true)",
    `sleep ${CPU_SAMPLE_SECONDS} 2>/dev/null || sleep 1`,
    "echo '@@CPU2@@'",
    "(grep -m1 '^cpu ' /proc/stat 2>/dev/null || true)",
    "echo '@@MEM@@'",
    "(grep -E '^(MemTotal|MemFree|MemAvailable|Buffers|Cached|SReclaimable|SwapTotal|SwapFree):' /proc/meminfo 2>/dev/null || true)",
    "echo '@@DF@@'",
    '((command -v timeout >/dev/null 2>&1 && timeout 5 df -kPT 2>/dev/null) || df -kPT 2>/dev/null || df -kP 2>/dev/null || true)',
    "echo '@@END@@'",
    'true'
  ].join('\n')

  let output: string
  try {
    output = await execStats(ctx, script)
  } catch (err) {
    return systemError('system_overview', err)
  }
  const sections = statSections(output)
  const cpu1 = parseCpuLine(sections.CPU1)
  if (!cpu1) {
    return 'Error: system_overview requires a Linux host (/proc/stat is unavailable).'
  }

  const unameParts = (sections.UNAME ?? '').trim().split(/\s+/).filter(Boolean)
  const os = unameParts[0] ?? null
  const arch = unameParts.length > 1 ? unameParts[unameParts.length - 1] : null
  const kernel = unameParts.length > 2 ? unameParts.slice(1, -1).join(' ') : null
  const release = (sections.OSRELEASE ?? '').trim()
  const prettyName = /^PRETTY_NAME="?([^"]*)"?$/.exec(release)?.[1] ?? release
  const uptimeSeconds = Number((sections.UPTIME ?? '').trim().split(/\s+/)[0] || 0)
  const logicalCores = toNumber((sections.NPROC ?? '').trim()) ?? 0
  const load = parseLoadavg(sections.LOADAVG)
  const mem = meminfoValues(sections.MEM)
  const memTotal = (mem.get('MemTotal') ?? 0) * 1024
  const memFree = (mem.get('MemFree') ?? 0) * 1024
  const memAvailable =
    (mem.get('MemAvailable') ??
      (mem.get('MemFree') ?? 0) +
        (mem.get('Buffers') ?? 0) +
        (mem.get('Cached') ?? 0) +
        (mem.get('SReclaimable') ?? 0)) * 1024
  const disks = normalizeDisks(parseDfSpace(sections.DF), null, false)

  const uniqueDevices = new Map<string, DiskEntry>()
  for (const disk of disks) {
    if (!uniqueDevices.has(disk.filesystem)) {
      uniqueDevices.set(disk.filesystem, disk)
    }
  }
  const diskTotal = [...uniqueDevices.values()].reduce((sum, d) => sum + d.sizeBytes, 0)
  const diskUsed = [...uniqueDevices.values()].reduce((sum, d) => sum + d.usedBytes, 0)

  const overview = {
    host: {
      hostname: (sections.HOSTNAME ?? '').trim() || null,
      os,
      distro: prettyName || null,
      kernel,
      arch
    },
    uptimeSeconds: Number.isFinite(uptimeSeconds) ? Math.round(uptimeSeconds) : null,
    load,
    cpu: {
      logicalCores,
      usedPercent: cpuUsedPercent(cpu1, parseCpuLine(sections.CPU2))
    },
    memory: {
      totalBytes: memTotal,
      availableBytes: memAvailable,
      usedBytes: Math.max(0, memTotal - memAvailable),
      freeBytes: memFree,
      buffersBytes: (mem.get('Buffers') ?? 0) * 1024,
      cachedBytes: ((mem.get('Cached') ?? 0) + (mem.get('SReclaimable') ?? 0)) * 1024,
      swapTotalBytes: (mem.get('SwapTotal') ?? 0) * 1024,
      swapUsedBytes: ((mem.get('SwapTotal') ?? 0) - (mem.get('SwapFree') ?? 0)) * 1024
    },
    disk: {
      totalBytes: diskTotal,
      usedBytes: diskUsed,
      usedPercent: diskTotal > 0 ? round1((diskUsed / diskTotal) * 100) : 0,
      filesystems: disks.slice(0, MAX_OVERVIEW_FILESYSTEMS),
      hiddenFilesystemCount: Math.max(0, disks.length - MAX_OVERVIEW_FILESYSTEMS)
    }
  }
  return JSON.stringify(overview, null, 2)
}

/** Execute System Tool: process list sorted by CPU, memory, PID, or CPU time. */
export async function executeSystemProcessList(
  ctx: PluginMainContext,
  sort: SystemProcessSort = 'cpu',
  limit?: number,
  user?: string,
  namePattern?: string
): Promise<string> {
  const rowLimit = clamp(Math.trunc(Number(limit) || MAX_SYSTEM_PROCESS_ROWS_DEFAULT), 1, MAX_SYSTEM_PROCESS_ROWS_LIMIT)
  const script = [
    'COLUMNS=4096; export COLUMNS',
    "echo '@@PS@@'",
    '(ps -ww -eo pid,user:32,stat,nice,nlwp,pcpu,pmem,rss,etime,time,args 2>/dev/null || ps -ww -eo pid,user:32,stat,pcpu,pmem,rss,etime,time,args 2>/dev/null || ps aux 2>/dev/null || true)',
    "echo '@@NPROC@@'",
    "(nproc 2>/dev/null || grep -c '^processor' /proc/cpuinfo 2>/dev/null || echo 0)",
    "echo '@@LOADAVG@@'",
    'cat /proc/loadavg 2>/dev/null || true',
    "echo '@@END@@'",
    'true'
  ].join('\n')

  let output: string
  try {
    output = await execStats(ctx, script)
  } catch (err) {
    return systemError('system_process_list', err)
  }
  const sections = statSections(output)
  const rows = parseProcessTable(sections.PS)
  if (rows.length === 0) {
    return 'Error: could not parse process list output from the remote host.'
  }
  const selected = selectProcesses(rows, sort, rowLimit, user?.trim() || undefined, namePattern)
  return JSON.stringify(
    {
      sort,
      totalProcesses: rows.length,
      matchedProcesses: selected.length,
      logicalCores: toNumber((sections.NPROC ?? '').trim()) ?? 0,
      load: parseLoadavg(sections.LOADAVG),
      processes: selected,
      note:
        rows.length > selected.length
          ? `Showing ${selected.length} of ${rows.length} processes sorted by ${sort}.`
          : undefined
    },
    null,
    2
  )
}

/** Execute System Tool: per-filesystem space and inode usage. */
export async function executeSystemDiskUsage(
  ctx: PluginMainContext,
  cwd: string,
  path?: string,
  includeInodes = true,
  includeAllFilesystems = false
): Promise<string> {
  const target = path && path.trim() ? resolveRemotePath(cwd, path.trim()) : null
  const operand = target ? ` -- ${shellQuote(target)}` : ''
  const script = [
    'COLUMNS=4096; export COLUMNS',
    "echo '@@DFSPACE@@'",
    `((command -v timeout >/dev/null 2>&1 && timeout 5 df -kPT${operand} 2>/dev/null) || df -kPT${operand} 2>/dev/null || df -kP${operand} 2>/dev/null || true)`,
    "echo '@@DFINODE@@'",
    includeInodes
      ? `((command -v timeout >/dev/null 2>&1 && timeout 5 df -iP${operand} 2>/dev/null) || df -iP${operand} 2>/dev/null || true)`
      : '',
    "echo '@@END@@'",
    'true'
  ]
    .filter((line) => line.length > 0)
    .join('\n')

  let output: string
  try {
    output = await execStats(ctx, script)
  } catch (err) {
    return systemError('system_disk_usage', err)
  }
  const sections = statSections(output)
  const entries = parseDfSpace(sections.DFSPACE)
  if (entries.length === 0) {
    return target
      ? `No filesystem usage found for "${target}".`
      : 'Error: could not parse df output from the remote host.'
  }
  // An explicit path means the user asked for that filesystem specifically,
  // so keep pseudo filesystems rather than hiding the answer.
  const includeAll = includeAllFilesystems || target !== null
  const inodes = includeInodes ? parseDfInodes(sections.DFINODE) : null
  const disks = normalizeDisks(entries, inodes, includeAll)
  const totalBytes = disks.reduce((sum, d) => sum + d.sizeBytes, 0)
  const usedBytes = disks.reduce((sum, d) => sum + d.usedBytes, 0)
  const fullInodes = disks.filter((d) => (d.inodes?.usedPercent ?? 0) >= 90).map((d) => d.mount)
  return JSON.stringify(
    {
      path: target,
      includeAllFilesystems: includeAll,
      totalBytes,
      usedBytes,
      usedPercent: totalBytes > 0 ? round1((usedBytes / totalBytes) * 100) : 0,
      filesystems: disks,
      inodesNearlyFull: fullInodes
    },
    null,
    2
  )
}

/** Execute System Tool: memory breakdown, pressure, swap, OOM kills, top consumers. */
export async function executeSystemMemoryReport(
  ctx: PluginMainContext,
  topConsumers?: number
): Promise<string> {
  const consumerCount = clamp(
    Math.trunc(Number(topConsumers) || MAX_MEMORY_CONSUMERS_DEFAULT),
    1,
    MAX_MEMORY_CONSUMERS_LIMIT
  )
  const script = [
    'COLUMNS=4096; export COLUMNS',
    "echo '@@MEMINFO@@'",
    'cat /proc/meminfo 2>/dev/null || true',
    "echo '@@PRESSURE@@'",
    'cat /proc/pressure/memory 2>/dev/null || true',
    "echo '@@VMSTAT@@'",
    "grep -E '^(oom_kill|pgscan_direct|pgsteal_direct|pswpin|pswpout) ' /proc/vmstat 2>/dev/null || true",
    "echo '@@SWAP@@'",
    'cat /proc/swaps 2>/dev/null || true',
    "echo '@@PS@@'",
    '(ps -ww -eo pid,user:32,pcpu,pmem,rss,args 2>/dev/null || ps aux 2>/dev/null || true)',
    "echo '@@END@@'",
    'true'
  ].join('\n')

  let output: string
  try {
    output = await execStats(ctx, script)
  } catch (err) {
    return systemError('system_memory_report', err)
  }
  const sections = statSections(output)
  const mem = meminfoValues(sections.MEMINFO)
  if (mem.size === 0) {
    return 'Error: /proc/meminfo is unavailable on the remote host.'
  }
  const kib = (key: string): number => (mem.get(key) ?? 0) * 1024
  const total = kib('MemTotal')
  const free = kib('MemFree')
  const available = mem.has('MemAvailable')
    ? kib('MemAvailable')
    : free + kib('Buffers') + kib('Cached') + kib('SReclaimable')
  const used = Math.max(0, total - available)

  const pressure: Record<string, { avg10: number; avg60: number; avg300: number; totalMicroseconds: number }> = {}
  for (const line of (sections.PRESSURE ?? '').split('\n')) {
    const match = /^(some|full)\s+(.*)$/.exec(line.trim())
    if (!match) {
      continue
    }
    const values: Record<string, number> = {}
    for (const pair of match[2].split(/\s+/)) {
      const [key, raw] = pair.split('=')
      const number = Number(raw)
      if (key && Number.isFinite(number)) {
        values[key] = number
      }
    }
    pressure[match[1]] = {
      avg10: values.avg10 ?? 0,
      avg60: values.avg60 ?? 0,
      avg300: values.avg300 ?? 0,
      totalMicroseconds: values.total ?? 0
    }
  }

  const vmstat: Record<string, number> = {}
  for (const line of (sections.VMSTAT ?? '').split('\n')) {
    const [key, raw] = line.trim().split(/\s+/)
    const number = Number(raw)
    if (key && Number.isFinite(number)) {
      vmstat[key] = number
    }
  }

  const swapDevices: Array<{ filename: string; type: string; sizeBytes: number; usedBytes: number }> = []
  for (const line of (sections.SWAP ?? '').split('\n').slice(1)) {
    const cols = splitColumns(line, 4)
    if (!cols) {
      continue
    }
    const sizeKb = toNumber(cols[2])
    const usedKb = toNumber(cols[3])
    if (sizeKb === null || usedKb === null) {
      continue
    }
    swapDevices.push({
      filename: cols[0],
      type: cols[1],
      sizeBytes: sizeKb * 1024,
      usedBytes: usedKb * 1024
    })
  }

  const consumers = selectProcesses(parseProcessTable(sections.PS), 'mem', consumerCount).map(
    (row) => ({
      pid: row.pid,
      user: row.user,
      rssBytes: row.rssBytes,
      memPercent: row.memPercent,
      command: row.command
    })
  )

  return JSON.stringify(
    {
      totalBytes: total,
      availableBytes: available,
      usedBytes: used,
      freeBytes: free,
      usedPercent: total > 0 ? round1((used / total) * 100) : 0,
      buffersBytes: kib('Buffers'),
      cachedBytes: kib('Cached'),
      reclaimableBytes: kib('SReclaimable'),
      sharedBytes: kib('Shmem'),
      dirtyBytes: kib('Dirty'),
      swap: {
        totalBytes: kib('SwapTotal'),
        freeBytes: kib('SwapFree'),
        usedBytes: Math.max(0, kib('SwapTotal') - kib('SwapFree')),
        devices: swapDevices
      },
      pressure: Object.keys(pressure).length > 0 ? pressure : null,
      oom: {
        killsSinceBoot: vmstat.oom_kill ?? 0,
        pagesReclaimedDirectly: vmstat.pgsteal_direct ?? 0,
        pagesScannedDirectly: vmstat.pgscan_direct ?? 0,
        swapInPages: vmstat.pswpin ?? 0,
        swapOutPages: vmstat.pswpout ?? 0
      },
      topConsumers: consumers
    },
    null,
    2
  )
}


