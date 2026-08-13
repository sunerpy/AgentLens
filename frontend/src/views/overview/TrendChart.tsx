/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Coverage-gap-aware trend chart.
 *
 * Rendering technique (documented here because todos 16/17 must stay consistent with it):
 * - A `none` bucket keeps `null` series values and `connectNulls={false}`, so d3's
 *   `defined()` genuinely breaks the path. It is never coerced to `0`.
 * - Non-`full` buckets additionally get a full-height hatched band, drawn as a single
 *   `Bar` on a hidden `[0, 1]` axis with `barCategoryGap={0}`, so the band spans the whole
 *   category. One bar (not two) is used because two bars would each be given half the band
 *   width. The band's styling is chosen by the shape from `payload.coverage`.
 * - `isAnimationActive={false}` everywhere: recharts skips dot rendering entirely while an
 *   animation is in flight, which would make the dot assertions race the transition.
 *
 * Grouped mode (按模型 / 按 agent / 按工具) reuses the same coverage band and the same `null`
 * discipline, but draws one `Line` per group instead of the stacked-area total, and takes its
 * coverage state from the ungrouped total series — see `trendGrouping.ts` for why.
 */
import { useMemo, useState } from 'react'
import {
  Area,
  Bar,
  CartesianGrid,
  ComposedChart,
  Line,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts'

import { Button } from '@/components/ui/button'
import { EmptyState, LoadingState } from '@/components/app-state'
import type { CoverageNote, CoverageShortfall, CoverageStatus } from '@/generated'
import { zh } from '@/i18n/zh'
import { cn } from '@/lib/utils'
import { CostPartialBadge } from '@/views/overview/CostPartialBadge'
import {
  coverageExplanationFor,
  coverageReasonIndex,
  hasCoverageGap,
  isBucketInProgress,
  type CoverageExplanation,
  type CoverageReasonIndex,
} from '@/views/overview/coverageReason'
import {
  CHART_TOKENS,
  GAP_PATTERN_ID,
  OTHER_SERIES_COLOR,
  PARTIAL_PATTERN_ID,
  SERIES_PALETTE,
} from '@/views/overview/chartTokens'
import { formatCompact, formatCount, formatMoney } from '@/views/overview/format'
import {
  TREND_GROUP_MODES,
  buildGroupedTrend,
  groupedSeriesTotal,
  visibleSeries,
  type GroupedTrendRow,
  type TrendGroupMode,
  type TrendGroupPart,
} from '@/views/overview/trendGrouping'
import {
  TREND_METRICS,
  type TrendMetric,
  type TrendRow,
  type TrendSeriesKey,
  cacheTokens,
  hasAnyCoverage,
  rowValue,
  seriesKeysFor,
  totalTokens,
  unavailableCount,
  valueAxisMax,
} from '@/views/overview/trendModel'

const CHART_HEIGHT = 300

const CHART_MARGIN = { top: 8, right: 8, bottom: 4, left: 4 } as const

const METRIC_LABEL: Record<TrendMetric, string> = {
  tokens: zh.overview.trend.metricTokens,
  cost: zh.overview.trend.metricCost,
}

const METRIC_HINT: Record<TrendMetric, string> = {
  tokens: zh.overview.trend.metricTokensHint,
  cost: zh.overview.trend.metricCostHint,
}

const SERIES_LABEL: Record<TrendSeriesKey, string> = {
  tokens: zh.overview.trend.seriesTokens,
  actual: zh.overview.trend.seriesActual,
  estimated: zh.overview.trend.seriesEstimated,
}

const SERIES_COLOR: Record<TrendSeriesKey, string> = {
  tokens: CHART_TOKENS.seriesTokens,
  actual: CHART_TOKENS.seriesActual,
  estimated: CHART_TOKENS.seriesEstimated,
}

const SERIES_DATA_KEY: Record<TrendSeriesKey, keyof TrendRow> = {
  tokens: 'tokensValue',
  actual: 'actualValue',
  estimated: 'estimatedValue',
}

const COVERAGE_LABEL = {
  full: zh.common.coverage.full,
  partial: zh.common.coverage.partial,
  none: zh.common.coverage.none,
} as const

const GROUP_LABEL: Record<TrendGroupMode, string> = {
  none: zh.overview.trend.groupNone,
  model: zh.overview.trend.groupModel,
  agent: zh.overview.trend.groupAgent,
  tool: zh.overview.trend.groupTool,
}

const GROUP_HINT: Record<TrendGroupMode, string> = {
  none: zh.overview.trend.groupNoneHint,
  model: zh.overview.trend.groupModelHint,
  agent: zh.overview.trend.groupAgentHint,
  tool: zh.overview.trend.groupToolHint,
}

function formatValue(metric: TrendMetric, value: number): string {
  return metric === 'cost' ? formatMoney(value) : formatCount(value)
}

/** Rust emits `YYYY-MM-DDTHH:MM±HH:MM` for hour buckets; only the clock part is useful here. */
function axisTick(label: string): string {
  const separator = label.indexOf('T')
  return separator === -1 ? label : label.slice(separator + 1, separator + 6)
}

/** Structural payload: the band is identical in ungrouped and grouped mode. */
interface CoveredRow {
  label: string
  coverage: CoverageStatus
}

interface BandShapeProps {
  x?: number
  y?: number
  width?: number
  height?: number
  payload?: CoveredRow
}

function CoverageBand({ x, y, width, height, payload }: BandShapeProps) {
  if (payload === undefined) return null
  const coverage = payload.coverage
  if (coverage !== 'none' && coverage !== 'partial') return null
  if (
    typeof x !== 'number' ||
    typeof y !== 'number' ||
    typeof width !== 'number' ||
    typeof height !== 'number' ||
    !Number.isFinite(x) ||
    !Number.isFinite(y) ||
    width <= 0 ||
    height <= 0
  ) {
    return null
  }
  const isGap = coverage === 'none'
  return (
    <rect
      data-testid="coverage-band"
      data-coverage={coverage}
      data-bucket={payload.label}
      x={x}
      y={y}
      width={width}
      height={height}
      fill={`url(#${isGap ? GAP_PATTERN_ID : PARTIAL_PATTERN_ID})`}
      stroke={isGap ? CHART_TOKENS.coverageGap : CHART_TOKENS.coveragePartial}
      strokeOpacity={isGap ? 0.35 : 0.5}
      strokeDasharray={isGap ? '3 3' : undefined}
      opacity={isGap ? 1 : 0.65}
    />
  )
}

interface TrendDotProps {
  cx?: number
  cy?: number
  payload?: TrendRow
  seriesKey?: TrendSeriesKey
}

/**
 * recharts hands an Area dot `value` as the range tuple `[base, value]`, and `cy === null`
 * for an undefined datum. The plotted number is therefore read back off `payload` via
 * `rowValue`, which is the same accessor the tooltip and axis use — so the rendered
 * `data-value` can never disagree with the series it belongs to.
 */
function TrendDot({ cx, cy, payload, seriesKey }: TrendDotProps) {
  if (payload === undefined || seriesKey === undefined) return null
  const value = rowValue(payload, seriesKey)
  if (
    value === null ||
    typeof cx !== 'number' ||
    typeof cy !== 'number' ||
    !Number.isFinite(cx) ||
    !Number.isFinite(cy)
  ) {
    return null
  }
  return (
    <circle
      data-testid={`trend-dot-${seriesKey}`}
      data-bucket={payload.label}
      data-coverage={payload.coverage}
      data-value={String(value)}
      cx={cx}
      cy={cy}
      r={payload.coverage === 'partial' ? 4 : 3.5}
      fill={CHART_TOKENS.surface}
      stroke={SERIES_COLOR[seriesKey]}
      strokeWidth={2}
      strokeDasharray={payload.coverage === 'partial' ? '2 2' : undefined}
    />
  )
}

interface TrendTooltipProps {
  active?: boolean
  label?: string | number
  rows?: TrendRow[]
  metric?: TrendMetric
  reasons?: CoverageReasonIndex
  nowMs?: number
}

function TooltipRow({ label, value, testId }: { label: string; value: string; testId?: string }) {
  return (
    <div data-testid={testId} className="flex items-baseline justify-between gap-6">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-medium tabular-nums">{value}</span>
    </div>
  )
}

function TrendTooltip({ active, label, rows, metric, reasons, nowMs }: TrendTooltipProps) {
  if (active !== true || typeof label !== 'string' || rows === undefined || metric === undefined) {
    return null
  }
  const row = rows.find((candidate) => candidate.label === label)
  if (row === undefined) return null
  const explanation =
    row.coverage === 'full' || reasons === undefined || nowMs === undefined
      ? undefined
      : coverageExplanationFor(reasons, row, nowMs)

  const missing = row.coverage === 'none'
  // Rendered only when non-zero, unlike the unconditional cost rows above: this row exists to
  // explain a discrepancy that does not exist at 0, so it would be noise in every tooltip of
  // every installation without a session-only source.
  const sessionRecords = row.sessionRecordCount ?? 0
  return (
    <div
      data-testid="trend-tooltip"
      data-bucket={row.label}
      data-coverage={row.coverage}
      className="min-w-56 rounded-lg border border-border bg-popover/95 p-3 text-xs shadow-lg backdrop-blur-sm"
    >
      <div className="flex items-center justify-between gap-3 border-b border-border pb-2">
        <span className="font-heading text-sm font-medium">{row.label}</span>
        <span
          data-testid="trend-tooltip-coverage"
          className={cn(
            'rounded-full px-2 py-0.5 text-[0.7rem] font-medium ring-1',
            missing
              ? 'bg-muted text-muted-foreground ring-border'
              : row.coverage === 'partial'
                ? 'bg-chart-1/40 text-foreground ring-foreground/15'
                : 'bg-foreground/5 text-foreground ring-foreground/10',
          )}
        >
          {COVERAGE_LABEL[row.coverage]}
        </span>
      </div>

      {missing ? (
        <>
          <p data-testid="trend-tooltip-gap-note" className="pt-2 text-muted-foreground">
            {zh.overview.trend.tooltipNoCoverage}
          </p>
          {explanation === undefined ? null : <CoverageReasonNote explanation={explanation} />}
        </>
      ) : (
        <div className="flex flex-col gap-1 pt-2">
          {row.tokens === null ? null : (
            <>
              <TooltipRow
                label={SERIES_LABEL.tokens}
                value={formatCount(totalTokens(row.tokens))}
              />
              <TooltipRow label={zh.common.tokens.input} value={formatCount(row.tokens.tokInput)} />
              <TooltipRow
                label={zh.common.tokens.output}
                value={formatCount(row.tokens.tokOutput)}
              />
              <TooltipRow
                label={zh.common.tokens.reasoning}
                value={formatCount(row.tokens.tokReasoning)}
              />
              <TooltipRow
                label={zh.overview.summary.tokenCache}
                value={formatCount(cacheTokens(row.tokens))}
              />
            </>
          )}
          {row.cost === null ? null : (
            <div className="mt-1 flex flex-col gap-1 border-t border-border pt-2">
              <TooltipRow label={SERIES_LABEL.actual} value={formatMoney(row.cost.actualSum)} />
              <TooltipRow
                label={SERIES_LABEL.estimated}
                value={formatMoney(row.cost.estimatedSum)}
              />
              <TooltipRow
                label={zh.overview.summary.costUnavailableLabel}
                value={formatCount(row.cost.unavailableCount)}
              />
              {row.cost.unavailableCount > 0 ? <CostPartialBadge className="self-start" /> : null}
            </div>
          )}
          {row.messageCount === null && sessionRecords === 0 ? null : (
            <div className="mt-1 flex flex-col gap-1 border-t border-border pt-2">
              {row.messageCount === null ? null : (
                <TooltipRow label={zh.common.messageCount} value={formatCount(row.messageCount)} />
              )}
              {sessionRecords > 0 ? (
                <TooltipRow
                  label={zh.common.sessionRecordCount}
                  value={formatCount(sessionRecords)}
                  testId="trend-tooltip-session-records"
                />
              ) : null}
            </div>
          )}
          {row.coverage === 'full' && row.tokens !== null && totalTokens(row.tokens) === 0 ? (
            <p data-testid="trend-tooltip-zero-note" className="pt-1 text-muted-foreground">
              {zh.overview.trend.tooltipZeroUsage}
            </p>
          ) : null}
          {explanation === undefined ? null : <CoverageReasonNote explanation={explanation} />}
        </div>
      )}
    </div>
  )
}

/**
 * Names the `(host, source)` pairs behind one non-`full` bucket.
 *
 * `partial` and `missing` are worded differently on purpose: "covered only part of it" points the
 * reader at a collection that started or stopped mid-bucket, while "no interval at all" points at
 * a source that was never collected on that host. Collapsing them into one sentence would leave
 * the reader unable to tell which of the two they are looking at.
 */
function CoverageReasonPairs({ pairs }: { pairs: readonly CoverageShortfall[] }) {
  return (
    <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
      {pairs.map((shortfall) => (
        <span
          key={`${shortfall.hostId}/${shortfall.source}`}
          data-testid="coverage-reason-pair"
          data-host-id={shortfall.hostId}
          data-source={shortfall.source}
          data-partial={shortfall.partial ? 'true' : 'false'}
          className="inline-flex min-w-0 items-baseline gap-1.5"
        >
          <span className="truncate font-medium">
            {zh.overview.trend.coverageReasonPair(shortfall.hostId, shortfall.source)}
          </span>
          <span className="shrink-0 text-muted-foreground">
            {shortfall.partial
              ? zh.overview.trend.coverageReasonPartial
              : zh.overview.trend.coverageReasonMissing}
          </span>
        </span>
      ))}
    </span>
  )
}

/**
 * An unfinished bucket keeps its badge but swaps the pair list for one sentence: the pairs are the
 * inevitable consequence of collecting up to `now`, so listing them reads as a defect report on
 * every refresh. Pairs still appear when something survives that filter — see
 * `coverageExplanationFor`.
 */
function CoverageReasonNote({ explanation }: { explanation: CoverageExplanation }) {
  return (
    <div
      data-testid="coverage-reason"
      data-in-progress={explanation.inProgress ? 'true' : 'false'}
      className="mt-1 flex flex-col gap-1 border-t border-border pt-2"
    >
      <span className="font-medium">
        {explanation.inProgress
          ? zh.overview.trend.coverageInProgressTitle
          : zh.overview.trend.coverageReasonTitle}
      </span>
      {explanation.inProgress ? (
        <span data-testid="coverage-reason-in-progress" className="text-muted-foreground">
          {zh.overview.trend.coverageInProgressNote}
        </span>
      ) : null}
      {explanation.unknown ? (
        <span data-testid="coverage-reason-unknown" className="text-muted-foreground">
          {zh.overview.trend.coverageReasonUnknown}
        </span>
      ) : null}
      {explanation.pairs.length > 0 ? (
        <>
          <span className="text-muted-foreground">
            {explanation.inProgress
              ? zh.overview.trend.coverageInProgressMissingIntro
              : zh.overview.trend.coverageReasonIntro}
          </span>
          <CoverageReasonPairs pairs={explanation.pairs} />
        </>
      ) : null}
    </div>
  )
}

/**
 * Always-visible counterpart to the tooltip copy. The tooltip only exists while the pointer is
 * over a bucket, so on a touch screen — and in any screenshot — the reason would otherwise be
 * unreachable.
 *
 * Rendered only for buckets with a gap left to report. An unfinished bucket whose every shortfall
 * is explained by "the period has not ended" is dropped: it is guaranteed non-`full` on every
 * refresh of every day, so listing it here turned the panel into a permanent false alarm that hid
 * the finished buckets whose gaps are real. Its band, its chip in `TrendNotes` and its tooltip all
 * still say 部分覆盖 — the data is genuinely incomplete, only the diagnosis is withdrawn.
 */
function CoverageReasons({
  rows,
  reasons,
  nowMs,
}: {
  rows: TrendRow[]
  reasons: CoverageReasonIndex
  nowMs: number
}) {
  const flagged = rows
    .filter((row) => row.coverage !== 'full')
    .map((row) => ({ row, explanation: coverageExplanationFor(reasons, row, nowMs) }))
    .filter((entry) => hasCoverageGap(entry.explanation))
  if (flagged.length === 0) return null
  return (
    <section
      data-testid="trend-coverage-reasons"
      className="flex flex-col gap-2 rounded-xl border border-border border-dashed bg-muted/30 px-4 py-3 text-xs"
    >
      <span className="font-medium tracking-wide">{zh.overview.trend.coverageReasonTitle}</span>
      <span className="text-muted-foreground">{zh.overview.trend.coverageReasonIntro}</span>
      <ul className="flex flex-col gap-1.5">
        {flagged.map(({ row, explanation }) => (
          <li
            key={row.label}
            data-testid="trend-coverage-reason-row"
            data-bucket={row.label}
            data-coverage={row.coverage}
            data-in-progress={explanation.inProgress ? 'true' : 'false'}
            className="flex flex-wrap items-baseline gap-x-3 gap-y-1"
          >
            <span className="shrink-0 font-medium tabular-nums">{axisTick(row.label)}</span>
            <span className="shrink-0 text-muted-foreground">{COVERAGE_LABEL[row.coverage]}</span>
            {explanation.inProgress ? (
              <span
                data-testid="coverage-reason-in-progress-tag"
                className="shrink-0 text-muted-foreground"
              >
                {zh.overview.trend.coverageInProgressTag}
              </span>
            ) : null}
            {explanation.unknown ? (
              <span data-testid="coverage-reason-unknown" className="text-muted-foreground">
                {zh.overview.trend.coverageReasonUnknown}
              </span>
            ) : (
              <CoverageReasonPairs pairs={explanation.pairs} />
            )}
          </li>
        ))}
      </ul>
      <span className="text-[0.7rem] text-muted-foreground">
        {zh.overview.trend.coverageReasonHint}
      </span>
    </section>
  )
}

function CoverageLegend() {
  return (
    <div className="flex flex-wrap items-center gap-4 text-xs text-muted-foreground">
      <span className="flex items-center gap-1.5">
        <svg aria-hidden width="18" height="12" className="shrink-0">
          <rect
            width="18"
            height="12"
            rx="2"
            fill={`url(#${GAP_PATTERN_ID})`}
            stroke={CHART_TOKENS.coverageGap}
            strokeOpacity={0.35}
            strokeDasharray="3 3"
          />
        </svg>
        {zh.overview.trend.legendGap}
      </span>
      <span className="flex items-center gap-1.5">
        <svg aria-hidden width="18" height="12" className="shrink-0">
          <rect
            width="18"
            height="12"
            rx="2"
            fill={`url(#${PARTIAL_PATTERN_ID})`}
            stroke={CHART_TOKENS.coveragePartial}
            strokeOpacity={0.5}
            opacity={0.65}
          />
        </svg>
        {zh.overview.trend.legendPartial}
      </span>
    </div>
  )
}

function HatchPatterns() {
  return (
    <>
      <pattern
        id={GAP_PATTERN_ID}
        width={8}
        height={8}
        patternTransform="rotate(45)"
        patternUnits="userSpaceOnUse"
      >
        <rect width={8} height={8} fill={CHART_TOKENS.coverageGap} fillOpacity={0.07} />
        <line
          x1={0}
          y1={0}
          x2={0}
          y2={8}
          stroke={CHART_TOKENS.coverageGap}
          strokeOpacity={0.45}
          strokeWidth={1.75}
        />
      </pattern>
      <pattern
        id={PARTIAL_PATTERN_ID}
        width={6}
        height={6}
        patternTransform="rotate(45)"
        patternUnits="userSpaceOnUse"
      >
        <rect width={6} height={6} fill={CHART_TOKENS.coveragePartial} fillOpacity={0.35} />
        <line
          x1={0}
          y1={0}
          x2={0}
          y2={6}
          stroke={CHART_TOKENS.coveragePartial}
          strokeOpacity={0.9}
          strokeWidth={1}
        />
      </pattern>
    </>
  )
}

/**
 * The unfinished bucket's chip carries 进行中 so the always-visible surface still answers "why is
 * it not full" in three characters, now that the diagnosis panel no longer lists its pairs.
 */
function TrendNotes({ rows, nowMs }: { rows: TrendRow[]; nowMs: number }) {
  const flagged = rows.filter((row) => row.coverage !== 'full' || unavailableCount(row) > 0)
  if (flagged.length === 0) return null
  return (
    <div
      data-testid="trend-notes"
      className="flex flex-wrap items-center gap-2 border-t border-border pt-3"
    >
      {flagged.map((row) => {
        const inProgress = row.coverage !== 'full' && isBucketInProgress(row, nowMs)
        return (
          <span
            key={row.label}
            data-testid="trend-note"
            data-bucket={row.label}
            data-coverage={row.coverage}
            data-in-progress={inProgress ? 'true' : 'false'}
            className="inline-flex items-center gap-2 rounded-md bg-muted/60 px-2 py-1 text-xs"
          >
            <span className="font-medium tabular-nums">{axisTick(row.label)}</span>
            {row.coverage === 'full' ? null : (
              <span className="text-muted-foreground">{COVERAGE_LABEL[row.coverage]}</span>
            )}
            {inProgress ? (
              <span data-testid="trend-note-in-progress" className="text-muted-foreground">
                {zh.overview.trend.coverageInProgressTag}
              </span>
            ) : null}
            {unavailableCount(row) > 0 ? <CostPartialBadge /> : null}
          </span>
        )
      })}
    </div>
  )
}

function ModeGroup<T extends string>({
  label,
  values,
  active,
  testIdPrefix,
  labels,
  onSelect,
}: {
  label: string
  values: readonly T[]
  active: T
  testIdPrefix: string
  labels: Record<T, string>
  onSelect: (value: T) => void
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className="inline-flex rounded-lg bg-muted p-0.5 ring-1 ring-inset ring-foreground/5"
    >
      {values.map((candidate) => (
        <Button
          key={candidate}
          data-testid={`${testIdPrefix}-${candidate}`}
          size="sm"
          variant={candidate === active ? 'default' : 'ghost'}
          aria-pressed={candidate === active}
          onClick={() => onSelect(candidate)}
        >
          {labels[candidate]}
        </Button>
      ))}
    </div>
  )
}

function TrendControls({
  metric,
  onMetric,
  mode,
  onMode,
  hint,
}: {
  metric: TrendMetric
  onMetric: (metric: TrendMetric) => void
  mode: TrendGroupMode
  onMode: (mode: TrendGroupMode) => void
  hint: string
}) {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-medium tracking-wide text-muted-foreground">
            {zh.overview.trend.metricLabel}
          </span>
          <ModeGroup
            label={zh.overview.trend.metricLabel}
            values={TREND_METRICS}
            active={metric}
            testIdPrefix="trend-metric"
            labels={METRIC_LABEL}
            onSelect={onMetric}
          />
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-medium tracking-wide text-muted-foreground">
            {zh.overview.trend.groupLabel}
          </span>
          <ModeGroup
            label={zh.overview.trend.groupLabel}
            values={TREND_GROUP_MODES}
            active={mode}
            testIdPrefix="trend-group"
            labels={GROUP_LABEL}
            onSelect={onMode}
          />
        </div>
      </div>
      <p data-testid="trend-metric-hint" className="text-xs text-muted-foreground">
        {METRIC_HINT[metric]}
        <span className="px-1.5 text-muted-foreground/50">·</span>
        <span data-testid="trend-group-hint">{hint}</span>
      </p>
    </div>
  )
}

/**
 * NOT a component, and it must stay that way: `ComposedChart` decides which axes, grids and
 * series exist by walking its children and matching `type.displayName`. recharts' `toArray`
 * flattens a Fragment but pushes a custom element through unchanged, so `<CoverageAxes />`
 * would be an unrecognised child and the grid, both `YAxis` and the coverage-band `Bar` would
 * all be silently dropped — the chart still draws its lines, so nothing throws. Calling this as
 * a plain function returns the Fragment itself, which recharts does flatten.
 */
function coverageAxes({ metric, axisMax }: { metric: TrendMetric; axisMax: number }) {
  return (
    <>
      <CartesianGrid vertical={false} strokeDasharray="2 4" stroke={CHART_TOKENS.grid} />
      <XAxis
        dataKey="label"
        tickFormatter={axisTick}
        tickLine={false}
        axisLine={{ stroke: CHART_TOKENS.grid }}
        tick={{ fill: CHART_TOKENS.axis, fontSize: 11 }}
        minTickGap={8}
      />
      <YAxis
        yAxisId="value"
        domain={[0, axisMax]}
        tickFormatter={(value: number) =>
          metric === 'cost' ? formatMoney(value) : formatCompact(value)
        }
        tickLine={false}
        axisLine={false}
        width={metric === 'cost' ? 68 : 44}
        tick={{ fill: CHART_TOKENS.axis, fontSize: 11 }}
      />
      <YAxis yAxisId="band" hide domain={[0, 1]} />
      <Bar
        yAxisId="band"
        dataKey="coverageBand"
        shape={<CoverageBand />}
        isAnimationActive={false}
        legendType="none"
      />
    </>
  )
}

function TotalChart({
  rows,
  metric,
  seriesKeys,
  axisMax,
  reasons,
  nowMs,
}: {
  rows: TrendRow[]
  metric: TrendMetric
  seriesKeys: TrendSeriesKey[]
  axisMax: number
  reasons: CoverageReasonIndex
  nowMs: number
}) {
  return (
    <ResponsiveContainer width="100%" height="100%">
      <ComposedChart data={rows} margin={CHART_MARGIN} barCategoryGap={0}>
        <defs>
          <HatchPatterns />
          {seriesKeys.map((key) => (
            <linearGradient key={key} id={`overview-area-${key}`} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={SERIES_COLOR[key]} stopOpacity={0.35} />
              <stop offset="100%" stopColor={SERIES_COLOR[key]} stopOpacity={0.02} />
            </linearGradient>
          ))}
        </defs>

        {coverageAxes({ metric, axisMax })}

        {seriesKeys.map((key) => (
          <Area
            key={key}
            yAxisId="value"
            type="monotone"
            dataKey={SERIES_DATA_KEY[key]}
            name={SERIES_LABEL[key]}
            connectNulls={false}
            isAnimationActive={false}
            stroke={SERIES_COLOR[key]}
            strokeWidth={2}
            fill={`url(#overview-area-${key})`}
            dot={<TrendDot seriesKey={key} />}
            activeDot={false}
          />
        ))}

        <Tooltip
          isAnimationActive={false}
          cursor={{ stroke: CHART_TOKENS.axis, strokeOpacity: 0.25 }}
          content={<TrendTooltip rows={rows} metric={metric} reasons={reasons} nowMs={nowMs} />}
        />
      </ComposedChart>
    </ResponsiveContainer>
  )
}

interface GroupedSeries {
  key: string
  label: string
  color: string
  isOther: boolean
}

function GroupedTooltip({
  active,
  label,
  rows,
  series,
  metric,
  totals,
  reasons,
  nowMs,
}: {
  active?: boolean
  label?: string | number
  rows?: GroupedTrendRow[]
  series?: GroupedSeries[]
  metric?: TrendMetric
  totals?: TrendRow[]
  reasons?: CoverageReasonIndex
  nowMs?: number
}) {
  if (
    active !== true ||
    typeof label !== 'string' ||
    rows === undefined ||
    series === undefined ||
    metric === undefined
  ) {
    return null
  }
  const row = rows.find((candidate) => candidate.label === label)
  if (row === undefined) return null
  const total = totals?.find((candidate) => candidate.label === label)
  const missing = row.coverage === 'none'
  // `GroupedTrendRow` carries no bucket edges — coverage is a property of the window, not of the
  // grouping, so the phase test reads the same ungrouped total row the coverage state came from.
  const explanation =
    row.coverage === 'full' || reasons === undefined || nowMs === undefined || total === undefined
      ? undefined
      : coverageExplanationFor(reasons, total, nowMs)

  return (
    <div
      data-testid="trend-group-tooltip"
      data-bucket={row.label}
      data-coverage={row.coverage}
      className="min-w-64 rounded-lg border border-border bg-popover/95 p-3 text-xs shadow-raised backdrop-blur-sm"
    >
      <div className="flex items-center justify-between gap-3 border-b border-border pb-2">
        <span className="font-heading text-sm font-medium">{row.label}</span>
        <span
          data-testid="trend-group-tooltip-coverage"
          className={cn(
            'rounded-full px-2 py-0.5 text-[0.7rem] font-medium ring-1',
            missing
              ? 'bg-muted text-muted-foreground ring-border'
              : row.coverage === 'partial'
                ? 'bg-chart-1/40 text-foreground ring-foreground/15'
                : 'bg-foreground/5 text-foreground ring-foreground/10',
          )}
        >
          {COVERAGE_LABEL[row.coverage]}
        </span>
      </div>

      {missing ? (
        <>
          <p data-testid="trend-group-tooltip-gap-note" className="pt-2 text-muted-foreground">
            {zh.overview.trend.tooltipNoCoverage}
          </p>
          {explanation === undefined ? null : <CoverageReasonNote explanation={explanation} />}
        </>
      ) : (
        <div className="flex flex-col gap-1 pt-2">
          {series.map((entry) => (
            <div
              key={entry.key}
              data-testid="trend-group-tooltip-row"
              data-series={entry.label}
              className="flex items-baseline justify-between gap-6"
            >
              <span className="flex min-w-0 items-center gap-1.5 text-muted-foreground">
                <span
                  aria-hidden
                  className="size-2 shrink-0 rounded-full"
                  style={{ background: entry.color }}
                />
                <span className="truncate">{entry.label}</span>
              </span>
              <span className="font-medium tabular-nums">
                {formatValue(metric, row.values[entry.key] ?? 0)}
              </span>
            </div>
          ))}
          {total?.cost !== null && total !== undefined && metric === 'cost' ? (
            <div className="mt-1 flex flex-col gap-1 border-t border-border pt-2">
              <TooltipRow
                label={SERIES_LABEL.estimated}
                value={formatMoney(total.cost.estimatedSum)}
              />
              <TooltipRow
                label={zh.overview.summary.costUnavailableLabel}
                value={formatCount(total.cost.unavailableCount)}
              />
              {total.cost.unavailableCount > 0 ? <CostPartialBadge className="self-start" /> : null}
            </div>
          ) : null}
          {explanation === undefined ? null : <CoverageReasonNote explanation={explanation} />}
        </div>
      )}
    </div>
  )
}

/**
 * Legend entries are the chart's filter control, so each one is a real `<button>` rather than an
 * `<li>` with a click handler: that is what gives keyboard focus, Enter/Space activation and an
 * announced pressed state for free. `data-testid` and the data attributes stay on the button so
 * they remain on the element a test clicks and reads.
 *
 * 其他 is deliberately selectable too. It is `total − Σ kept`, and "how much did the folded-away
 * sources add up to" is a question worth isolating.
 */
function GroupedLegend({
  series,
  rows,
  metric,
  selectedKey,
  onSelect,
}: {
  series: GroupedSeries[]
  rows: GroupedTrendRow[]
  metric: TrendMetric
  selectedKey: string | null
  onSelect: (key: string) => void
}) {
  return (
    <ul
      data-testid="trend-group-legend"
      data-selected-series={selectedKey ?? ''}
      className="grid gap-x-5 gap-y-1.5 sm:grid-cols-2 lg:grid-cols-3"
    >
      {series.map((entry) => {
        const total = groupedSeriesTotal(rows, entry.key)
        const selected = entry.key === selectedKey
        const dimmed = selectedKey !== null && !selected
        return (
          <li key={entry.key} className="flex min-w-0">
            <button
              type="button"
              data-testid="trend-group-legend-item"
              data-series={entry.label}
              data-other={entry.isOther ? 'true' : 'false'}
              data-total-value={total === null ? '' : String(total)}
              data-selected={selected ? 'true' : 'false'}
              aria-pressed={selected}
              title={zh.overview.trend.legendItemTitle(entry.label)}
              onClick={() => onSelect(entry.key)}
              className={cn(
                'flex min-w-0 flex-1 items-baseline gap-2 rounded-md px-1.5 py-0.5 text-left text-xs transition-colors',
                'hover:bg-foreground/10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none',
                selected && 'bg-muted ring-1 ring-inset ring-foreground/15',
                dimmed && 'opacity-45',
              )}
            >
              {/* An <svg> line, not a coloured <span>: 其他 is stroked dashed on the chart, and a
                solid swatch would break the legend's one-to-one mapping onto the lines. */}
              <svg aria-hidden width="16" height="8" className="mt-1 shrink-0 overflow-visible">
                <line
                  x1={0}
                  y1={4}
                  x2={16}
                  y2={4}
                  stroke={entry.color}
                  strokeWidth={entry.isOther ? 1.5 : 2}
                  strokeDasharray={entry.isOther ? '4 3' : undefined}
                  strokeLinecap="round"
                />
              </svg>
              <span className="min-w-0 flex-1 truncate">{entry.label}</span>
              <span className="shrink-0 font-medium tabular-nums text-muted-foreground">
                {total === null ? '—' : formatValue(metric, total)}
              </span>
            </button>
          </li>
        )
      })}
    </ul>
  )
}

function LegendSelectionBar({
  selectedLabel,
  onClear,
}: {
  selectedLabel: string | null
  onClear: () => void
}) {
  if (selectedLabel === null) {
    return (
      <p data-testid="trend-legend-hint" className="text-xs text-muted-foreground">
        {zh.overview.trend.legendSelectHint}
      </p>
    )
  }
  return (
    <div className="flex flex-wrap items-center justify-between gap-2">
      <p data-testid="trend-legend-selected" className="text-xs text-muted-foreground">
        {zh.overview.trend.legendSelectedHint(selectedLabel)}
      </p>
      <Button data-testid="trend-legend-show-all" size="sm" variant="ghost" onClick={onClear}>
        {zh.overview.trend.legendShowAll}
      </Button>
    </div>
  )
}

function GroupedChart({
  rows,
  series,
  metric,
  axisMax,
  totals,
  reasons,
  nowMs,
}: {
  rows: GroupedTrendRow[]
  series: GroupedSeries[]
  metric: TrendMetric
  axisMax: number
  totals: TrendRow[]
  reasons: CoverageReasonIndex
  nowMs: number
}) {
  return (
    <ResponsiveContainer width="100%" height="100%">
      <ComposedChart data={rows} margin={CHART_MARGIN} barCategoryGap={0}>
        <defs>
          <HatchPatterns />
        </defs>

        {coverageAxes({ metric, axisMax })}

        {series.map((entry) => (
          <Line
            key={entry.key}
            yAxisId="value"
            type="monotone"
            /* A function dataKey, not `values.${key}`: recharts resolves a dotted string as a
               key path, and a model id such as `gpt-4.1` would silently read as a nested one. */
            dataKey={(row: GroupedTrendRow) => row.values[entry.key] ?? null}
            name={entry.label}
            connectNulls={false}
            isAnimationActive={false}
            stroke={entry.color}
            strokeWidth={entry.isOther ? 1.5 : 2}
            strokeDasharray={entry.isOther ? '4 3' : undefined}
            dot={false}
            activeDot={{
              r: 3.5,
              strokeWidth: 2,
              fill: CHART_TOKENS.surface,
              stroke: entry.color,
            }}
          />
        ))}

        <Tooltip
          isAnimationActive={false}
          cursor={{ stroke: CHART_TOKENS.axis, strokeOpacity: 0.25 }}
          content={
            <GroupedTooltip
              rows={rows}
              series={series}
              metric={metric}
              totals={totals}
              reasons={reasons}
              nowMs={nowMs}
            />
          }
        />
      </ComposedChart>
    </ResponsiveContainer>
  )
}

export interface TrendGroupingState {
  mode: TrendGroupMode
  onModeChange: (mode: TrendGroupMode) => void
  /** Kept groups only; the tail is summarised by `droppedCount`. */
  parts: TrendGroupPart[]
  droppedCount: number
  /** Number of dimension values in the window, before the Top-N cut. */
  totalCount: number
  isPending: boolean
}

/**
 * Legend selection is stored together with the group mode it was made in, and read back only when
 * the modes still match. Group keys are index-derived (`g0`, `g1`, …), so a selection carried
 * across a mode switch would resolve to a different dimension's line; pairing it with the mode
 * makes the reset a derivation instead of an effect that can fire a render late.
 */
interface LegendSelection {
  mode: TrendGroupMode
  key: string
}

export function TrendChart({
  rows,
  grouping,
  coverageNotes = [],
}: {
  rows: TrendRow[]
  grouping: TrendGroupingState
  coverageNotes?: readonly CoverageNote[]
}) {
  const [metric, setMetric] = useState<TrendMetric>('tokens')
  const [selection, setSelection] = useState<LegendSelection | null>(null)
  // Read per render rather than memoised: a memo would freeze the instant and keep calling a
  // long-since-ended bucket "in progress". Staleness between renders is bounded by the data's own
  // staleness — a bucket that ends while the window sits idle is reclassified on the next refresh.
  const nowMs = Date.now()
  const reasons = useMemo(() => coverageReasonIndex(coverageNotes), [coverageNotes])
  const seriesKeys = useMemo(() => seriesKeysFor(metric), [metric])
  const axisMax = useMemo(() => valueAxisMax(rows, metric), [rows, metric])
  const grouped = useMemo(
    () =>
      grouping.mode === 'none'
        ? null
        : buildGroupedTrend({
            total: rows,
            parts: grouping.parts,
            metric,
            droppedCount: grouping.droppedCount,
            palette: SERIES_PALETTE,
            otherColor: OTHER_SERIES_COLOR,
            otherLabel: zh.overview.trend.groupOther,
          }),
    [rows, grouping.mode, grouping.parts, grouping.droppedCount, metric],
  )
  const covered = hasAnyCoverage(rows)
  const selectedKey = selection !== null && selection.mode === grouping.mode ? selection.key : null
  const shownSeries = grouped === null ? [] : visibleSeries(grouped.series, selectedKey)
  const selectedLabel =
    grouped === null
      ? null
      : (grouped.series.find((entry) => entry.key === selectedKey)?.label ?? null)

  if (rows.length === 0) {
    return <EmptyState label={zh.overview.trend.empty} />
  }

  const primaryKey = seriesKeys[0]
  const lastRow = rows[rows.length - 1]

  return (
    <div
      data-testid="overview-trend"
      data-group-mode={grouping.mode}
      className="flex flex-col gap-4"
    >
      <TrendControls
        metric={metric}
        onMetric={setMetric}
        mode={grouping.mode}
        onMode={grouping.onModeChange}
        hint={GROUP_HINT[grouping.mode]}
      />

      {covered ? null : (
        <p data-testid="trend-all-gap" className="text-xs text-muted-foreground">
          {zh.overview.trend.allGap}
        </p>
      )}

      {grouping.mode !== 'none' && grouping.droppedCount > 0 ? (
        <p data-testid="trend-group-topn" className="text-xs text-muted-foreground">
          {zh.overview.trend.groupOtherHint(grouping.parts.length, grouping.totalCount)}
        </p>
      ) : null}
      {grouping.mode !== 'none' && grouping.droppedCount === 0 && grouping.totalCount > 0 ? (
        <p data-testid="trend-group-topn" className="text-xs text-muted-foreground">
          {zh.overview.trend.groupTopHint(grouping.totalCount)}
        </p>
      ) : null}

      {grouping.mode !== 'none' && grouping.isPending ? (
        <LoadingState label={zh.overview.trend.groupLoading} />
      ) : grouping.mode !== 'none' && grouping.totalCount === 0 ? (
        <EmptyState label={zh.overview.trend.groupEmpty} />
      ) : (
        <>
          <div style={{ height: CHART_HEIGHT }}>
            {grouped === null ? (
              <TotalChart
                rows={rows}
                metric={metric}
                seriesKeys={seriesKeys}
                axisMax={axisMax}
                reasons={reasons}
                nowMs={nowMs}
              />
            ) : (
              <GroupedChart
                rows={grouped.rows}
                series={shownSeries}
                metric={metric}
                axisMax={grouped.axisMax}
                totals={rows}
                reasons={reasons}
                nowMs={nowMs}
              />
            )}
          </div>

          {grouped === null ? (
            <div className="flex flex-wrap items-center justify-between gap-3">
              <CoverageLegend />
              <span className="text-xs text-muted-foreground">
                {SERIES_LABEL[primaryKey]}
                {rowValue(lastRow, primaryKey) !== null
                  ? ` · ${formatValue(metric, rowValue(lastRow, primaryKey) ?? 0)}`
                  : ''}
              </span>
            </div>
          ) : (
            <div className="flex flex-col gap-3 border-t border-border pt-3">
              <GroupedLegend
                series={grouped.series}
                rows={grouped.rows}
                metric={metric}
                selectedKey={selectedKey}
                onSelect={(key) =>
                  setSelection((current) =>
                    current !== null && current.mode === grouping.mode && current.key === key
                      ? null
                      : { mode: grouping.mode, key },
                  )
                }
              />
              <LegendSelectionBar
                selectedLabel={selectedLabel}
                onClear={() => setSelection(null)}
              />
              <div className="flex flex-wrap items-center justify-between gap-3">
                <CoverageLegend />
                <span className="text-xs text-muted-foreground">
                  {metric === 'cost' ? zh.overview.trend.groupCostSeriesHint : ''}
                  {grouping.droppedCount > 0 ? ` ${zh.overview.trend.groupOtherNote}` : ''}
                </span>
              </div>
            </div>
          )}
        </>
      )}

      <TrendNotes rows={rows} nowMs={nowMs} />
      <CoverageReasons rows={rows} reasons={reasons} nowMs={nowMs} />
    </div>
  )
}
