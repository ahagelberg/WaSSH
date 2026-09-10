import {
  AI_AGENT_ANTHROPIC_MODELS_PATH,
  AI_AGENT_ANTHROPIC_PATH,
  AI_AGENT_ANTHROPIC_VERSION,
  AI_AGENT_OPENAI_CHAT_PATH,
  AI_AGENT_OPENAI_MODELS_PATH,
  AI_AGENT_PROTOCOL_ANTHROPIC,
  type AiAgentProviderProtocol
} from './protocol'
import type { PluginApiMethod } from '@plugin-api/shared'

/** Cap on provider error text attached to thrown errors */
const MAX_ERROR_BODY_CHARS = 500

export interface ApiToolCallMsg {
  id: string
  name: string
  /** JSON string of the tool arguments */
  arguments: string
}

export type ApiMessage =
  | { role: 'user'; content: string }
  | { role: 'assistant'; content?: string; toolCalls?: ApiToolCallMsg[] }
  | { role: 'tool'; toolCallId: string; content: string }

export interface CompletionResult {
  text: string
  toolCalls: ApiToolCallMsg[]
  stopReason?: string
}

export interface CompletionOptions {
  baseUrl: string
  apiKey: string
  protocol: AiAgentProviderProtocol
  model: string
  system: string
  messages: ApiMessage[]
  tools?: PluginApiMethod[]
  maxTokens: number
  signal: AbortSignal
  /** Called as assistant text streams in */
  onDelta: (text: string) => void
}

/** The default shell command tool name */
export const RUN_COMMAND_TOOL_NAME = 'run_command'

function toOpenAiTool(tool: PluginApiMethod): Record<string, unknown> {
  return {
    type: 'function',
    function: {
      name: tool.name,
      description: tool.description,
      parameters: tool.parameters
    }
  }
}

function toAnthropicTool(tool: PluginApiMethod): Record<string, unknown> {
  return {
    name: tool.name,
    description: tool.description,
    input_schema: tool.parameters
  }
}

function joinUrl(baseUrl: string, path: string): string {
  return `${baseUrl.replace(/\/+$/, '')}${path}`
}

async function errorText(res: Response): Promise<string> {
  const body = await res.text().catch(() => '')
  return body.slice(0, MAX_ERROR_BODY_CHARS)
}

function toOpenAiMessages(
  system: string,
  messages: ApiMessage[]
): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = [{ role: 'system', content: system }]
  for (const m of messages) {
    if (m.role === 'user') {
      out.push({ role: 'user', content: m.content })
    } else if (m.role === 'tool') {
      out.push({ role: 'tool', tool_call_id: m.toolCallId, content: m.content })
    } else {
      out.push({
        role: 'assistant',
        content: m.content ?? '',
        tool_calls: m.toolCalls?.map((tc) => ({
          id: tc.id,
          type: 'function',
          function: { name: tc.name, arguments: tc.arguments }
        }))
      })
    }
  }
  return out
}

function parseArguments(json: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(json) as unknown
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}

function toAnthropicMessages(messages: ApiMessage[]): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = []
  for (const m of messages) {
    if (m.role === 'user') {
      out.push({ role: 'user', content: m.content })
      continue
    }
    if (m.role === 'tool') {
      const block = {
        type: 'tool_result',
        tool_use_id: m.toolCallId,
        content: m.content
      }
      const last = out[out.length - 1]
      if (last && last.role === 'user' && Array.isArray(last.content)) {
        ;(last.content as unknown[]).push(block)
      } else {
        out.push({ role: 'user', content: [block] })
      }
      continue
    }
    const content: Record<string, unknown>[] = []
    if (m.content) {
      content.push({ type: 'text', text: m.content })
    }
    for (const tc of m.toolCalls ?? []) {
      content.push({
        type: 'tool_use',
        id: tc.id,
        name: tc.name,
        input: parseArguments(tc.arguments)
      })
    }
    out.push({ role: 'assistant', content })
  }
  return out
}

async function readSse(
  res: Response,
  onPayload: (payload: string) => void
): Promise<void> {
  const reader = res.body?.getReader()
  if (!reader) {
    throw new Error('Provider returned no response body')
  }
  const decoder = new TextDecoder()
  let buffer = ''
  for (;;) {
    const { done, value } = await reader.read()
    if (done) {
      break
    }
    buffer += decoder.decode(value, { stream: true })
    let newline = buffer.indexOf('\n')
    while (newline >= 0) {
      const line = buffer.slice(0, newline).replace(/\r$/, '')
      buffer = buffer.slice(newline + 1)
      newline = buffer.indexOf('\n')
      if (!line.startsWith('data:')) {
        continue
      }
      const payload = line.slice(5).trim()
      if (payload && payload !== '[DONE]') {
        onPayload(payload)
      }
    }
  }
}

async function completeOpenAi(opts: CompletionOptions): Promise<CompletionResult> {
  const tools = opts.tools && opts.tools.length > 0 ? opts.tools.map(toOpenAiTool) : undefined
  const res = await fetch(joinUrl(opts.baseUrl, AI_AGENT_OPENAI_CHAT_PATH), {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      ...(opts.apiKey ? { authorization: `Bearer ${opts.apiKey}` } : {})
    },
    body: JSON.stringify({
      model: opts.model,
      messages: toOpenAiMessages(opts.system, opts.messages),
      ...(tools ? { tools } : {}),
      stream: true
    }),
    signal: opts.signal
  })
  if (!res.ok) {
    throw new Error(`Provider error ${res.status}: ${await errorText(res)}`)
  }

  let text = ''
  const toolFragments = new Map<
    number,
    { id: string; name: string; arguments: string }
  >()
  let stopReason: string | undefined

  await readSse(res, (payload) => {
    const json = JSON.parse(payload) as Record<string, unknown>
    const choice = (json.choices as Record<string, unknown>[] | undefined)?.[0]
    if (!choice) {
      return
    }
    if (typeof choice.finish_reason === 'string' && choice.finish_reason) {
      stopReason = choice.finish_reason
    }
    const delta = choice.delta as Record<string, unknown> | undefined
    if (!delta) {
      return
    }
    if (typeof delta.content === 'string' && delta.content) {
      text += delta.content
      opts.onDelta(delta.content)
    }
    const calls = delta.tool_calls as Record<string, unknown>[] | undefined
    if (!calls) {
      return
    }
    for (const call of calls) {
      const index = Number(call.index ?? 0)
      const frag = toolFragments.get(index) ?? { id: '', name: '', arguments: '' }
      if (typeof call.id === 'string' && call.id) {
        frag.id = call.id
      }
      const fn = call.function as Record<string, unknown> | undefined
      if (fn) {
        if (typeof fn.name === 'string' && fn.name) {
          frag.name = (frag.name ? frag.name : '') + fn.name
        }
        if (typeof fn.arguments === 'string') {
          frag.arguments += fn.arguments
        }
      }
      toolFragments.set(index, frag)
    }
  })

  const toolCalls: ApiToolCallMsg[] = Array.from(toolFragments.entries())
    .sort((a, b) => a[0] - b[0])
    .filter(([, frag]) => Boolean(frag.name))
    .map(([index, frag]) => ({
      id: frag.id || `call_${index}_${Math.random().toString(36).slice(2, 9)}`,
      name: frag.name,
      arguments: frag.arguments
    }))
  return { text, toolCalls, stopReason }
}

async function completeAnthropic(opts: CompletionOptions): Promise<CompletionResult> {
  const tools = opts.tools && opts.tools.length > 0 ? opts.tools.map(toAnthropicTool) : undefined
  const res = await fetch(joinUrl(opts.baseUrl, AI_AGENT_ANTHROPIC_PATH), {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-api-key': opts.apiKey,
      'anthropic-version': AI_AGENT_ANTHROPIC_VERSION
    },
    body: JSON.stringify({
      model: opts.model,
      max_tokens: opts.maxTokens,
      system: opts.system,
      messages: toAnthropicMessages(opts.messages),
      ...(tools ? { tools } : {}),
      stream: true
    }),
    signal: opts.signal
  })
  if (!res.ok) {
    throw new Error(`Provider error ${res.status}: ${await errorText(res)}`)
  }

  let text = ''
  const toolBlocks = new Map<
    number,
    { id: string; name: string; arguments: string }
  >()
  let stopReason: string | undefined

  await readSse(res, (payload) => {
    const json = JSON.parse(payload) as Record<string, unknown>
    const type = json.type
    if (type === 'content_block_start') {
      const block = json.content_block as Record<string, unknown> | undefined
      const index = Number(json.index ?? 0)
      if (block?.type === 'tool_use') {
        toolBlocks.set(index, {
          id: typeof block.id === 'string' && block.id ? block.id : `call_${index}_${Math.random().toString(36).slice(2, 9)}`,
          name: typeof block.name === 'string' ? block.name : '',
          arguments: ''
        })
      }
      return
    }
    if (type === 'content_block_delta') {
      const delta = json.delta as Record<string, unknown> | undefined
      if (!delta) {
        return
      }
      if (delta.type === 'text_delta' && typeof delta.text === 'string') {
        text += delta.text
        opts.onDelta(delta.text)
      } else if (delta.type === 'input_json_delta' && typeof delta.partial_json === 'string') {
        const index = Number(json.index ?? 0)
        const block = toolBlocks.get(index)
        if (block) {
          block.arguments += delta.partial_json
        }
      }
      return
    }
    if (type === 'message_delta') {
      const delta = json.delta as Record<string, unknown> | undefined
      if (delta && typeof delta.stop_reason === 'string') {
        stopReason = delta.stop_reason
      }
    }
  })

  const toolCalls: ApiToolCallMsg[] = Array.from(toolBlocks.entries())
    .sort((a, b) => a[0] - b[0])
    .filter(([, block]) => Boolean(block.name))
    .map(([index, block]) => ({
      id: block.id || `call_${index}_${Math.random().toString(36).slice(2, 9)}`,
      name: block.name,
      arguments: block.arguments
    }))
  return { text, toolCalls, stopReason }
}

/** Stream one model turn with the run_command tool available. */
export async function complete(opts: CompletionOptions): Promise<CompletionResult> {
  if (opts.protocol === AI_AGENT_PROTOCOL_ANTHROPIC) {
    return completeAnthropic(opts)
  }
  return completeOpenAi(opts)
}

export interface ListModelsOptions {
  baseUrl: string
  apiKey: string
  protocol: AiAgentProviderProtocol
}

/** Extract non-empty string "id" fields from a provider's models-list JSON body. */
function extractModelIds(json: unknown): string[] {
  const data = (json as { data?: unknown } | null)?.data
  if (!Array.isArray(data)) {
    return []
  }
  const ids: string[] = []
  for (const entry of data) {
    const id = (entry as { id?: unknown } | null)?.id
    if (typeof id === 'string' && id.length > 0) {
      ids.push(id)
    }
  }
  return ids
}

/** Derive Ollama's native API origin (scheme + host) from an OpenAI-compat base URL. */
function ollamaOriginFrom(baseUrl: string): string {
  try {
    const url = new URL(baseUrl)
    return `${url.protocol}//${url.host}`
  } catch {
    // Not a full URL (unlikely) — strip a trailing /v1 segment as a best effort.
    return baseUrl.replace(/\/+$/, '').replace(/\/v1$/, '')
  }
}

/** Extract model names/tags from Ollama's native `/api/tags` response body. */
function extractOllamaTagNames(json: unknown): string[] {
  const models = (json as { models?: unknown } | null)?.models
  if (!Array.isArray(models)) {
    return []
  }
  const ids: string[] = []
  for (const entry of models) {
    const rec = entry as { model?: unknown; name?: unknown } | null
    const id = (typeof rec?.model === 'string' && rec.model) || (typeof rec?.name === 'string' && rec.name) || ''
    if (id) {
      ids.push(id)
    }
  }
  return ids
}

async function listOpenAiCompatModels(opts: ListModelsOptions): Promise<string[]> {
  const res = await fetch(joinUrl(opts.baseUrl, AI_AGENT_OPENAI_MODELS_PATH), {
    method: 'GET',
    headers: {
      ...(opts.apiKey ? { authorization: `Bearer ${opts.apiKey}` } : {})
    }
  })
  if (!res.ok) {
    throw new Error(`Provider error ${res.status}: ${await errorText(res)}`)
  }
  return extractModelIds(await res.json())
}

/**
 * Ollama's own model list lives at GET /api/tags (outside the /v1 OpenAI
 * compat prefix) and reports every model pulled locally, e.g. via
 * `ollama pull`. Not every OpenAI-compatible server exposes this, so
 * failures here are non-fatal — see listOpenAiModels().
 */
async function listOllamaTags(baseUrl: string): Promise<string[]> {
  const res = await fetch(`${ollamaOriginFrom(baseUrl)}/api/tags`, { method: 'GET' })
  if (!res.ok) {
    throw new Error(`Provider error ${res.status}: ${await errorText(res)}`)
  }
  return extractOllamaTagNames(await res.json())
}

async function listOpenAiModels(opts: ListModelsOptions): Promise<string[]> {
  // Query both the standard OpenAI-compat listing (GET {baseUrl}/models) and
  // Ollama's native tags endpoint, since Ollama's own /api/tags is the most
  // reliable way to see everything currently pulled locally, while other
  // OpenAI-compatible servers only support the former.
  const [compat, tags] = await Promise.allSettled([
    listOpenAiCompatModels(opts),
    listOllamaTags(opts.baseUrl)
  ])
  const ids = new Set<string>()
  if (compat.status === 'fulfilled') {
    for (const id of compat.value) {
      ids.add(id)
    }
  }
  if (tags.status === 'fulfilled') {
    for (const id of tags.value) {
      ids.add(id)
    }
  }
  if (ids.size === 0) {
    const failure =
      compat.status === 'rejected' ? compat.reason : tags.status === 'rejected' ? tags.reason : undefined
    if (failure) {
      throw failure instanceof Error ? failure : new Error(String(failure))
    }
  }
  return Array.from(ids)
}

async function listAnthropicModels(opts: ListModelsOptions): Promise<string[]> {
  const res = await fetch(joinUrl(opts.baseUrl, AI_AGENT_ANTHROPIC_MODELS_PATH), {
    method: 'GET',
    headers: {
      'x-api-key': opts.apiKey,
      'anthropic-version': AI_AGENT_ANTHROPIC_VERSION
    }
  })
  if (!res.ok) {
    throw new Error(`Provider error ${res.status}: ${await errorText(res)}`)
  }
  return extractModelIds(await res.json())
}

/** Fetch the list of model ids a provider currently reports as available. */
export async function listModels(opts: ListModelsOptions): Promise<string[]> {
  if (opts.protocol === AI_AGENT_PROTOCOL_ANTHROPIC) {
    return listAnthropicModels(opts)
  }
  return listOpenAiModels(opts)
}
