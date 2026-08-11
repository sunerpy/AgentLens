/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Token grouping (shared contract with todos 16/17): four display columns —
 * 输入 / 输出 / 推理 / 缓存 — where 缓存 is `tokCacheRead + tokCacheWrite`. The query layer
 * deliberately returns five atomic buckets and never pre-merges, so the merge happens here
 * and only for presentation; both atomic cache values stay visible in the footer.
 */
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { Summary } from '@/generated'
import { zh } from '@/i18n/zh'
import { cn } from '@/lib/utils'
import type { MissingPriceEntry } from '@/views/overview/costMissing'
import { CostMissingPrices } from '@/views/overview/CostMissingPrices'
import type { CostTiersView, CostTierView } from '@/views/overview/costTiers'
import { costTiers } from '@/views/overview/costTiers'
import { formatCompact, formatCount, formatMoney, formatShare } from '@/views/overview/format'
import { cacheTokens } from '@/views/overview/trendModel'

/**
 * One metric cell.
 *
 * `exact` is the full-precision rendering of the same number, exposed as the `title` so a
 * compacted display never becomes the only copy of the figure.
 *
 * `min-w-0` on the wrapper is the grid-blowout guard: without it an over-wide value widens its
 * own column and pushes the siblings out of the card.
 *
 * The value span carries NO clipping — not `overflow-hidden`, not `truncate`. WebDriver defines
 * `getText()` as the element's *rendered* text and excludes anything clipped away, so clipping
 * here returns `""` to every text-reading client while the pixels still look correct (measured:
 * it silently turned the real-data assertions in `e2e-real/10-flow.spec.mjs` into `Number('')`).
 * Nothing needs clipping anyway — the compact value is ~46px inside a 110px cell.
 */
function Metric({
  label,
  value,
  exact,
  testId,
  emphasis = false,
  allowWrap = false,
}: {
  label: string
  value: string
  exact?: string
  testId: string
  emphasis?: boolean
  allowWrap?: boolean
}) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <span className="text-xs font-medium tracking-wide text-muted-foreground">{label}</span>
      <span
        data-testid={testId}
        title={exact ?? value}
        className={cn(
          'font-heading tabular-nums',
          emphasis ? 'text-2xl leading-tight font-semibold' : 'text-lg leading-tight font-medium',
          allowWrap &&
            'max-w-full text-xl [overflow-wrap:anywhere] sm:text-2xl lg:text-lg xl:text-xl',
        )}
      >
        {value}
      </span>
    </div>
  )
}

/**
 * Token magnitudes reach 11 digits on a real archive, so the display uses the same compact
 * notation the trend chart's Y axis already shows (`2.6B`) for visual consistency.
 */
function TokenMetric({ label, value, testId }: { label: string; value: number; testId: string }) {
  return (
    <Metric
      label={label}
      value={formatCompact(value)}
      exact={formatCount(value)}
      testId={testId}
      emphasis
    />
  )
}

function TokenCard({ summary }: { summary: Summary }) {
  const { tokens } = summary
  return (
    <Card className="lg:col-span-3">
      <CardHeader>
        <CardTitle>{zh.overview.summary.tokenTitle}</CardTitle>
        <CardDescription>{zh.overview.summary.tokenDescription}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="grid grid-cols-2 gap-x-6 gap-y-4 sm:grid-cols-4">
          <TokenMetric
            label={zh.common.tokens.input}
            value={tokens.tokInput}
            testId="summary-token-input"
          />
          <TokenMetric
            label={zh.common.tokens.output}
            value={tokens.tokOutput}
            testId="summary-token-output"
          />
          <TokenMetric
            label={zh.common.tokens.reasoning}
            value={tokens.tokReasoning}
            testId="summary-token-reasoning"
          />
          <TokenMetric
            label={zh.overview.summary.tokenCache}
            value={cacheTokens(tokens)}
            testId="summary-token-cache"
          />
        </div>
        <dl className="flex flex-wrap items-center gap-x-5 gap-y-1 border-t border-border pt-3 text-xs text-muted-foreground">
          <div className="flex items-center gap-1.5">
            <dt>{zh.common.tokens.totalInput}</dt>
            <dd
              data-testid="summary-token-total-input"
              className="font-medium tabular-nums text-foreground"
            >
              {formatCount(tokens.totalInput)}
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt>{zh.common.tokens.cacheRead}</dt>
            <dd data-testid="summary-token-cache-read" className="tabular-nums">
              {formatCount(tokens.tokCacheRead)}
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt>{zh.common.tokens.cacheWrite}</dt>
            <dd data-testid="summary-token-cache-write" className="tabular-nums">
              {formatCount(tokens.tokCacheWrite)}
            </dd>
          </div>
          <span className="text-[0.7rem]">{zh.overview.summary.tokenTotalHint}</span>
        </dl>
      </CardContent>
    </Card>
  )
}

/**
 * One costed coverage layer: how much of the range it covers, what it came to, and — the only
 * cross-tier comparable figure — what that works out to per million billable tokens.
 *
 * `emphasis` is the whole point of this round. Exactly one tier gets it: the local estimate, which
 * covers 99.97% of this project's records. The source-provided tier renders through the same
 * component at reduced weight inside a disclosure, so the two are never peers on screen — see
 * `costTiers.ts` for why equal weight was the defect rather than the copy around it.
 */
function CostTierRow({
  label,
  tier,
  amount,
  amountTestId,
  emphasis = false,
}: {
  label: string
  tier: CostTierView
  amount: number
  amountTestId: string
  emphasis?: boolean
}) {
  const share = tier.tokenShare
  return (
    <div
      data-testid={`summary-cost-tier-${tier.key}`}
      data-coverage-records={tier.recordCount}
      data-emphasis={emphasis ? 'primary' : 'secondary'}
      className={cn(
        'flex flex-col gap-1.5 rounded-lg border px-3 py-2.5',
        emphasis ? 'border-border bg-muted/30' : 'border-border/60 bg-background/40',
      )}
    >
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <span
            className={cn(
              'font-medium tracking-wide',
              emphasis ? 'text-sm text-foreground' : 'text-xs text-muted-foreground',
            )}
          >
            {label}
          </span>
          <span
            data-testid={`summary-cost-${tier.key}-share`}
            className="rounded-full bg-foreground/5 px-2 py-0.5 text-[0.7rem] font-medium tabular-nums ring-1 ring-border"
          >
            {share === null
              ? zh.overview.summary.costTierShareUnknown
              : zh.overview.summary.costTierShare(formatShare(share))}
          </span>
        </div>
        <span
          data-testid={amountTestId}
          title={formatMoney(amount)}
          className={cn(
            'font-heading max-w-full leading-tight font-semibold tabular-nums [overflow-wrap:anywhere]',
            emphasis ? 'text-2xl' : 'text-base',
          )}
        >
          {formatMoney(amount)}
        </span>
      </div>
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-[0.7rem] text-muted-foreground">
        <span>{zh.overview.summary.costUnitPriceLabel}</span>
        <span
          data-testid={`summary-cost-${tier.key}-unit-price`}
          className="font-medium tabular-nums text-foreground"
        >
          {tier.unitPricePerMillion === null
            ? zh.overview.summary.costUnitPriceUndefined
            : formatMoney(tier.unitPricePerMillion)}
        </span>
      </div>
      <span
        data-testid={`summary-cost-${tier.key}-coverage`}
        className="text-[0.7rem] leading-relaxed text-muted-foreground"
      >
        {zh.overview.summary.costCoverage(
          formatCount(tier.recordCount),
          formatCompact(tier.billableTokens),
        )}
      </span>
    </div>
  )
}

/**
 * Disclaims the headline `$0` in the two shapes where it is the absence of data rather than a real
 * total, and renders nothing otherwise.
 *
 * The previous version of this note ran unconditionally and had to talk about "两格的 $0" because
 * two amounts were always on screen. With one headline there is only one zero to qualify, so the
 * note disappears entirely whenever the estimate stands on real coverage — which is the common
 * case, and a card that stops explaining itself when there is nothing to explain reads as working
 * rather than as warning.
 */
function CostPrimaryNote({ tiers }: { tiers: CostTiersView }) {
  if (tiers.primaryNote === null) return null
  return (
    <p
      data-testid="summary-cost-primary-note"
      data-primary-note={tiers.primaryNote}
      role="note"
      className="rounded-lg border border-border border-dashed bg-muted/30 px-3 py-2 text-[0.7rem] leading-relaxed text-muted-foreground"
    >
      {tiers.primaryNote === 'estimatedNoCoverage'
        ? zh.overview.summary.costEstimatedNoCoverage
        : zh.overview.summary.costNoCoverage}
    </p>
  )
}

/**
 * The source-provided amount, behind a collapsed disclosure that names its own coverage.
 *
 * The trigger states the record count rather than the tier name, because the count is what makes
 * the figure worth opening or ignoring: `另有 117 条记录自带上游金额` already tells a reader with
 * 287,747 estimated records that this is a footnote, before any money is on screen. The
 * incomparability sentence and the unit-price pointer live inside, next to the only two numbers
 * they describe, instead of as a standing caveat on a card-wide comparison that no longer exists.
 */
function CostSourceProvidedDisclosure({ tiers, amount }: { tiers: CostTiersView; amount: number }) {
  const [open, setOpen] = useState(false)
  if (!tiers.hasSourceProvided) return null
  return (
    <div data-testid="summary-cost-source-provided" className="flex flex-col gap-2">
      <Button
        type="button"
        size="sm"
        variant="ghost"
        aria-expanded={open}
        className="self-start"
        data-testid="summary-cost-source-toggle"
        onClick={() => setOpen((current) => !current)}
      >
        {open
          ? zh.overview.summary.costSourceHide
          : zh.overview.summary.costSourceShow(formatCount(tiers.actual.recordCount))}
      </Button>
      {open ? (
        <div className="flex flex-col gap-2">
          <CostTierRow
            label={zh.common.cost.actual}
            tier={tiers.actual}
            amount={amount}
            amountTestId="summary-cost-actual"
          />
          <div
            data-testid="summary-cost-source-explain"
            data-comparability={tiers.comparability}
            role="note"
            className="flex flex-col gap-1 rounded-lg border border-border border-dashed bg-muted/30 px-3 py-2"
          >
            <p className="text-[0.7rem] leading-relaxed text-muted-foreground">
              {zh.overview.summary.costSourceExplain}
            </p>
            {tiers.comparability === 'incomparable' ? (
              <>
                <p className="text-[0.7rem] leading-relaxed text-muted-foreground">
                  {zh.overview.summary.costSourceIncomparable(
                    formatCount(tiers.actual.recordCount),
                    formatCount(tiers.estimated.recordCount),
                  )}
                </p>
                <p className="text-[0.7rem] leading-relaxed text-muted-foreground">
                  {zh.overview.summary.costUnitPriceHint}
                </p>
              </>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  )
}

/**
 * The 目录无价 block is gated on a non-zero count, not merely styled differently at zero.
 *
 * Its explanatory line describes why some usage has no amount attached. At zero there is nothing
 * to explain, so the sentence describes nothing and reads as a warning about a problem the user
 * does not have. The estimate headline stays unconditional by contrast: it is the one figure the
 * card is *about*, and `$0.0000` there is qualified by {@link CostPrimaryNote} instead of hidden.
 */
function CostCard({
  summary,
  missingPrices,
  archiveMissingPrices,
}: {
  summary: Summary
  missingPrices: readonly MissingPriceEntry[]
  archiveMissingPrices: readonly MissingPriceEntry[]
}) {
  const { cost } = summary
  const { costCoverage } = summary
  const tiers = costTiers(cost, costCoverage)
  const hasUnavailable = cost.unavailableCount > 0
  return (
    <Card className="lg:col-span-2" data-testid="summary-cost-card">
      <CardHeader>
        <CardTitle className="flex items-center justify-between gap-2">
          <span>{zh.overview.summary.costTitle}</span>
        </CardTitle>
        <CardDescription>{zh.overview.summary.costDescription}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <CostTierRow
          label={zh.common.cost.estimated}
          tier={tiers.estimated}
          amount={cost.estimatedSum}
          amountTestId="summary-cost-estimated"
          emphasis
        />
        <span className="text-[0.7rem] leading-relaxed text-muted-foreground select-none">
          {zh.overview.summary.costPrimaryHint}
        </span>
        <CostPrimaryNote tiers={tiers} />
        <CostSourceProvidedDisclosure tiers={tiers} amount={cost.actualSum} />
        {hasUnavailable ? (
          <>
            <div
              data-testid="summary-cost-unavailable-block"
              className="flex flex-col gap-1 border-t border-border pt-3"
            >
              <div className="flex items-baseline gap-1.5 text-xs">
                <span className="text-muted-foreground">
                  {zh.overview.summary.costUnavailableLabel}
                </span>
                <span
                  data-testid="summary-cost-unavailable"
                  className="font-medium tabular-nums text-destructive"
                >
                  {formatCount(cost.unavailableCount)}
                </span>
                <span className="text-muted-foreground">
                  {zh.overview.summary.costUnavailableUnit}
                </span>
              </div>
              <span className="text-[0.7rem] text-muted-foreground">
                {zh.overview.summary.costUnavailableHint}
              </span>
              <span
                data-testid="summary-cost-unavailable-coverage"
                className="text-[0.7rem] text-muted-foreground"
              >
                {zh.overview.summary.costCoverage(
                  formatCount(costCoverage.unavailable.recordCount),
                  formatCompact(costCoverage.unavailable.billableTokens),
                )}
              </span>
            </div>
            <CostMissingPrices
              entries={missingPrices}
              unavailableCount={cost.unavailableCount}
              archiveEntries={archiveMissingPrices}
            />
          </>
        ) : null}
      </CardContent>
    </Card>
  )
}

function VolumeCard({ summary }: { summary: Summary }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{zh.overview.summary.volumeTitle}</CardTitle>
        <CardDescription>{zh.overview.summary.volumeDescription}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <Metric
          label={zh.common.messageCount}
          value={formatCount(summary.messageCount)}
          testId="summary-message-count"
          emphasis
        />
        {hasSessionRecords(summary) ? (
          <Metric
            label={zh.common.sessionRecordCount}
            value={formatCount(summary.sessionRecordCount)}
            testId="summary-session-record-count"
            emphasis
          />
        ) : null}
        <Metric
          label={zh.common.activeSessionCount}
          value={formatCount(summary.activeSessionCount)}
          testId="summary-active-session-count"
        />
      </CardContent>
    </Card>
  )
}

/**
 * The granularity explainer is a full-width band rather than in-card copy for two reasons: the
 * volume card is one of six grid columns, so a paragraph there would stretch the whole row; and
 * the sentence has to be readable without hovering, because the discrepancy it explains
 * (`消息数` excluding records the token totals include) otherwise reads as a defect.
 */
function GranularityNote({ summary }: { summary: Summary }) {
  return (
    <section
      data-testid="summary-granularity-note"
      className="flex flex-col gap-1 rounded-xl border border-border border-dashed bg-muted/30 px-4 py-3 lg:col-span-6"
    >
      <span className="text-xs font-medium tracking-wide">
        {zh.overview.summary.granularityNoteTitle}
      </span>
      <p className="text-xs leading-relaxed text-muted-foreground">
        {zh.overview.summary.granularityNote(
          formatCount(summary.messageCount),
          formatCount(summary.sessionRecordCount),
        )}
      </p>
    </section>
  )
}

function hasSessionRecords(summary: Summary): boolean {
  return summary.sessionRecordCount > 0
}

export function SummaryCards({
  summary,
  missingPrices = [],
  archiveMissingPrices = [],
}: {
  summary: Summary
  missingPrices?: readonly MissingPriceEntry[]
  archiveMissingPrices?: readonly MissingPriceEntry[]
}) {
  return (
    <div data-testid="overview-summary" className="grid gap-4 lg:grid-cols-6">
      <TokenCard summary={summary} />
      <CostCard
        summary={summary}
        missingPrices={missingPrices}
        archiveMissingPrices={archiveMissingPrices}
      />
      <VolumeCard summary={summary} />
      {hasSessionRecords(summary) ? <GranularityNote summary={summary} /> : null}
    </div>
  )
}
