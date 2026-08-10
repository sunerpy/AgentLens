/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Turns the backend's `coverageNotes` into a per-bucket lookup, so the chart can answer *why* a
 * bucket is 部分覆盖 instead of only stating that it is.
 *
 * The backend's Partial rule is "not every selected (host_id, source) pair covers the whole
 * bucket". That makes Partial common as soon as a user enables a second source: any pair with no
 * archived interval in that bucket flips the whole bucket. Naming the pairs is therefore the
 * whole point — a bare 部分覆盖 badge is indistinguishable from a defect.
 *
 * Keyed by bucket label rather than index: the label is what `SeriesPoint.bucket.label` already
 * carries and what the tooltip already looks rows up by, so no second alignment can drift.
 *
 * An *unfinished* bucket is the third case, and the only reason this module knows about `now`.
 * A collection archives the interval `[since, now]`, while the backend's Full rule needs one
 * interval that covers the whole bucket — so the bucket containing the present instant can never
 * be Full, for any source, on any day, no matter how healthy the collection is. Naming its pairs
 * as a coverage gap is therefore permanently wrong: it fires on every refresh, for every enabled
 * source, and it buries the finished buckets whose gaps are real defects.
 *
 * `nowMs` is compared against `TimeBucket`'s absolute epoch edges, which Rust already resolved
 * with `chrono_tz` for the report timezone. Comparing two instants needs no calendar arithmetic
 * and no timezone knowledge here, so the test is exact for hour / day / week / month buckets and
 * for every report timezone — the frontend still derives no boundary and no label of its own.
 */
import type { CoverageNote, CoverageShortfall } from '@/generated'

export interface CoverageReason {
  /** Pairs whose archived intervals only partly overlap the bucket. */
  partial: CoverageShortfall[]
  /** Pairs with no archived interval inside the bucket at all. */
  missing: CoverageShortfall[]
}

export type CoverageReasonIndex = ReadonlyMap<string, CoverageReason>

/**
 * Splits each note's shortfalls into the two causes the UI words differently.
 *
 * A note whose shortfalls are all filtered away is dropped rather than kept as an empty reason,
 * so `has(label)` stays equivalent to "there is something to say".
 */
export function coverageReasonIndex(notes: readonly CoverageNote[]): CoverageReasonIndex {
  const index = new Map<string, CoverageReason>()
  for (const note of notes) {
    const partial = note.shortfalls.filter((shortfall) => shortfall.partial)
    const missing = note.shortfalls.filter((shortfall) => !shortfall.partial)
    if (partial.length === 0 && missing.length === 0) continue
    index.set(note.label, { partial, missing })
  }
  return index
}

export function coverageReasonFor(
  index: CoverageReasonIndex,
  label: string,
): CoverageReason | null {
  return index.get(label) ?? null
}

/** Ordered for display: partly-covered pairs first, since they are the actionable ones. */
export function coverageReasonPairs(reason: CoverageReason): CoverageShortfall[] {
  return [...reason.partial, ...reason.missing]
}

/** The absolute bucket edges the phase test needs, structurally satisfied by `TrendRow`. */
export interface BucketWindow {
  label: string
  startUtcMs: number
  endUtcMs: number
}

/**
 * Whether the present instant still falls inside the bucket.
 *
 * Half-open, matching the backend's own `[start, end)` buckets: a bucket whose right edge is
 * exactly `nowMs` has ended and is judged as history. A bucket entirely in the future is not "in
 * progress" either — no interval intersects it, so the backend reports `none`, which is a
 * different statement and keeps its own wording.
 */
export function isBucketInProgress(bucket: BucketWindow, nowMs: number): boolean {
  return nowMs >= bucket.startUtcMs && nowMs < bucket.endUtcMs
}

export interface CoverageExplanation {
  /** The bucket has not ended, so incomplete coverage is expected here rather than a defect. */
  inProgress: boolean
  /** Pairs worth naming; empty once `inProgress` accounts for every shortfall. */
  pairs: CoverageShortfall[]
  /** The archive left nothing that explains this bucket. */
  unknown: boolean
}

/**
 * What the UI is entitled to say about one non-`full` bucket.
 *
 * For an unfinished bucket the `partial` shortfalls are dropped: they are the arithmetic
 * consequence of `[since, now]` stopping inside the bucket. A `missing` pair survives — it has no
 * interval anywhere in the bucket, i.e. that host/source has collected nothing this period, which
 * "the period is not over" does not explain.
 */
export function coverageExplanationFor(
  index: CoverageReasonIndex,
  bucket: BucketWindow,
  nowMs: number,
): CoverageExplanation {
  const inProgress = isBucketInProgress(bucket, nowMs)
  const reason = coverageReasonFor(index, bucket.label)
  if (reason === null) {
    // Calling an unfinished bucket "undiagnosable" would be a lie: it is fully diagnosed.
    return { inProgress, pairs: [], unknown: !inProgress }
  }
  return {
    inProgress,
    pairs: inProgress ? [...reason.missing] : coverageReasonPairs(reason),
    unknown: false,
  }
}

/** Whether the always-visible gap panel has anything to report for this bucket. */
export function hasCoverageGap(explanation: CoverageExplanation): boolean {
  return explanation.pairs.length > 0 || explanation.unknown
}
