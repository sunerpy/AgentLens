/**
 * EXCLUSIVE FILE BOUNDARY — todo 17 owns `src/views/detail/**` and the `zh.detail` dictionary
 * section. No other worker edits this directory; this worker edits no shell file.
 *
 * Shared infrastructure to build on (do not reimplement):
 * - `@/lib/ipc` — typed `invoke` wrappers + `toIpcError`
 * - `@/app/reportRange` — `useReportRange()` for the shared range / timezone / granularity
 * - `@/components/app-state` — `LoadingState` / `EmptyState` / `ErrorState`
 * - `@/i18n/zh` — every user-visible string (`scripts/check-i18n.mjs` enforces this)
 */
import { EmptyState, ErrorState, LoadingState } from '@/components/app-state'
import { zh } from '@/i18n/zh'

import { DetailFilterBar } from './DetailFilterBar'
import { DetailPager } from './DetailPager'
import { DetailTable } from './DetailTable'
import { useDetailPage } from './useDetailPage'

export function DetailView() {
  const {
    filters,
    offset,
    pageSize,
    page,
    isPending,
    isFetching,
    error,
    refetch,
    dispatch,
    timezone,
  } = useDetailPage()

  const totalCount = page?.totalCount ?? 0
  const rows = page?.rows ?? []

  return (
    <section data-testid="view-detail" className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 className="text-2xl font-semibold tracking-tight">{zh.detail.title}</h2>
        <p className="text-sm text-muted-foreground">{zh.detail.description}</p>
      </div>

      <DetailFilterBar filters={filters} dispatch={dispatch} />

      {error !== null ? (
        <ErrorState error={error} onRetry={refetch} />
      ) : isPending ? (
        <LoadingState />
      ) : (
        <div
          data-testid="detail-content"
          data-fetching={String(isFetching)}
          className={isFetching ? 'flex flex-col gap-3 opacity-60' : 'flex flex-col gap-3'}
        >
          <DetailPager
            offset={offset}
            pageSize={pageSize}
            totalCount={totalCount}
            disabled={isFetching}
            onPrevious={() => dispatch({ type: 'previousPage' })}
            onNext={() => dispatch({ type: 'nextPage', totalCount })}
          />
          {rows.length === 0 ? (
            <EmptyState label={zh.detail.emptyFiltered} />
          ) : (
            <DetailTable rows={rows} timezone={timezone} />
          )}
        </div>
      )}
    </section>
  )
}
