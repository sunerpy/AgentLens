/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**` and the `zh.overview`
 * dictionary section. No other worker edits this directory; this worker edits no shell file.
 *
 * Shared infrastructure this view builds on (never reimplemented here):
 * - `@/lib/ipc` — typed `invoke` wrappers + `toIpcError`
 * - `@/app/reportRange` — `useReportRange()` for the shared range / timezone / granularity
 * - `@/components/app-state` — `LoadingState` / `EmptyState` / `ErrorState`
 * - `@/i18n/zh` — every user-visible string (`scripts/check-i18n.mjs` enforces this)
 */
import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'

import { useReportRange } from '@/app/reportRange'
import { ErrorState, LoadingState } from '@/components/app-state'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { AggregateFilters } from '@/generated'
import { zh } from '@/i18n/zh'
import { archiveQueryKey } from '@/lib/archiveQueries'
import { getSummary, getTrend } from '@/lib/ipc'
import { RangeSelector } from '@/views/overview/RangeSelector'
import { SummaryCards } from '@/views/overview/SummaryCards'
import { TrendChart } from '@/views/overview/TrendChart'
import { toTrendRows } from '@/views/overview/trendModel'

/** The overview is unfiltered by design; drill-down filtering is todo 16's surface. */
const NO_FILTERS: AggregateFilters = {
  hostId: null,
  source: null,
  agentKey: null,
  providerId: null,
  modelId: null,
}

export function OverviewView() {
  const { range, timezone, granularity } = useReportRange()

  const summary = useQuery({
    queryKey: archiveQueryKey('overview', 'summary', range, timezone),
    queryFn: () => getSummary(range, timezone, NO_FILTERS),
  })

  const trend = useQuery({
    queryKey: archiveQueryKey('overview', 'trend', range, timezone, granularity),
    queryFn: () => getTrend(range, timezone, granularity, null),
  })

  const rows = useMemo(
    () => (trend.data === undefined ? [] : toTrendRows(trend.data)),
    [trend.data],
  )

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
          ) : (
            <TrendChart rows={rows} />
          )}
        </CardContent>
      </Card>
    </section>
  )
}
