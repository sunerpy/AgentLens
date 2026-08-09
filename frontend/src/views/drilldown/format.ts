/**
 * EXCLUSIVE FILE BOUNDARY — todo 16 owns `src/views/drilldown/**`.
 *
 * Display formatting only. Amounts are rendered with four decimals because the archive's
 * per-message costs are of the order of 1e-3 USD; two decimals would render most rows as
 * `$0.00` and hide the difference between "cheap" and "free".
 */
const INTEGER_FORMAT = new Intl.NumberFormat('en-US')

export function formatCount(value: number): string {
  return INTEGER_FORMAT.format(value)
}

export function formatAmount(value: number): string {
  return `$${value.toFixed(4)}`
}

export function formatShare(share: number): string {
  return `${(share * 100).toFixed(1)}%`
}

export function sharePercent(share: number): number {
  return Math.round(share * 1000) / 10
}
