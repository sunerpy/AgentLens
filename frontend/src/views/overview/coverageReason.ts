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
