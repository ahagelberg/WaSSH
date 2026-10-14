/** One `/command` the composer understands. */
export interface AiAgentSlashCommand {
  /** Command word without the leading slash, lowercase */
  name: string
  /** How the command is spelled in the UI, including the slash */
  label: string
  /** One-line explanation shown in the command list */
  description: string
}

/**
 * Commands recognized at the start of a composer message. All of them take no
 * arguments: as soon as whitespace follows the command word the text is treated
 * as an ordinary message instead.
 */
export const AI_AGENT_SLASH_COMMANDS: AiAgentSlashCommand[] = [
  {
    name: 'summary',
    label: '/summary',
    description: 'Summarize this chat into context for a new chat'
  },
  {
    name: 'context',
    label: '/context',
    description: 'Same as /summary — carry-over context for a new chat'
  },
  {
    name: 'new',
    label: '/new',
    description: 'Start a new chat'
  },
  {
    name: 'transfer',
    label: '/transfer',
    description: 'Summarize this chat, start a new chat, and put the summary there'
  }
]

/** Parsed composer text that names a command. */
export interface AiAgentSlashInput {
  /** Lowercase text typed after the slash, e.g. "sum" for "/sum" */
  prefix: string
  /** Commands whose name starts with `prefix`, in declaration order */
  matches: AiAgentSlashCommand[]
  /** The command `prefix` spells out exactly, if any */
  exact: AiAgentSlashCommand | null
}

/**
 * Interpret composer text as a command. Returns null when the text is not a bare
 * slash word — ordinary messages (including ones that merely start with a slash)
 * are left alone and sent to the model.
 */
export function parseSlashInput(raw: string): AiAgentSlashInput | null {
  const text = raw.trim()
  if (!text.startsWith('/')) {
    return null
  }
  const prefix = text.slice(1).toLowerCase()
  if (/\s/.test(prefix)) {
    return null
  }
  return {
    prefix,
    matches: AI_AGENT_SLASH_COMMANDS.filter((command) => command.name.startsWith(prefix)),
    exact: AI_AGENT_SLASH_COMMANDS.find((command) => command.name === prefix) ?? null
  }
}
