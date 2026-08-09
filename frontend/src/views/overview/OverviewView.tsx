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
 * 趋势 IPC 一次返回总线与全部预聚合分组；切换分组只做本地筛选，不再触发查询扇出。
 */
import { useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'

import { useReportRange } from '@/app/reportRange'
import { ErrorState, LoadingState } from '@/components/app-state'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { AggregateFilters } from '@/generated'
import { zh } from '@/i18n/zh'
import { archiveQueryKey } from '@/lib/archiveQueries'
import { getSummary, getTrend, priceCatalogGet } from '@/lib/ipc'
import { missingPriceEntries, rangeMissingPriceEntries } from '@/views/overview/costMissing'
import { RangeSelector } from '@/views/overview/RangeSelector'
import { PRICE_CATALOG_QUERY_KEY } from '@/views/settings/usePriceOverrides'
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
  const summary = useQuery({
    queryKey: archiveQueryKey('overview', 'summary', range, timezone),
    queryFn: () => getSummary(range, timezone, NO_FILTERS),
  })

  const trend = useQuery({
    queryKey: archiveQueryKey('overview', 'trend', range, timezone, granularity),
    queryFn: () => getTrend(range, timezone, granularity, null),
  })

  /**
   * Names the models behind `部分缺失`, **on the same scope as the number next to it**.
   *
   * Derived from the trend response this view already holds: its `model` groups carry a
   * `CostTotals` per bucket over exactly the range and filters `get_summary` used, so summing
   * their `unavailableCount` decomposes `summary.cost.unavailableCount` instead of measuring
   * something else. No extra IPC call — and specifically not `get_breakdown`, which the
   * trend-grouping specs assert this view never issues.
   */
  const missingPrices = useMemo(
    () => rangeMissingPriceEntries(trend.data?.groups ?? []),
    [trend.data?.groups],
  )

  /**
   * Archive-wide fallback, used only when the trend groups name nothing.
   *
   * The query key is the one the settings editor already uses, imported rather than redeclared:
   * saving a price override invalidates that exact key, so a second key of its own would leave
   * this list asserting a model has no price right after the user gave it one.
   *
   * Not gated on `summary`: the catalog is small, offline and independent of the report range, so
   * it costs nothing to have ready and avoids a second round-trip when the badge appears.
   */
  const catalog = useQuery({ queryKey: PRICE_CATALOG_QUERY_KEY, queryFn: priceCatalogGet })

  const archiveMissingPrices = useMemo(
    () => missingPriceEntries(catalog.data?.observedModels ?? []),
    [catalog.data?.observedModels],
  )

  const split = useMemo(
    () => splitGroups(trendGroups(trend.data?.groups ?? [], groupMode)),
    [trend.data?.groups, groupMode],
  )

  const rows = useMemo(
    () => (trend.data === undefined ? [] : toTrendRows(trend.data.total)),
    [trend.data],
  )

  const parts: TrendGroupPart[] = split.kept.map((group) => ({
    group,
    rows: toTrendRows(group.series),
  }))

  const grouping: TrendGroupingState = {
    mode: groupMode,
    onModeChange: setGroupMode,
    parts,
    droppedCount: split.droppedCount,
    totalCount: split.kept.length + split.droppedCount,
    isPending: false,
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
        <SummaryCards
          summary={summary.data}
          missingPrices={missingPrices}
          archiveMissingPrices={archiveMissingPrices}
        />
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
          ) : (
            <TrendChart rows={rows} grouping={grouping} coverageNotes={trend.data.coverageNotes} />
          )}
        </CardContent>
      </Card>
    </section>
  )
}
