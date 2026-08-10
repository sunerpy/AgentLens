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

const PERCENT = new Intl.NumberFormat('en-US', {
  style: 'percent',
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
})

/** Smallest and largest share the two-decimal format can state without over-claiming. */
const SHARE_FLOOR = 0.0001
const SHARE_CEILING = 0.9999

/**
 * A `0..1` share as a percentage.
 *
 * The two guards exist because rounding at the extremes would state something false, and this
 * number is the card's whole argument for why two amounts are not comparable. A tier holding
 * 20.3M of 69.9B tokens rounds to `0.00%`, which reads as "covers nothing" when it covers 117
 * real records; its counterpart rounds to `100.00%`, which reads as "covers everything" when it
 * does not. So both extremes are rendered as bounds instead.
 */
export function formatShare(value: number): string {
  if (value > 0 && value < SHARE_FLOOR) return `<${PERCENT.format(SHARE_FLOOR)}`
  if (value < 1 && value > SHARE_CEILING) return `>${PERCENT.format(SHARE_CEILING)}`
  return PERCENT.format(value)
}
