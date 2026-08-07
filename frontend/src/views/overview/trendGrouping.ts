/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Grouping math for the trend chart. No React, no IPC, no i18n, so the two decisions that
 * actually needed thinking about are assertable on their own:
 *
 * 1. **Where the group list comes from.** `get_breakdown` over the same window already returns
 *    `source` / `agentKey` / `(providerId, modelId)` on every row, so the dimension values and
 *    their token weights are read from that one call. Nothing here re-derives an aggregate the
 *    backend already computes, and each group's series is fetched with the existing
 *    `get_trend(filters)` contract — no new aggregation path exists.
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
 */
import type { AggregateFilters, BreakdownRow, CoverageStatus } from '@/generated'

import { totalTokens, type TrendMetric, type TrendRow } from '@/views/overview/trendModel'

export const TREND_GROUP_MODES = ['none', 'model', 'agent', 'tool'] as const

export type TrendGroupMode = (typeof TREND_GROUP_MODES)[number]

/** Length of `SERIES_PALETTE`; `--series-7` is held back for the 其他 line. */
export const TREND_GROUP_LIMIT = 6

export interface TrendGroup {
  /** Stable identity across refetches; also the react-query key segment. */
  id: string
  label: string
  filters: AggregateFilters
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

const NO_FILTERS: AggregateFilters = {
  hostId: null,
  source: null,
  agentKey: null,
  providerId: null,
  modelId: null,
}

/** `\u0000` cannot occur in a provider or model id, so the composite key is collision-free. */
function modelId(providerId: string, model: string): string {
  return `${providerId}\u0000${model}`
}

interface Bucket {
  label: string
  filters: AggregateFilters
  weight: number
}

function dimensionOf(
  row: BreakdownRow,
  mode: Exclude<TrendGroupMode, 'none'>,
): Bucket & {
  id: string
} {
  switch (mode) {
    case 'tool':
      return {
        id: row.source,
        label: row.source,
        filters: { ...NO_FILTERS, source: row.source },
        weight: 0,
      }
    case 'agent':
      return {
        id: row.agentKey,
        label: row.agentRaw,
        filters: { ...NO_FILTERS, agentKey: row.agentKey },
        weight: 0,
      }
    case 'model':
      return {
        id: modelId(row.providerId, row.modelId),
        label: `${row.providerId} / ${row.modelId}`,
        filters: { ...NO_FILTERS, providerId: row.providerId, modelId: row.modelId },
        weight: 0,
      }
  }
}

/**
 * Dimension values present in `rows`, heaviest first.
 *
 * Ties break on `id` so the ordering — and therefore the colour each group gets — is stable
 * across refetches instead of following `Map` insertion order.
 */
export function trendGroups(rows: readonly BreakdownRow[], mode: TrendGroupMode): TrendGroup[] {
  if (mode === 'none') return []
  const buckets = new Map<string, Bucket>()
  for (const row of rows) {
    const { id, label, filters } = dimensionOf(row, mode)
    const existing = buckets.get(id)
    const weight = (existing?.weight ?? 0) + totalTokens(row.tokens)
    // The label follows the last row observed in backend order: `agentRaw` is display-only and
    // two rows of one `agentKey` can carry different raw labels.
    buckets.set(id, { label, filters, weight })
  }
  return [...buckets]
    .map(([id, bucket]) => ({ id, ...bucket }))
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
