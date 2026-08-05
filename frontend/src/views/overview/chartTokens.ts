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

export const GAP_PATTERN_ID = 'overview-coverage-gap-hatch'
export const PARTIAL_PATTERN_ID = 'overview-coverage-partial-hatch'
