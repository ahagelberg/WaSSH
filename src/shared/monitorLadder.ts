import type { MonitorSeries } from './monitorSeries'

/**
 * Multi-resolution history ring. Each tier covers a longer span at a coarser
 * bucket width; every bucket stores min/max/avg per series so peaks survive
 * downsampling. Coarser tiers are derived from the next-finer tier as buckets
 * complete, so total memory is bounded by the tier capacities regardless of
 * how long the ladder runs.
 */

export interface MonitorTierSpec {
  /** Bucket width (seconds) */
  bucketSeconds: number
  /** Buckets retained */
  capacity: number
}

/** 1 s buckets for 5 minutes */
export const LADDER_TIER_FAST: MonitorTierSpec = { bucketSeconds: 1, capacity: 300 }
/** 15 s buckets for 1 hour */
export const LADDER_TIER_MEDIUM: MonitorTierSpec = { bucketSeconds: 15, capacity: 240 }
/** 60 s buckets for 24 hours */
export const LADDER_TIER_SLOW: MonitorTierSpec = { bucketSeconds: 60, capacity: 1440 }
/** 10 min buckets for 7 days */
export const LADDER_TIER_ARCHIVE: MonitorTierSpec = { bucketSeconds: 600, capacity: 1008 }

export const LADDER_TIERS: MonitorTierSpec[] = [
  LADDER_TIER_FAST,
  LADDER_TIER_MEDIUM,
  LADDER_TIER_SLOW,
  LADDER_TIER_ARCHIVE
]

/** Longest span any tier can answer for (seconds) */
export const LADDER_MAX_SPAN_SECONDS =
  LADDER_TIERS[LADDER_TIERS.length - 1].bucketSeconds *
  LADDER_TIERS[LADDER_TIERS.length - 1].capacity

/** One bucket of aggregated samples for a single series. */
export interface MonitorBucket {
  /** Bucket start (ms since epoch) */
  timestamp: number
  min: number
  max: number
  avg: number
  /** Samples folded into this bucket */
  count: number
}

/** A series' buckets across one tier, oldest first. */
export interface MonitorTierSeries {
  seriesId: string
  buckets: MonitorBucket[]
}

export interface MonitorTierSnapshot {
  bucketSeconds: number
  series: MonitorTierSeries[]
}

export interface MonitorHistorySnapshot {
  /** Buckets per tier, coarsest last */
  tiers: MonitorTierSnapshot[]
}

/** Pending accumulation for one series inside the currently-open bucket. */
interface OpenBucket {
  timestamp: number
  min: number
  max: number
  sum: number
  count: number
}

interface TierState {
  spec: MonitorTierSpec
  /** seriesId -> fixed-size ring of completed buckets, oldest first */
  rings: Map<string, MonitorBucket[]>
  /** seriesId -> bucket being filled right now */
  open: Map<string, OpenBucket>
}

function bucketStart(timestamp: number, bucketMs: number): number {
  return timestamp - (timestamp % bucketMs)
}

export class MonitorLadder {
  private readonly tiers: TierState[]
  private readonly knownSeries = new Set<string>()

  constructor(series: MonitorSeries[]) {
    this.tiers = LADDER_TIERS.map((spec) => ({
      spec,
      rings: new Map(),
      open: new Map()
    }))
    for (const item of series) {
      this.knownSeries.add(item.id)
    }
  }

  /** Series ids the ladder will accept samples for. */
  seriesIds(): string[] {
    return Array.from(this.knownSeries)
  }

  /**
   * Fold one sample into every tier. Values are keyed by series id; unknown
   * ids are ignored so a newer daemon can add series without breaking an
   * older app.
   */
  push(timestamp: number, values: Record<string, number>): void {
    for (const tier of this.tiers) {
      const bucketMs = tier.spec.bucketSeconds * 1000
      const start = bucketStart(timestamp, bucketMs)
      for (const [seriesId, value] of Object.entries(values)) {
        if (!this.knownSeries.has(seriesId) || !Number.isFinite(value)) {
          continue
        }
        const open = tier.open.get(seriesId)
        if (open && open.timestamp === start) {
          open.min = Math.min(open.min, value)
          open.max = Math.max(open.max, value)
          open.sum += value
          open.count += 1
          continue
        }
        if (open) {
          this.commit(tier, seriesId, open)
        }
        tier.open.set(seriesId, {
          timestamp: start,
          min: value,
          max: value,
          sum: value,
          count: 1
        })
      }
    }
  }

  /**
   * Buckets for one tier within `[fromMs, toMs]`, oldest first. The open
   * bucket is included so the newest point is always visible, and the oldest
   * completed bucket is dropped when that would exceed the tier capacity.
   */
  buckets(bucketSeconds: number, fromMs: number, toMs: number): MonitorTierSnapshot | null {
    const tier = this.tiers.find((candidate) => candidate.spec.bucketSeconds === bucketSeconds)
    if (!tier) {
      return null
    }
    const series: MonitorTierSeries[] = []
    for (const seriesId of this.knownSeries) {
      const ring = tier.rings.get(seriesId) ?? []
      const open = tier.open.get(seriesId)
      const includeOpen = open !== undefined && open.timestamp >= fromMs && open.timestamp <= toMs
      const buckets: MonitorBucket[] = []
      for (const bucket of ring) {
        if (bucket.timestamp >= fromMs && bucket.timestamp <= toMs) {
          buckets.push(bucket)
        }
      }
      if (includeOpen && buckets.length >= tier.spec.capacity) {
        buckets.splice(0, buckets.length - tier.spec.capacity + 1)
      }
      if (includeOpen && open) {
        buckets.push(this.toBucket(open))
      }
      series.push({ seriesId, buckets })
    }
    return { bucketSeconds, series }
  }

  /** Every tier within `[fromMs, toMs]`, finest first. */
  history(fromMs: number, toMs: number): MonitorHistorySnapshot {
    const tiers: MonitorTierSnapshot[] = []
    for (const tier of this.tiers) {
      const snapshot = this.buckets(tier.spec.bucketSeconds, fromMs, toMs)
      if (snapshot) {
        tiers.push(snapshot)
      }
    }
    return { tiers }
  }

  /** Oldest bucket timestamp retained across all tiers, or null when empty. */
  oldestTimestamp(): number | null {
    let oldest: number | null = null
    for (const tier of this.tiers) {
      for (const ring of tier.rings.values()) {
        const first = ring[0]
        if (first && (oldest === null || first.timestamp < oldest)) {
          oldest = first.timestamp
        }
      }
    }
    return oldest
  }

  private toBucket(open: OpenBucket): MonitorBucket {
    return {
      timestamp: open.timestamp,
      min: open.min,
      max: open.max,
      avg: open.sum / open.count,
      count: open.count
    }
  }

  private commit(tier: TierState, seriesId: string, open: OpenBucket): void {
    let ring = tier.rings.get(seriesId)
    if (!ring) {
      ring = []
      tier.rings.set(seriesId, ring)
    }
    ring.push(this.toBucket(open))
    if (ring.length > tier.spec.capacity) {
      ring.splice(0, ring.length - tier.spec.capacity)
    }
  }
}
