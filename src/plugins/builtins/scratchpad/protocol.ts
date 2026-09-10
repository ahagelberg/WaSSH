export interface ScratchpadData {
  content: string
}

export function contentFromData(data: unknown): string {
  if (!data || typeof data !== 'object' || Array.isArray(data)) {
    return ''
  }
  const content = (data as ScratchpadData).content
  return typeof content === 'string' ? content : ''
}
