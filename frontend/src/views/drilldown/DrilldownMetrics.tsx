/**
 * EXCLUSIVE FILE BOUNDARY — todo 16 owns `src/views/drilldown/**`.
 *
 * Shared presentation primitives for the three level tables: the level shell, the numeric
 * cells and the share bar. Keeping the metric cells in one component is what guarantees
 * the four token groups and the three cost buckets are rendered identically at every level.
 */
import type { ReactNode } from 'react'

import { zh } from '@/i18n/zh'

import type { DrilldownMetric } from './aggregate'
import { formatAmount, formatCount, formatShare, sharePercent } from './format'

export function LevelCard({
  step,
  stepLabel,
  title,
  hint,
  testId,
  meta,
  children,
}: {
  step: number
  stepLabel: string
  title: string
  hint: string
  testId: string
  meta?: ReactNode
  children: ReactNode
}) {
  return (
    <section
      data-testid={testId}
      data-level={step}
      className="overflow-hidden rounded-xl border border-border bg-card shadow-xs"
    >
      <header className="flex flex-wrap items-baseline gap-x-3 gap-y-1 border-b border-border bg-muted/40 px-4 py-3">
        <span
          aria-hidden="true"
          className="flex size-6 shrink-0 translate-y-0.5 items-center justify-center rounded-full bg-foreground text-[11px] font-semibold tabular-nums text-background"
        >
          {step}
        </span>
        <h3 className="text-sm font-semibold tracking-tight">
          <span className="text-muted-foreground">{stepLabel}</span>
          <span className="px-1.5 text-muted-foreground/60">/</span>
          {title}
        </h3>
        <p className="min-w-0 flex-1 text-xs text-muted-foreground">{hint}</p>
        {meta === undefined ? null : (
          <div className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
            {meta}
          </div>
        )}
      </header>
      {children}
    </section>
  )
}

export function MetaChip({ label, value }: { label: string; value: number }) {
  return (
    <span className="rounded-md border border-border bg-background px-2 py-0.5 tabular-nums">
      {formatCount(value)}
      <span className="pl-1 text-muted-foreground/80">{label}</span>
    </span>
  )
}

const HEAD_CELL = 'px-3 py-2 text-right text-xs font-medium text-muted-foreground whitespace-nowrap'

/**
 * `showSessionRecords` comes from the view, not from the row: the column must appear or vanish for
 * a whole table at once, so a row-local check would desync header from cells. It stays off unless
 * the range really holds session-granularity records, which would otherwise be a column of zeros.
 */
export function MetricHeadCells({ showSessionRecords }: { showSessionRecords: boolean }) {
  return (
    <>
      <th scope="col" className={HEAD_CELL}>
        {zh.common.tokens.input}
      </th>
      <th scope="col" className={HEAD_CELL}>
        {zh.common.tokens.output}
      </th>
      <th scope="col" className={HEAD_CELL}>
        {zh.common.tokens.reasoning}
      </th>
      <th scope="col" className={HEAD_CELL}>
        {zh.common.tokens.cacheRead}
        <span className="px-1 text-muted-foreground/60">+</span>
        {zh.common.tokens.cacheWrite}
      </th>
      <th scope="col" className={`${HEAD_CELL} border-l border-border`}>
        {zh.common.cost.actual}
      </th>
      <th scope="col" className={HEAD_CELL}>
        {zh.common.cost.estimated}
      </th>
      <th scope="col" className={`${HEAD_CELL} border-l border-border`}>
        {zh.common.messageCount}
      </th>
      {showSessionRecords ? (
        <th scope="col" className={HEAD_CELL} title={zh.drilldown.sessionRecordNote}>
          {zh.common.sessionRecordCount}
        </th>
      ) : null}
      <th scope="col" className={`${HEAD_CELL} w-40 text-left`}>
        {zh.drilldown.columnShare}
      </th>
    </>
  )
}

const CELL = 'px-3 py-2 text-right text-xs tabular-nums whitespace-nowrap'

export function MetricCells({
  metric,
  share,
  showSessionRecords,
}: {
  metric: DrilldownMetric
  share: number
  showSessionRecords: boolean
}) {
  const { tokens, cost } = metric
  return (
    <>
      <td data-testid="cell-input" className={CELL}>
        {formatCount(tokens.tokInput)}
      </td>
      <td data-testid="cell-output" className={CELL}>
        {formatCount(tokens.tokOutput)}
      </td>
      <td data-testid="cell-reasoning" className={`${CELL} text-muted-foreground`}>
        {formatCount(tokens.tokReasoning)}
      </td>
      <td
        data-testid="cell-cache"
        className={CELL}
        title={`${zh.common.tokens.cacheRead} ${formatCount(tokens.tokCacheRead)} · ${zh.common.tokens.cacheWrite} ${formatCount(tokens.tokCacheWrite)}`}
      >
        <span data-testid="cell-cache-total">
          {formatCount(tokens.tokCacheRead + tokens.tokCacheWrite)}
        </span>
        <span
          data-testid="cell-cache-split"
          className="block text-[11px] font-normal text-muted-foreground"
        >
          {formatCount(tokens.tokCacheRead)}
          <span className="px-1 text-muted-foreground/60">+</span>
          {formatCount(tokens.tokCacheWrite)}
        </span>
      </td>
      <td data-testid="cell-cost-actual" className={`${CELL} border-l border-border`}>
        <span className="font-medium">{formatAmount(cost.actualSum)}</span>
        {cost.unavailableCount > 0 ? (
          <span
            data-testid="cost-unavailable-badge"
            className="mt-1 block rounded-md border border-border bg-background px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground"
          >
            {zh.drilldown.unavailableBadge}
            <span className="pl-1">{formatCount(cost.unavailableCount)}</span>
          </span>
        ) : null}
      </td>
      <td data-testid="cell-cost-estimated" className={`${CELL} text-muted-foreground`}>
        {formatAmount(cost.estimatedSum)}
      </td>
      <td data-testid="cell-messages" className={`${CELL} border-l border-border`}>
        {formatCount(metric.messageCount)}
      </td>
      {showSessionRecords ? (
        <td data-testid="cell-session-records" className={CELL}>
          {formatCount(metric.sessionRecordCount)}
        </td>
      ) : null}
      <td data-testid="cell-share" className="px-3 py-2">
        <ShareBar share={share} />
      </td>
    </>
  )
}

export function ShareBar({ share }: { share: number }) {
  const percent = sharePercent(share)
  return (
    <span className="flex items-center gap-2" title={zh.drilldown.shareNote}>
      <span
        data-testid="share-bar"
        data-share={String(percent)}
        role="progressbar"
        aria-label={zh.drilldown.columnShare}
        aria-valuenow={percent}
        aria-valuemin={0}
        aria-valuemax={100}
        className="h-1.5 min-w-16 flex-1 overflow-hidden rounded-full bg-muted"
      >
        <span
          className="block h-full rounded-full bg-foreground/70"
          style={{ width: `${Math.max(percent, percent > 0 ? 2 : 0)}%` }}
        />
      </span>
      <span className="w-12 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
        {formatShare(share)}
      </span>
    </span>
  )
}
