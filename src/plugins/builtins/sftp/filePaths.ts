import { posix } from 'node:path'
import type { PluginFileSession } from '@plugin-api/main'
import { joinRemotePath } from '@plugin-api/main'

export async function availableCopyPath(
  target: PluginFileSession,
  directory: string,
  name: string
): Promise<string> {
  const extension = posix.extname(name)
  const stem = extension ? name.slice(0, -extension.length) : name
  for (let index = 0; ; index += 1) {
    const copyLabel = index === 0 ? '' : index === 1 ? ' (copy)' : ` (copy ${index})`
    const candidate = joinRemotePath(directory, `${stem}${copyLabel}${extension}`)
    if (!(await target.statSafe(candidate))) {
      return candidate
    }
  }
}