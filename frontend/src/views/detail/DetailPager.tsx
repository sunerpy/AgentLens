/**
 * EXCLUSIVE FILE BOUNDARY — todo 17 owns `src/views/detail/**`.
 *
 * Pager. Every bound is derived from the server's `totalCount`, never from `rows.length`:
 * `rows.length` is the size of the page that happened to come back and would make the last page
 * look like the whole result set.
 */
import { Button } from '@/components/ui/button'
import { zh } from '@/i18n/zh'

import { formatCount } from './formatDetail'

export function DetailPager({
  offset,
  pageSize,
  totalCount,
  disabled,
  onPrevious,
  onNext,
}: {
  offset: number
  pageSize: number
  totalCount: number
  disabled: boolean
  onPrevious: () => void
  onNext: () => void
}) {
  const firstRow = totalCount === 0 ? 0 : offset + 1
  const lastRow = Math.min(offset + pageSize, totalCount)

  return (
    <div
      data-testid="detail-pager"
      className="flex flex-wrap items-center justify-between gap-3 text-sm"
    >
      <div className="flex flex-wrap items-center gap-3 text-muted-foreground">
        <span data-testid="detail-total-count" data-total-count={String(totalCount)}>
          {`${zh.detail.pager.totalRows} ${formatCount(totalCount)} ${zh.detail.pager.rowsUnit}`}
        </span>
        <span data-testid="detail-page-range">
          {`${zh.detail.pager.showing} ${formatCount(firstRow)} ${zh.detail.pager.to} ${formatCount(lastRow)} ${zh.detail.pager.rowsUnit}`}
        </span>
        <span data-testid="detail-page-size">
          {`${zh.detail.pager.pageSize} ${formatCount(pageSize)} ${zh.detail.pager.rowsUnit}`}
        </span>
      </div>
      <div className="flex items-center gap-2">
        <Button
          type="button"
          size="sm"
          variant="outline"
          data-testid="detail-prev-page"
          disabled={disabled || offset === 0}
          onClick={onPrevious}
        >
          {zh.detail.pager.previous}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          data-testid="detail-next-page"
          disabled={disabled || lastRow >= totalCount}
          onClick={onNext}
        >
          {zh.detail.pager.next}
        </Button>
      </div>
    </div>
  )
}
