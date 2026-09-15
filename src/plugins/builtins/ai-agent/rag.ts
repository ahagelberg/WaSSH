import { readdir, readFile, stat } from 'fs/promises'
import { extname, join } from 'path'
import { extractText, getDocumentProxy } from 'unpdf'
import {
  AI_AGENT_RAG_CHUNK_CHARS,
  AI_AGENT_RAG_CHUNK_OVERLAP_CHARS,
  AI_AGENT_RAG_DEFAULT_RESULT_LIMIT,
  AI_AGENT_RAG_EMBEDDING_BATCH_SIZE,
  AI_AGENT_RAG_FILE_EXTENSIONS,
  AI_AGENT_RAG_IGNORED_DIRS,
  AI_AGENT_RAG_MAX_CHUNKS,
  AI_AGENT_RAG_MAX_FILES,
  AI_AGENT_RAG_MAX_FILE_BYTES,
  AI_AGENT_RAG_MAX_PDF_BYTES,
  AI_AGENT_RAG_MAX_RESULT_LIMIT
} from './defaults'
import type { PluginMainContext } from '@plugin-api/main'

const RAG_INDEX_VERSION = 2
const TEXT_SEARCH_TOKEN_RE = /[\p{L}\p{N}_-]+/gu
const MIN_TEXT_SEARCH_TOKEN_CHARS = 2

type RagSearchMode = 'embedding' | 'text'

interface RagFileState {
  mtimeMs: number
  size: number
  chunkIds: string[]
}

interface RagChunkRecord {
  file: string
  index: number
  text: string
  embedding: number[]
}

interface RagIndexData {
  version: number
  searchMode: RagSearchMode
  providerId: string
  embeddingModel: string
  files: Record<string, RagFileState>
  chunks: Record<string, RagChunkRecord>
}

interface RagFolderFile {
  relPath: string
  absPath: string
  extension: string
  mtimeMs: number
  size: number
}

export type EmbedFn = (texts: string[]) => Promise<number[][]>

function emptyIndex(
  searchMode: RagSearchMode,
  providerId: string,
  embeddingModel: string
): RagIndexData {
  return { version: RAG_INDEX_VERSION, searchMode, providerId, embeddingModel, files: {}, chunks: {} }
}

function isRagIndexData(value: unknown): value is RagIndexData {
  if (!value || typeof value !== 'object') {
    return false
  }
  const v = value as Record<string, unknown>
  return (
    v.version === RAG_INDEX_VERSION &&
    (v.searchMode === 'embedding' || v.searchMode === 'text') &&
    typeof v.providerId === 'string' &&
    typeof v.embeddingModel === 'string' &&
    Boolean(v.files) &&
    typeof v.files === 'object' &&
    Boolean(v.chunks) &&
    typeof v.chunks === 'object'
  )
}

/** Recursively list indexable files under `root`, sorted for stable ordering. */
async function walkFolder(root: string): Promise<RagFolderFile[]> {
  const out: RagFolderFile[] = []

  async function walk(dir: string, relDir: string): Promise<void> {
    if (out.length >= AI_AGENT_RAG_MAX_FILES) {
      return
    }
    let entries
    try {
      entries = await readdir(dir, { withFileTypes: true })
    } catch {
      return
    }
    entries.sort((a, b) => a.name.localeCompare(b.name))
    for (const entry of entries) {
      if (out.length >= AI_AGENT_RAG_MAX_FILES) {
        return
      }
      const absPath = join(dir, entry.name)
      const relPath = relDir ? `${relDir}/${entry.name}` : entry.name
      if (entry.isDirectory()) {
        if (AI_AGENT_RAG_IGNORED_DIRS.has(entry.name)) {
          continue
        }
        await walk(absPath, relPath)
        continue
      }
      const extension = extname(entry.name).toLowerCase()
      if (!entry.isFile() || !AI_AGENT_RAG_FILE_EXTENSIONS.has(extension)) {
        continue
      }
      try {
        const st = await stat(absPath)
        const maxFileBytes =
          extension === '.pdf' ? AI_AGENT_RAG_MAX_PDF_BYTES : AI_AGENT_RAG_MAX_FILE_BYTES
        if (st.size > maxFileBytes) {
          continue
        }
        out.push({ relPath, absPath, extension, mtimeMs: st.mtimeMs, size: st.size })
      } catch {
        continue
      }
    }
  }

  await walk(root, '')
  return out
}

async function readIndexableText(file: RagFolderFile): Promise<string> {
  const data = await readFile(file.absPath)
  if (file.extension !== '.pdf') {
    return data.toString('utf8')
  }
  const pdf = await getDocumentProxy(
    new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
  )
  const { text } = await extractText(pdf, { mergePages: true })
  return text
}

/** Split text into overlapping character-sized chunks. */
function chunkText(text: string): string[] {
  const trimmed = text.trim()
  if (!trimmed) {
    return []
  }
  if (trimmed.length <= AI_AGENT_RAG_CHUNK_CHARS) {
    return [trimmed]
  }
  const chunks: string[] = []
  let start = 0
  while (start < trimmed.length) {
    const end = Math.min(start + AI_AGENT_RAG_CHUNK_CHARS, trimmed.length)
    chunks.push(trimmed.slice(start, end))
    if (end >= trimmed.length) {
      break
    }
    start = end - AI_AGENT_RAG_CHUNK_OVERLAP_CHARS
  }
  return chunks
}

/** Call `embedFn` in fixed-size batches, preserving input order. */
async function embedAll(embedFn: EmbedFn, texts: string[]): Promise<number[][]> {
  const out: number[][] = []
  for (let i = 0; i < texts.length; i += AI_AGENT_RAG_EMBEDDING_BATCH_SIZE) {
    const batch = texts.slice(i, i + AI_AGENT_RAG_EMBEDDING_BATCH_SIZE)
    out.push(...(await embedFn(batch)))
  }
  return out
}

function cosineSimilarity(a: number[], b: number[]): number {
  if (a.length === 0 || b.length === 0 || a.length !== b.length) {
    return 0
  }
  let dot = 0
  let normA = 0
  let normB = 0
  for (let i = 0; i < a.length; i += 1) {
    dot += a[i] * b[i]
    normA += a[i] * a[i]
    normB += b[i] * b[i]
  }
  if (normA === 0 || normB === 0) {
    return 0
  }
  return dot / (Math.sqrt(normA) * Math.sqrt(normB))
}

function textSearchTokens(text: string): string[] {
  return (text.toLowerCase().match(TEXT_SEARCH_TOKEN_RE) ?? []).filter(
    (token) => token.length >= MIN_TEXT_SEARCH_TOKEN_CHARS
  )
}

function textSearchScore(queryTokens: Set<string>, text: string): number {
  const textTokens = textSearchTokens(text)
  if (queryTokens.size === 0 || textTokens.length === 0) {
    return 0
  }
  const counts = new Map<string, number>()
  for (const token of textTokens) {
    if (queryTokens.has(token)) {
      counts.set(token, (counts.get(token) ?? 0) + 1)
    }
  }
  let score = 0
  for (const count of counts.values()) {
    score += 1 + Math.log(count)
  }
  return score / Math.sqrt(textTokens.length)
}

function removeFileFromIndex(index: RagIndexData, relPath: string): void {
  const existing = index.files[relPath]
  if (!existing) {
    return
  }
  for (const chunkId of existing.chunkIds) {
    delete index.chunks[chunkId]
  }
  delete index.files[relPath]
}

/**
 * Re-sync one knowledge base folder's index against the current filesystem
 * state: removes files no longer present, and (re)chunks + (re)embeds only
 * files that are new or whose mtime/size changed since the last sync.
 */
async function syncFolderIndex(
  ctx: PluginMainContext,
  scopeId: string,
  folder: string,
  searchMode: RagSearchMode,
  providerId: string,
  embeddingModel: string,
  embedFn?: EmbedFn
): Promise<RagIndexData> {
  const stored = ctx.getData(scopeId)
  let index = isRagIndexData(stored)
    ? stored
    : emptyIndex(searchMode, providerId, embeddingModel)
  if (
    index.searchMode !== searchMode ||
    (searchMode === 'embedding' &&
      (index.providerId !== providerId || index.embeddingModel !== embeddingModel))
  ) {
    // Embeddings from a different provider/model live in a different vector
    // space and can't be compared, so a provider/model change forces a
    // full rebuild.
    index = emptyIndex(searchMode, providerId, embeddingModel)
  }

  const diskFiles = await walkFolder(folder)
  const diskPaths = new Set(diskFiles.map((f) => f.relPath))

  for (const relPath of Object.keys(index.files)) {
    if (!diskPaths.has(relPath)) {
      removeFileFromIndex(index, relPath)
    }
  }

  let chunkCount = Object.keys(index.chunks).length
  for (const file of diskFiles) {
    if (chunkCount >= AI_AGENT_RAG_MAX_CHUNKS) {
      break
    }
    const existing = index.files[file.relPath]
    if (existing && existing.mtimeMs === file.mtimeMs && existing.size === file.size) {
      continue
    }
    removeFileFromIndex(index, file.relPath)

    let text: string
    try {
      text = await readIndexableText(file)
    } catch {
      continue
    }
    const pieces = chunkText(text)
    if (pieces.length === 0) {
      index.files[file.relPath] = { mtimeMs: file.mtimeMs, size: file.size, chunkIds: [] }
      continue
    }
    const budget = AI_AGENT_RAG_MAX_CHUNKS - chunkCount
    const limitedPieces = pieces.slice(0, Math.max(budget, 0))
    const embeddings = embedFn ? await embedAll(embedFn, limitedPieces) : []
    const chunkIds: string[] = []
    limitedPieces.forEach((piece, i) => {
      const id = `${file.relPath}#${i}`
      index.chunks[id] = { file: file.relPath, index: i, text: piece, embedding: embeddings[i] ?? [] }
      chunkIds.push(id)
    })
    index.files[file.relPath] = { mtimeMs: file.mtimeMs, size: file.size, chunkIds }
    chunkCount += chunkIds.length
  }

  ctx.setData(index, scopeId)
  return index
}

export interface RagFolderConfig {
  scopeId: string
  folder: string
  label: string
}

export interface RagSearchOptions {
  query: string
  limit?: number
  folders: RagFolderConfig[]
  searchMode: RagSearchMode
  providerId: string
  embeddingModel: string
  embed?: EmbedFn
}

/** Sync every configured knowledge base folder, then return the top matching chunks for `query`. */
export async function searchKnowledgeBase(ctx: PluginMainContext, opts: RagSearchOptions): Promise<string> {
  const folders = opts.folders.filter((f) => f.folder.trim().length > 0)
  if (folders.length === 0) {
    return 'No knowledge base folder is configured. Set one in AI agent settings (app-wide and/or per-host).'
  }

  const scored: Array<{ label: string; chunk: RagChunkRecord }> = []
  for (const f of folders) {
    let index: RagIndexData
    try {
      index = await syncFolderIndex(
        ctx,
        f.scopeId,
        f.folder,
        opts.searchMode,
        opts.providerId,
        opts.embeddingModel,
        opts.embed
      )
    } catch (err) {
      return `Failed to index knowledge base folder "${f.folder}": ${err instanceof Error ? err.message : String(err)}`
    }
    for (const chunk of Object.values(index.chunks)) {
      scored.push({ label: f.label, chunk })
    }
  }

  if (scored.length === 0) {
    return 'The configured knowledge base folder(s) contain no indexable text files yet.'
  }

  let ranked: Array<{ label: string; chunk: RagChunkRecord; score: number }>
  if (opts.searchMode === 'text') {
    const queryTokens = new Set(textSearchTokens(opts.query))
    ranked = scored
      .map((item) => ({ ...item, score: textSearchScore(queryTokens, item.chunk.text) }))
      .filter((item) => item.score > 0)
      .sort((a, b) => b.score - a.score)
  } else {
    if (!opts.embed) {
      return 'No embedding function is configured for knowledge base search.'
    }
    let queryEmbedding: number[]
    try {
      const [embedding] = await opts.embed([opts.query])
      queryEmbedding = embedding ?? []
    } catch (err) {
      return `Failed to compute query embedding: ${err instanceof Error ? err.message : String(err)}`
    }
    ranked = scored
      .map((item) => ({ ...item, score: cosineSimilarity(queryEmbedding, item.chunk.embedding) }))
      .sort((a, b) => b.score - a.score)
  }

  if (ranked.length === 0) {
    return opts.searchMode === 'text'
      ? 'No indexed text matches the search terms.'
      : 'No relevant knowledge base content was found.'
  }

  const count = Math.min(
    Math.max(Number(opts.limit) || AI_AGENT_RAG_DEFAULT_RESULT_LIMIT, 1),
    AI_AGENT_RAG_MAX_RESULT_LIMIT
  )
  const top = ranked.slice(0, count)

  const formatted = top
    .map(
      (item, i) =>
        `[${i + 1}] ${item.label} knowledge base — ${item.chunk.file} (score ${item.score.toFixed(3)}):\n${item.chunk.text}`
    )
    .join('\n\n')

  return `Knowledge base search results for "${opts.query}":\n\n${formatted}`
}
