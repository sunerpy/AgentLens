/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * recharts writes colours straight onto SVG attributes, so it cannot consume Tailwind
 * utility classes. Every value below is a `var(--token)` reference into the design system
 * declared in `src/index.css`; there is no hard-coded colour anywhere in this view.
 */
export const CHART_TOKENS = {
  grid: 'var(--border)',
  axis: 'var(--muted-foreground)',
  surface: 'var(--card)',
  seriesTokens: 'var(--chart-5)',
  seriesActual: 'var(--chart-4)',
  seriesEstimated: 'var(--chart-2)',
  coverageGap: 'var(--muted-foreground)',
  coveragePartial: 'var(--chart-1)',
} as const

/**
 * Categorical palette for the grouped trend lines, in the order groups are assigned.
 *
 * Deliberately NOT the `--chart-*` ramp: `--chart-1` is already spoken for by the
 * partial-coverage hatch, so a grouped line drawn in it would read as a coverage marker.
 *
 * Length is the reason `TREND_GROUP_LIMIT` is 6 — six hues that stay separable side by side in
 * a legend. `--series-7` is a low-chroma tone held back for the 其他 line, so the aggregate
 * never competes with a named group for attention.
 */
export const SERIES_PALETTE = [
  'var(--series-1)',
  'var(--series-2)',
  'var(--series-3)',
  'var(--series-4)',
  'var(--series-5)',
  'var(--series-6)',
] as const

export const OTHER_SERIES_COLOR = 'var(--series-7)'

export const GAP_PATTERN_ID = 'overview-coverage-gap-hatch'
export const PARTIAL_PATTERN_ID = 'overview-coverage-partial-hatch'
