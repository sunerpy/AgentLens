/**
 * EXCLUSIVE FILE BOUNDARY — todo 17 owns `src/views/detail/**`.
 *
 * Presentation formatters only. There is deliberately **no calendar arithmetic** here and no
 * date library (`date-fns` / `dayjs` / `moment` are forbidden by the plan): bucket boundaries,
 * DST folds and week starts are all decided in Rust (`agentlens_core::query`). The single
 * timezone-aware operation below is `Intl.DateTimeFormat`, which renders an epoch instant in
 * the report timezone without ever deriving a new instant from it.
 */
import type { DetailCost, TokenValues } from '@/generated'

const NUMBER_FORMAT = new Intl.NumberFormat('en-US')

/**
 * `sv-SE` is used purely for its CLDR short-date pattern (`YYYY-MM-DD HH:mm:ss`), which keeps
 * timestamps sortable by eye and locale-neutral; the locale carries no user-visible words.
 */
function timestampFormatter(timezone: string): Intl.DateTimeFormat {
  const options: Intl.DateTimeFormatOptions = {
    timeZone: timezone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  }
  try {
    return new Intl.DateTimeFormat('sv-SE', options)
  } catch {
    return new Intl.DateTimeFormat('sv-SE', { ...options, timeZone: 'UTC' })
  }
}

const formatterCache = new Map<string, Intl.DateTimeFormat>()

export function formatTimestamp(epochMs: number | null | undefined, timezone: string): string {
  if (typeof epochMs !== 'number' || !Number.isFinite(epochMs)) return '—'
  let formatter = formatterCache.get(timezone)
  if (formatter === undefined) {
    formatter = timestampFormatter(timezone)
    formatterCache.set(timezone, formatter)
  }
  return formatter.format(new Date(epochMs))
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
