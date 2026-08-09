/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Number and money formatting only. Locale is fixed to `en-US` so digit grouping and the
 * currency symbol are stable across machines, which is what makes the Playwright literal
 * assertions deterministic. No calendar formatting lives here: `@/lib/localDate` owns the
 * `YYYY-MM-DD` strings and Rust owns every bucket boundary.
 */
const COUNT = new Intl.NumberFormat('en-US')

const COMPACT = new Intl.NumberFormat('en-US', {
  notation: 'compact',
  maximumFractionDigits: 1,
})

const MONEY = new Intl.NumberFormat('en-US', {
  style: 'currency',
  currency: 'USD',
  minimumFractionDigits: 4,
  maximumFractionDigits: 4,
})

export function formatCount(value: number): string {
  return COUNT.format(value)
}

export function formatCompact(value: number): string {
  return COMPACT.format(value)
}

export function formatMoney(value: number): string {
  return MONEY.format(value)
}
