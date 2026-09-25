import { AI_AGENT_RULE_REGEX_PREFIX } from './protocol'

export type RuleDecision = 'allow' | 'deny' | 'ask'

/** Rule pattern matching every command, used by "allow all commands this session". */
export const ALLOW_ALL_PATTERN = '*'

/** Regex special characters escaped outside `*` (kept as glob wildcard). */
const REGEX_SPECIAL_CHARS = /[.+?^${}()|[\]\\]/g

/** Every regex metacharacter, including the glob `*` (unlike REGEX_SPECIAL_CHARS). */
const REGEX_META_CHARS = /[.*+?^${}()|[\]\\]/g

/** Command words are whitespace-separated; a pattern scope is a prefix of them. */
const COMMAND_WORD_SEPARATOR = /\s+/

/** Number of leading command words offered as "starts with" scopes. */
export const APPROVAL_PREFIX_WORD_MAX = 4

/** One "always allow/deny" choice: the leading command words its rule covers. */
export interface ApprovalScope {
  /** Leading words covered; 0 marks the exact command. */
  wordCount: number
  /** Text shown for this scope. */
  text: string
  /** Rule list entry saved when this scope is chosen. */
  pattern: string
}

/** Comment / empty lines are ignored. */
function isCommentLine(line: string): boolean {
  return line.length === 0 || line.startsWith('#')
}

function ruleToRegExp(rawLine: string): RegExp | null {
  const line = rawLine.trim()
  if (isCommentLine(line)) {
    return null
  }
  if (line.startsWith(AI_AGENT_RULE_REGEX_PREFIX)) {
    const source = line.slice(AI_AGENT_RULE_REGEX_PREFIX.length).trim()
    if (!source) {
      return null
    }
    try {
      return new RegExp(source)
    } catch {
      return null
    }
  }
  const source = line.replace(REGEX_SPECIAL_CHARS, '\\$&').replace(/\*/g, '.*')
  return new RegExp(`^${source}$`)
}

function matchesAny(rules: string[] | undefined, command: string): boolean {
  if (!rules) {
    return false
  }
  for (const rule of rules) {
    const re = ruleToRegExp(rule)
    if (re && re.test(command)) {
      return true
    }
  }
  return false
}

function commandWords(command: string): string[] {
  return command.split(COMMAND_WORD_SEPARATOR).filter((word) => word !== '')
}

/**
 * Rule list entries an "always allow/deny" choice can add for one command:
 * "starts with the first n words" for n up to APPROVAL_PREFIX_WORD_MAX (fewer
 * when the command is shorter), plus the exact command last.
 */
export function approvalScopes(command: string): ApprovalScope[] {
  const words = commandWords(command)
  const scopes: ApprovalScope[] = []
  for (let count = 1; count <= Math.min(APPROVAL_PREFIX_WORD_MAX, words.length); count += 1) {
    const text = words.slice(0, count).join(' ')
    const literal = text.replace(REGEX_META_CHARS, '\\$&')
    scopes.push({
      wordCount: count,
      text,
      pattern: `${AI_AGENT_RULE_REGEX_PREFIX}^${literal}( |$)`
    })
  }
  scopes.push({ wordCount: 0, text: command, pattern: command })
  return scopes
}

/**
 * Permission decision for one command. Host rules win over app defaults,
 * deny always beats allow, otherwise the command asks for approval.
 */
export function decideCommand(
  command: string,
  hostAllow: string[] | undefined,
  hostDeny: string[] | undefined,
  appAllow: string[] | undefined,
  appDeny: string[] | undefined,
  extraAllow: string[],
  extraDeny: string[]
): RuleDecision {
  if (matchesAny([...extraDeny, ...(hostDeny ?? [])], command)) {
    return 'deny'
  }
  if (matchesAny([...extraAllow, ...(hostAllow ?? [])], command)) {
    return 'allow'
  }
  if (matchesAny(appDeny, command)) {
    return 'deny'
  }
  if (matchesAny(appAllow, command)) {
    return 'allow'
  }
  return 'ask'
}
