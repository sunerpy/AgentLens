/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * `SeriesPoint[]` → chart rows. This module is where the two semantics the plan spent
 * three review rounds on are enforced, so they are testable independently of recharts:
 *
 * 1. A coverage gap is NOT zero. `coverage === 'none'` carries `null` payloads and every
 *    plotted series value stays `null`, which is what makes the line break. A bucket with
 *    `coverage === 'full'` and zero tokens is real idleness and yields a literal `0`.
 * 2. Cost is three-way separated. `actualSum` and `estimatedSum` are two independent
 *    series and are never added together; `unavailableCount` is a count and never becomes
 *    a money value, not even `0`.
 */
import type { CostTotals, SeriesPoint, TokenValues } from '@/generated'

export const TREND_METRICS = ['tokens', 'cost'] as const

export type TrendMetric = (typeof TREND_METRICS)[number]

export type TrendSeriesKey = 'tokens' | 'actual' | 'estimated'

export interface TrendRow {
  label: string
  startUtcMs: number
  /**
   * Bucket right edge, exclusive — carried so the UI can tell an *unfinished* bucket from a
   * historical one without ever computing a boundary itself. Rust already resolved both edges
   * with `chrono_tz` for the report timezone; these are absolute epoch instants.
   */
  endUtcMs: number
  coverage: SeriesPoint['coverage']
  tokens: TokenValues | null
  cost: CostTotals | null
  messageCount: number | null
  sessionRecordCount: number | null
  tokensValue: number | null
  actualValue: number | null
  estimatedValue: number | null
  /** Drives the full-height coverage band; `null` for `full` buckets so no band is drawn. */
  coverageBand: number | null
}

/**
 * Sum of the five atomic buckets. Deliberately not `totalInput`, which is the derived
 * `input + cacheRead + cacheWrite` figure and would double-count the cache buckets.
 */
export function totalTokens(tokens: TokenValues): number {
  return (
    tokens.tokInput +
    tokens.tokOutput +
    tokens.tokReasoning +
    tokens.tokCacheRead +
    tokens.tokCacheWrite
  )
}

/** Display grouping shared with the summary card: cache is read + write. */
export function cacheTokens(tokens: TokenValues): number {
  return tokens.tokCacheRead + tokens.tokCacheWrite
}

export function toTrendRows(points: SeriesPoint[]): TrendRow[] {
  return points.map((point) => {
    const covered = point.coverage !== 'none'
    return {
      label: point.bucket.label,
      startUtcMs: point.bucket.startUtcMs,
      endUtcMs: point.bucket.endUtcMs,
      coverage: point.coverage,
      tokens: point.tokens,
      cost: point.cost,
      messageCount: point.messageCount,
      sessionRecordCount: point.sessionRecordCount,
      tokensValue: covered && point.tokens !== null ? totalTokens(point.tokens) : null,
      actualValue: covered && point.cost !== null ? point.cost.actualSum : null,
      estimatedValue: covered && point.cost !== null ? point.cost.estimatedSum : null,
      coverageBand: point.coverage === 'full' ? null : 1,
    }
  })
}

export function seriesKeysFor(metric: TrendMetric): TrendSeriesKey[] {
  return metric === 'tokens' ? ['tokens'] : ['actual', 'estimated']
}

export function rowValue(row: TrendRow, key: TrendSeriesKey): number | null {
  switch (key) {
    case 'tokens':
      return row.tokensValue
    case 'actual':
      return row.actualValue
    case 'estimated':
      return row.estimatedValue
  }
}

/**
 * Upper bound for the value axis. Returns `1` when nothing is plottable (every bucket
 * uncovered, or a genuinely all-zero range) so recharts never has to resolve an empty
 * domain, which would otherwise produce NaN geometry.
 */
export function valueAxisMax(rows: TrendRow[], metric: TrendMetric): number {
  const keys = seriesKeysFor(metric)
  let max = 0
  for (const row of rows) {
    for (const key of keys) {
      const value = rowValue(row, key)
      if (value !== null && value > max) max = value
    }
  }
  return max > 0 ? max : 1
}

export function hasAnyCoverage(rows: TrendRow[]): boolean {
  return rows.some((row) => row.coverage !== 'none')
}

export function unavailableCount(row: TrendRow): number {
  return row.cost?.unavailableCount ?? 0
}
