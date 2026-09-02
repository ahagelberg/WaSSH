import { AI_AGENT_RULE_REGEX_PREFIX } from '../../../shared/plugins'

export type RuleDecision = 'allow' | 'deny' | 'ask'

/** Regex special characters escaped outside `*` (kept as glob wildcard). */
const REGEX_SPECIAL_CHARS = /[.+?^${}()|[\]\\]/g

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
