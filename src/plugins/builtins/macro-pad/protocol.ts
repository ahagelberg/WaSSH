export type MacroPadRendererMessage = { type: 'send'; text: string }

export function isMacroPadRendererMessage(value: unknown): value is MacroPadRendererMessage {
  if (!value || typeof value !== 'object') {
    return false
  }
  const { type, text } = value as Partial<MacroPadRendererMessage>
  return type === 'send' && typeof text === 'string'
}
