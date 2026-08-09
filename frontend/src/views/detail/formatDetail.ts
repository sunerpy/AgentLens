/**
 * EXCLUSIVE FILE BOUNDARY — todo 17 owns `src/views/detail/**`.
 *
 * Presentation formatters only. There is deliberately **no calendar arithmetic** here and no
 * date library (`date-fns` / `dayjs` / `moment` are forbidden by the plan): bucket boundaries,
 * DST folds and week starts are all decided in Rust (`agentlens_core::query`). Rendering an
 * instant in the report timezone is delegated to `@/lib/localDate`, the single formatter the
 * whole UI shares, so a detail row's clock cannot drift from a host row's or a log record's.
 */
import type { DetailCost, TokenValues } from '@/generated'
import { formatInstantInZone } from '@/lib/localDate'

const NUMBER_FORMAT = new Intl.NumberFormat('en-US')

/** An em dash rather than a blank cell: "no timestamp" must be visible, not inferred. */
export function formatTimestamp(epochMs: number | null | undefined, timezone: string): string {
  return formatInstantInZone(epochMs, timezone) ?? '—'
}

export function formatCount(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '—'
  return NUMBER_FORMAT.format(value)
}

/** Money keeps 4-6 fraction digits: real per-message costs sit around 0.0004. */
export function formatMoney(value: number): string {
  return new Intl.NumberFormat('en-US', {
    minimumFractionDigits: 4,
    maximumFractionDigits: 6,
  }).format(value)
}

export type CostKind = 'actual' | 'estimated' | 'unavailable'

export interface ResolvedDetailCost {
  kind: CostKind
  amount: number | null
}

/**
 * Mirrors `agentlens_core::pricing::ResolvedCost`: `unavailable` is a real third state and must
 * never be rendered as the number 0. A row whose cost object is missing or carries no finite
 * amount is also `unavailable` rather than free.
 */
export function resolveDetailCost(cost: DetailCost | null | undefined): ResolvedDetailCost {
  if (cost === null || cost === undefined || cost.unavailable) {
    return { kind: 'unavailable', amount: null }
  }
  if (typeof cost.actual === 'number' && Number.isFinite(cost.actual)) {
    return { kind: 'actual', amount: cost.actual }
  }
  if (typeof cost.estimated === 'number' && Number.isFinite(cost.estimated)) {
    return { kind: 'estimated', amount: cost.estimated }
  }
  return { kind: 'unavailable', amount: null }
}

function bucket(value: number | undefined): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

/**
 * The four displayed token columns. `cache` is the sum of the two atomic cache buckets, which
 * are still reported separately by `cacheRead` / `cacheWrite` so no atomic value is lost.
 * `tokens.total` from the source data is never read (its semantics differ from `totalInput`).
 */
export interface DisplayTokens {
  input: number
  output: number
  reasoning: number
  cache: number
  cacheRead: number
  cacheWrite: number
}

export function displayTokens(tokens: TokenValues | null | undefined): DisplayTokens {
  const cacheRead = bucket(tokens?.tokCacheRead)
  const cacheWrite = bucket(tokens?.tokCacheWrite)
  return {
    input: bucket(tokens?.tokInput),
    output: bucket(tokens?.tokOutput),
    reasoning: bucket(tokens?.tokReasoning),
    cache: cacheRead + cacheWrite,
    cacheRead,
    cacheWrite,
  }
}
