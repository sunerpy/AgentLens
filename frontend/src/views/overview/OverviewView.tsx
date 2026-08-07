/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**` and the `zh.overview`
 * dictionary section. No other worker edits this directory; this worker edits no shell file.
 *
 * Shared infrastructure this view builds on (never reimplemented here):
 * - `@/lib/ipc` — typed `invoke` wrappers + `toIpcError`
 * - `@/app/reportRange` — `useReportRange()` for the shared range / timezone / granularity
 * - `@/components/app-state` — `LoadingState` / `EmptyState` / `ErrorState`
 * - `@/i18n/zh` — every user-visible string (`scripts/check-i18n.mjs` enforces this)
 *
 * Grouped trend fan-out: one `get_breakdown` names the dimension values in the window, then one
 * `get_trend(filters)` per kept group. Both commands already exist — the grouping adds no new
 * aggregation path. The fan-out is bounded by `TREND_GROUP_LIMIT` (6), so the worst case is
 * 1 + 6 extra reads, and it only runs while a grouped mode is selected: in 不分组 the breakdown
 * query is disabled and the request pattern is byte-for-byte what it was before.
 */
import { useMemo, useState } from 'react'
import { useQueries, useQuery } from '@tanstack/react-query'

import { useReportRange } from '@/app/reportRange'
import { ErrorState, LoadingState } from '@/components/app-state'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { AggregateFilters, BreakdownDimensions } from '@/generated'
import { zh } from '@/i18n/zh'
import { archiveQueryKey } from '@/lib/archiveQueries'
import { getBreakdown, getSummary, getTrend } from '@/lib/ipc'
import { RangeSelector } from '@/views/overview/RangeSelector'
import { SummaryCards } from '@/views/overview/SummaryCards'
import { TrendChart, type TrendGroupingState } from '@/views/overview/TrendChart'
import {
  splitGroups,
  trendGroups,
  type TrendGroupMode,
  type TrendGroupPart,
} from '@/views/overview/trendGrouping'
import { toTrendRows } from '@/views/overview/trendModel'

/** The overview total is unfiltered by design; drill-down filtering is todo 16's surface. */
const NO_FILTERS: AggregateFilters = {
  hostId: null,
  source: null,
  agentKey: null,
  providerId: null,
  modelId: null,
}

export function OverviewView() {
  const { range, timezone, granularity } = useReportRange()
  const [groupMode, setGroupMode] = useState<TrendGroupMode>('none')
  const grouped = groupMode !== 'none'

  const summary = useQuery({
    queryKey: archiveQueryKey('overview', 'summary', range, timezone),
    queryFn: () => getSummary(range, timezone, NO_FILTERS),
  })

  const trend = useQuery({
    queryKey: archiveQueryKey('overview', 'trend', range, timezone, granularity),
    queryFn: () => getTrend(range, timezone, granularity, null),
  })

  /**
   * `expandVariant: false`: variants are a reasoning-effort dimension of one model, and every
   * grouping offered here is coarser than that, so expanding would only multiply rows that get
   * folded straight back together.
   */
  const breakdown = useQuery({
    queryKey: archiveQueryKey('overview', 'groupDims', range, timezone),
    queryFn: () => {
      const dims: BreakdownDimensions = {
        timezone,
        filters: NO_FILTERS,
        expandVariant: false,
      }
      return getBreakdown(range, dims)
    },
    enabled: grouped,
  })

  const split = useMemo(
    () => splitGroups(trendGroups(breakdown.data ?? [], groupMode)),
    [breakdown.data, groupMode],
  )

  const groupTrends = useQueries({
    queries: split.kept.map((group) => ({
      queryKey: archiveQueryKey(
        'overview',
        'groupTrend',
        range,
        timezone,
        granularity,
        groupMode,
        group.id,
      ),
      queryFn: () => getTrend(range, timezone, granularity, group.filters),
    })),
  })

  const rows = useMemo(
    () => (trend.data === undefined ? [] : toTrendRows(trend.data)),
    [trend.data],
  )

  /**
   * Deliberately not memoised: `useQueries` returns a fresh array every render, so any dep list
   * covering it would have to be spread — and re-deriving ≤ 6 groups × ≤ 31 buckets is cheaper
   * than the bug that a hand-maintained dep list invites.
   */
  const parts: TrendGroupPart[] = split.kept.map((group, index) => ({
    group,
    rows: toTrendRows(groupTrends[index]?.data ?? []),
  }))

  const groupError = grouped
    ? (breakdown.error ?? groupTrends.find((query) => query.error !== null)?.error ?? null)
    : null

  const grouping: TrendGroupingState = {
    mode: groupMode,
    onModeChange: setGroupMode,
    parts,
    droppedCount: split.droppedCount,
    totalCount: split.kept.length + split.droppedCount,
    isPending: grouped && (breakdown.isPending || groupTrends.some((query) => query.isPending)),
  }

  return (
    <section data-testid="view-overview" className="flex flex-col gap-6">
      <div className="flex flex-col gap-1">
        <h2 className="font-heading text-2xl font-semibold tracking-tight">{zh.overview.title}</h2>
        <p className="text-sm text-muted-foreground">{zh.overview.subtitle}</p>
      </div>

      <RangeSelector />

      {summary.isPending ? (
        <LoadingState />
      ) : summary.isError ? (
        <ErrorState error={summary.error} onRetry={() => void summary.refetch()} />
      ) : (
        <SummaryCards summary={summary.data} />
      )}

      <Card>
        <CardHeader>
          <CardTitle>{zh.overview.trend.title}</CardTitle>
          <CardDescription>{zh.overview.range.halfOpenHint}</CardDescription>
        </CardHeader>
        <CardContent>
          {trend.isPending ? (
            <LoadingState />
          ) : trend.isError ? (
            <ErrorState error={trend.error} onRetry={() => void trend.refetch()} />
          ) : groupError !== null ? (
            <ErrorState error={groupError} onRetry={() => void breakdown.refetch()} />
          ) : (
            <TrendChart rows={rows} grouping={grouping} />
          )}
        </CardContent>
      </Card>
    </section>
  )
}
