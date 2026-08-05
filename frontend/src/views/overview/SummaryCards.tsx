/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Token grouping (shared contract with todos 16/17): four display columns —
 * 输入 / 输出 / 推理 / 缓存 — where 缓存 is `tokCacheRead + tokCacheWrite`. The query layer
 * deliberately returns five atomic buckets and never pre-merges, so the merge happens here
 * and only for presentation; both atomic cache values stay visible in the footer.
 */
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { Summary } from '@/generated'
import { zh } from '@/i18n/zh'
import { cn } from '@/lib/utils'
import { CostPartialBadge } from '@/views/overview/CostPartialBadge'
import { formatCompact, formatCount, formatMoney } from '@/views/overview/format'
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
}: {
  label: string
  value: string
  exact?: string
  testId: string
  emphasis?: boolean
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

function CostCard({ summary }: { summary: Summary }) {
  const { cost } = summary
  const hasUnavailable = cost.unavailableCount > 0
  return (
    <Card className="lg:col-span-2" data-testid="summary-cost-card">
      <CardHeader>
        <CardTitle className="flex items-center justify-between gap-2">
          <span>{zh.overview.summary.costTitle}</span>
          {hasUnavailable ? <CostPartialBadge /> : null}
        </CardTitle>
        <CardDescription>{zh.overview.summary.costDescription}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="grid grid-cols-2 gap-x-6 gap-y-4">
          <Metric
            label={zh.common.cost.actual}
            value={formatMoney(cost.actualSum)}
            testId="summary-cost-actual"
            emphasis
          />
          <Metric
            label={zh.common.cost.estimated}
            value={formatMoney(cost.estimatedSum)}
            testId="summary-cost-estimated"
            emphasis
          />
        </div>
        <div className="flex flex-col gap-1 border-t border-border pt-3">
          <div className="flex items-baseline gap-1.5 text-xs">
            <span className="text-muted-foreground">
              {zh.overview.summary.costUnavailableLabel}
            </span>
            <span
              data-testid="summary-cost-unavailable"
              className={cn(
                'font-medium tabular-nums',
                hasUnavailable ? 'text-destructive' : 'text-foreground',
              )}
            >
              {formatCount(cost.unavailableCount)}
            </span>
            <span className="text-muted-foreground">{zh.overview.summary.costUnavailableUnit}</span>
          </div>
          <span className="text-[0.7rem] text-muted-foreground">
            {zh.overview.summary.costUnavailableHint}
          </span>
        </div>
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
        <Metric
          label={zh.common.activeSessionCount}
          value={formatCount(summary.activeSessionCount)}
          testId="summary-active-session-count"
        />
      </CardContent>
    </Card>
  )
}

export function SummaryCards({ summary }: { summary: Summary }) {
  return (
    <div data-testid="overview-summary" className="grid gap-4 lg:grid-cols-6">
      <TokenCard summary={summary} />
      <CostCard summary={summary} />
      <VolumeCard summary={summary} />
    </div>
  )
}
