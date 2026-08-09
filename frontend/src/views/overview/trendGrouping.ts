/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Grouping math for the trend chart. No React, no IPC, no i18n, so the two decisions that
 * actually needed thinking about are assertable on their own:
 *
 * 1. **Where the group list comes from.** `get_trend` 一次返回后端预聚合的 source / agent /
 *    provider / model 趋势。这里仅按维度筛选和排序，切换按钮不再产生 IPC 或 SQL。
 *
 * 2. **Why there is a Top-N cut.** A real archive has dozens of models; a chart with forty
 *    lines conveys less than no chart, because the reader cannot attach a line to a legend
 *    entry. {@link TREND_GROUP_LIMIT} is set to the number of visually separable hues in the
 *    `--series-*` palette, and everything past it is folded into one 其他 line so the total
 *    still reconciles. 其他 is computed as `total − Σ kept` rather than by fetching the tail:
 *    the tail can be hundreds of groups, and the ungrouped total is already on screen.
 *
 * Coverage is deliberately taken from the ungrouped total series only. Coverage answers "does
 * the archive cover this window", which is a property of the window and not of a model, so a
 * per-group coverage state would invite the reader to conclude a model was idle when the truth
 * is that nothing was archived. When the total says `none`, every group value is `null` and the
 * lines break — never 0.
 *
 * 3. **Why legend selection filters `series` and not `rows`.** Selecting one legend entry hides
 *    the other lines but must not change a single plotted value, and above all must not change
 *    the coverage band: the band comes from the total series, so it stays whatever the window
 *    says. Selection is therefore applied to the series list only — `rows` keep every group's
 *    values, so the tooltip totals and the 其他 remainder are identical selected or not.
 */
import type { CoverageStatus, SeriesGroup, SeriesGroupDimension, SeriesPoint } from '@/generated'

import { totalTokens, type TrendMetric, type TrendRow } from '@/views/overview/trendModel'

export const TREND_GROUP_MODES = ['none', 'model', 'agent', 'tool'] as const

export type TrendGroupMode = (typeof TREND_GROUP_MODES)[number]

/** Length of `SERIES_PALETTE`; `--series-7` is held back for the 其他 line. */
export const TREND_GROUP_LIMIT = 6

export interface TrendGroup {
  id: string
  label: string
  series: SeriesPoint[]
  /** Token total over the whole window, used only for ranking. */
  weight: number
}

export interface TrendGroupPart {
  group: TrendGroup
  rows: TrendRow[]
}

export interface TrendGroupSeries {
  /** Index-derived, so a model id containing a dot is never read as a recharts key path. */
  key: string
  label: string
  color: string
  isOther: boolean
}

export interface GroupedTrendRow {
  label: string
  coverage: CoverageStatus
  coverageBand: number | null
  values: Record<string, number | null>
}

export interface GroupedTrend {
  series: TrendGroupSeries[]
  rows: GroupedTrendRow[]
  axisMax: number
}

const GROUP_DIMENSION: Record<Exclude<TrendGroupMode, 'none'>, SeriesGroupDimension> = {
  tool: 'source',
  agent: 'agent',
  model: 'model',
}

/**
 * Dimension values present in `rows`, heaviest first.
 *
 * Ties break on `id` so the ordering — and therefore the colour each group gets — is stable
 * across refetches instead of following `Map` insertion order.
 */
export function trendGroups(groups: readonly SeriesGroup[], mode: TrendGroupMode): TrendGroup[] {
  if (mode === 'none') return []
  return groups
    .filter((group) => group.dimension === GROUP_DIMENSION[mode])
    .map((group) => ({
      id: group.id,
      label: group.label,
      series: group.series,
      weight: group.series.reduce(
        (sum, point) => sum + (point.tokens === null ? 0 : totalTokens(point.tokens)),
        0,
      ),
    }))
    .filter((group) => group.weight > 0)
    .sort((left, right) =>
      right.weight !== left.weight ? right.weight - left.weight : left.id.localeCompare(right.id),
    )
}

export interface TrendGroupSplit {
  kept: TrendGroup[]
  droppedCount: number
}

export function splitGroups(
  groups: readonly TrendGroup[],
  limit: number = TREND_GROUP_LIMIT,
): TrendGroupSplit {
  const safeLimit = Math.max(limit, 0)
  return {
    kept: groups.slice(0, safeLimit),
    droppedCount: Math.max(groups.length - safeLimit, 0),
  }
}

/** Grouped mode plots one line per group; cost uses `actualSum` only (see `groupCostSeriesHint`). */
function metricValue(row: TrendRow | undefined, metric: TrendMetric): number | null {
  if (row === undefined) return null
  return metric === 'cost' ? row.actualValue : row.tokensValue
}

export const OTHER_SERIES_KEY = 'gother'

/**
 * Narrows a grouped chart to one legend entry. An unknown or `null` key means "show all", so a
 * selection that survives a group-mode switch (whose keys are index-derived and get reused for a
 * different label) degrades to the full view instead of silently plotting the wrong line.
 */
export function visibleSeries(
  series: readonly TrendGroupSeries[],
  selectedKey: string | null,
): TrendGroupSeries[] {
  if (selectedKey === null) return [...series]
  const selected = series.filter((entry) => entry.key === selectedKey)
  return selected.length === 0 ? [...series] : selected
}

export function groupedSeriesTotal(rows: readonly GroupedTrendRow[], key: string): number | null {
  let total = 0
  let present = false
  for (const row of rows) {
    const value = row.values[key]
    if (value === null || value === undefined) continue
    present = true
    total += value
  }
  return present ? total : null
}

export function buildGroupedTrend({
  total,
  parts,
  metric,
  droppedCount,
  palette,
  otherColor,
  otherLabel,
}: {
  total: readonly TrendRow[]
  parts: readonly TrendGroupPart[]
  metric: TrendMetric
  droppedCount: number
  palette: readonly string[]
  otherColor: string
  otherLabel: string
}): GroupedTrend {
  const includeOther = droppedCount > 0
  const series: TrendGroupSeries[] = parts.map((part, index) => ({
    key: `g${index}`,
    label: part.group.label,
    color: palette[index % palette.length],
    isOther: false,
  }))
  if (includeOther) {
    series.push({ key: OTHER_SERIES_KEY, label: otherLabel, color: otherColor, isOther: true })
  }

  const byLabel = parts.map((part) => new Map(part.rows.map((row) => [row.label, row])))

  let axisMax = 0
  const rows = total.map((totalRow) => {
    const values: Record<string, number | null> = {}
    if (totalRow.coverage === 'none') {
      for (const entry of series) values[entry.key] = null
      return { label: totalRow.label, coverage: totalRow.coverage, coverageBand: 1, values }
    }

    let keptSum = 0
    parts.forEach((_part, index) => {
      const value = metricValue(byLabel[index].get(totalRow.label), metric) ?? 0
      values[`g${index}`] = value
      keptSum += value
      if (value > axisMax) axisMax = value
    })
    if (includeOther) {
      // Clamped: float cost sums and an eventually-consistent breakdown can both make the
      // remainder marginally negative, and a negative "其他" would be a lie about the data.
      const other = Math.max((metricValue(totalRow, metric) ?? 0) - keptSum, 0)
      values[OTHER_SERIES_KEY] = other
      if (other > axisMax) axisMax = other
    }
    return {
      label: totalRow.label,
      coverage: totalRow.coverage,
      coverageBand: totalRow.coverage === 'full' ? null : 1,
      values,
    }
  })

  return { series, rows, axisMax: axisMax > 0 ? axisMax : 1 }
}
