import type { DockerClient } from './dockerClient'
import { EMPTY_UPDATE_STATUS } from './protocol'
import type {
  DockerContainerView,
  DockerPullProgress,
  DockerUpdateEvent,
  DockerUpdateResult,
  DockerUpdateStatus
} from './protocol'
import { backupName, buildRecreatePlan } from './recreate'

/** Everything the update flow needs from the host session. */
export interface UpdateDeps {
  client: DockerClient
  /** Push an update progress event to the view. */
  emit: (event: DockerUpdateEvent) => void
  /** Push image-pull progress to the view. */
  onPullProgress: (progress: DockerPullProgress) => void
  /** Re-list containers after the daemon state changes. */
  refresh: () => Promise<void>
}

export interface UpdateOutcome {
  ok: boolean
  error?: string
  warnings?: string[]
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** Image reference to check for updates, or null when it cannot go stale. */
export function updatableReference(image: string): string | null {
  const ref = image.trim()
  if (!ref || ref === '<none>:<none>' || ref.startsWith('sha256:') || ref.includes('@sha256:')) {
    return null
  }
  const lastSlash = ref.lastIndexOf('/')
  const lastColon = ref.lastIndexOf(':')
  return lastColon > lastSlash ? ref : `${ref}:latest`
}

/** Repository part of a tagged reference (`reg:5000/app:1` → `reg:5000/app`). */
export function repositoryOf(reference: string): string {
  const lastSlash = reference.lastIndexOf('/')
  const lastColon = reference.lastIndexOf(':')
  return lastColon > lastSlash ? reference.slice(0, lastColon) : reference
}

/** Compare the locally recorded digest with the registry's current one. */
export async function referenceIsStale(
  client: DockerClient,
  reference: string
): Promise<{ stale: boolean; error?: string }> {
  let digests: string[] = []
  try {
    const info = await client.imageInspect(reference)
    digests = (info.RepoDigests ?? []).filter((entry): entry is string => typeof entry === 'string')
  } catch (error) {
    return { stale: false, error: errorMessage(error) }
  }

  const repo = repositoryOf(reference)
  const localDigest = digests.find((entry) => entry.startsWith(`${repo}@`))?.split('@')[1]
  if (!localDigest) {
    // Built locally or pulled without digest metadata — nothing to compare.
    return { stale: false }
  }

  try {
    const remote = await client.distributionDigest(reference)
    if (!remote) {
      return { stale: false }
    }
    return { stale: remote !== localDigest }
  } catch (error) {
    return { stale: false, error: errorMessage(error) }
  }
}

/**
 * Which containers run an image older than the registry's current tag. All
 * unique image references are checked in parallel; failures are reported
 * per-image rather than failing the whole check.
 */
export async function computeUpdateStatus(
  deps: UpdateDeps,
  containers: DockerContainerView[]
): Promise<DockerUpdateStatus> {
  const byReference = new Map<string, string[]>()
  for (const container of containers) {
    const reference = updatableReference(container.image)
    if (!reference) {
      continue
    }
    const ids = byReference.get(reference)
    if (ids) {
      ids.push(container.id)
    } else {
      byReference.set(reference, [container.id])
    }
  }

  const entries = await Promise.all(
    Array.from(byReference.entries()).map(async ([reference, ids]) => ({
      reference,
      ids,
      ...(await referenceIsStale(deps.client, reference))
    }))
  )

  const staleContainerIds: string[] = []
  const errors: Array<{ image: string; error: string }> = []
  for (const entry of entries) {
    if (entry.error) {
      errors.push({ image: entry.reference, error: entry.error })
    } else if (entry.stale) {
      staleContainerIds.push(...entry.ids)
    }
  }

  return {
    ...EMPTY_UPDATE_STATUS,
    checking: false,
    at: Date.now(),
    staleContainerIds,
    checkedImages: entries.map((entry) => entry.reference),
    errors
  }
}

/**
 * Update one container in place: pull its image tag, then stop, rename and
 * replace it with an identical container built from its inspected config. The
 * previous container is renamed back and restarted if the replacement cannot be
 * created or started, so a failed update is not destructive.
 */
export async function runContainerUpdate(
  deps: UpdateDeps,
  id: string
): Promise<UpdateOutcome> {
  const { client } = deps

  let raw
  try {
    raw = await client.inspectContainerRaw(id)
  } catch (error) {
    return { ok: false, error: errorMessage(error) }
  }

  const containerName = (raw.Name ?? '').replace(/^\//, '') || id.slice(0, 12)
  const imageReference = raw.Config?.Image ?? raw.Image ?? ''

  let plan
  try {
    plan = buildRecreatePlan(raw)
  } catch (error) {
    const message = errorMessage(error)
    deps.emit({ id, name: containerName, image: imageReference, phase: 'failed', message })
    return { ok: false, error: message }
  }

  const wasRunning = Boolean(raw.State?.Running)

  try {
    deps.emit({ id, name: containerName, image: plan.image, phase: 'pulling' })
    await client.pullImage(plan.image, deps.onPullProgress)

    deps.emit({ id, name: containerName, image: plan.image, phase: 'recreating' })
    if (wasRunning) {
      try {
        await client.containerAction(id, 'stop')
      } catch {
        /* already stopping or stopped */
      }
    }

    // Move the old container aside so the original name is free, keeping it
    // available for rollback until the replacement is running.
    let backup: string | null = null
    if (containerName) {
      backup = backupName(containerName, raw.Id ?? id)
      await client.renameContainer(id, backup)
    }

    let newId: string | null = null
    try {
      const created = await client.createContainerRaw(plan.body, containerName || undefined)
      newId = created.Id ?? null
      if (!newId) {
        throw new Error('Docker did not return a container id')
      }
      if (wasRunning) {
        deps.emit({ id, name: containerName, image: plan.image, phase: 'starting' })
        await client.startContainer(newId)
      }
    } catch (error) {
      if (newId) {
        try {
          await client.removeContainer(newId, true)
        } catch {
          /* nothing else we can do */
        }
      }
      if (backup) {
        try {
          await client.renameContainer(id, containerName)
          if (wasRunning) {
            await client.startContainer(id)
          }
        } catch {
          /* rollback is best effort */
        }
      }
      throw error
    }

    // The replacement is up; drop the previous container but keep its volumes.
    try {
      await client.removeContainer(id, true, false)
    } catch {
      /* the renamed backup is left behind rather than lost */
    }

    deps.emit({ id, name: containerName, image: plan.image, phase: 'done' })
    await deps.refresh()
    return { ok: true, warnings: plan.warnings }
  } catch (error) {
    const message = errorMessage(error)
    deps.emit({ id, name: containerName, image: plan.image, phase: 'failed', message })
    await deps.refresh()
    return { ok: false, error: message, warnings: plan.warnings }
  }
}

/** Update every container whose image tag is stale. */
export async function updateAllContainers(
  deps: UpdateDeps,
  containers: DockerContainerView[],
  status: DockerUpdateStatus,
  isUpdating: (id: string) => boolean,
  update: (id: string) => Promise<UpdateOutcome>
): Promise<DockerUpdateResult> {
  const stale = new Set(status.staleContainerIds)
  const targets = containers.filter((container) => stale.has(container.id))

  const result: DockerUpdateResult = { ok: true, updated: [], skipped: [], failures: [] }
  for (const container of targets) {
    const label = container.name || container.shortId
    if (isUpdating(container.id)) {
      result.skipped?.push(label)
      continue
    }
    const outcome = await update(container.id)
    if (outcome.ok) {
      result.updated?.push(label)
    } else {
      result.failures?.push({ id: container.id, name: label, error: outcome.error ?? 'Update failed' })
    }
  }
  result.ok = (result.failures?.length ?? 0) === 0
  return result
}

export { buildRecreatePlan, backupName }
