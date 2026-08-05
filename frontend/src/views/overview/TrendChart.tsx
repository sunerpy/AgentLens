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
 */
import { useMemo, useState } from 'react'
import {
  Area,
  Bar,
  CartesianGrid,
  ComposedChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts'

import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/app-state'
import { zh } from '@/i18n/zh'
import { cn } from '@/lib/utils'
import { CostPartialBadge } from '@/views/overview/CostPartialBadge'
import { CHART_TOKENS, GAP_PATTERN_ID, PARTIAL_PATTERN_ID } from '@/views/overview/chartTokens'
import { formatCompact, formatCount, formatMoney } from '@/views/overview/format'
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

function formatValue(metric: TrendMetric, value: number): string {
  return metric === 'cost' ? formatMoney(value) : formatCount(value)
}

/** Rust emits `YYYY-MM-DDTHH:MM±HH:MM` for hour buckets; only the clock part is useful here. */
function axisTick(label: string): string {
  const separator = label.indexOf('T')
  return separator === -1 ? label : label.slice(separator + 1, separator + 6)
}

interface BandShapeProps {
  x?: number
  y?: number
  width?: number
  height?: number
  payload?: TrendRow
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
}

function TooltipRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-6">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-medium tabular-nums">{value}</span>
    </div>
  )
}

function TrendTooltip({ active, label, rows, metric }: TrendTooltipProps) {
  if (active !== true || typeof label !== 'string' || rows === undefined || metric === undefined) {
    return null
  }
  const row = rows.find((candidate) => candidate.label === label)
  if (row === undefined) return null

  const missing = row.coverage === 'none'
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
        <p data-testid="trend-tooltip-gap-note" className="pt-2 text-muted-foreground">
          {zh.overview.trend.tooltipNoCoverage}
        </p>
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
          {row.messageCount === null ? null : (
            <div className="mt-1 border-t border-border pt-2">
              <TooltipRow label={zh.common.messageCount} value={formatCount(row.messageCount)} />
            </div>
          )}
          {row.coverage === 'full' && row.tokens !== null && totalTokens(row.tokens) === 0 ? (
            <p data-testid="trend-tooltip-zero-note" className="pt-1 text-muted-foreground">
              {zh.overview.trend.tooltipZeroUsage}
            </p>
          ) : null}
        </div>
      )}
    </div>
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

function TrendNotes({ rows }: { rows: TrendRow[] }) {
  const flagged = rows.filter((row) => row.coverage !== 'full' || unavailableCount(row) > 0)
  if (flagged.length === 0) return null
  return (
    <div
      data-testid="trend-notes"
      className="flex flex-wrap items-center gap-2 border-t border-border pt-3"
    >
      {flagged.map((row) => (
        <span
          key={row.label}
          data-testid="trend-note"
          data-bucket={row.label}
          data-coverage={row.coverage}
          className="inline-flex items-center gap-2 rounded-md bg-muted/60 px-2 py-1 text-xs"
        >
          <span className="font-medium tabular-nums">{axisTick(row.label)}</span>
          {row.coverage === 'full' ? null : (
            <span className="text-muted-foreground">{COVERAGE_LABEL[row.coverage]}</span>
          )}
          {unavailableCount(row) > 0 ? <CostPartialBadge /> : null}
        </span>
      ))}
    </div>
  )
}

export function TrendChart({ rows }: { rows: TrendRow[] }) {
  const [metric, setMetric] = useState<TrendMetric>('tokens')
  const seriesKeys = useMemo(() => seriesKeysFor(metric), [metric])
  const axisMax = useMemo(() => valueAxisMax(rows, metric), [rows, metric])
  const covered = hasAnyCoverage(rows)

  if (rows.length === 0) {
    return <EmptyState label={zh.overview.trend.empty} />
  }

  const primaryKey = seriesKeys[0]

  return (
    <div data-testid="overview-trend" className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-0.5">
          <span className="text-xs font-medium tracking-wide text-muted-foreground">
            {zh.overview.trend.metricLabel}
          </span>
          <p data-testid="trend-metric-hint" className="text-xs text-muted-foreground">
            {METRIC_HINT[metric]}
          </p>
        </div>
        <div
          role="group"
          aria-label={zh.overview.trend.metricLabel}
          className="inline-flex rounded-lg bg-muted p-0.5"
        >
          {TREND_METRICS.map((candidate) => (
            <Button
              key={candidate}
              data-testid={`trend-metric-${candidate}`}
              size="sm"
              variant={candidate === metric ? 'default' : 'ghost'}
              aria-pressed={candidate === metric}
              onClick={() => setMetric(candidate)}
            >
              {METRIC_LABEL[candidate]}
            </Button>
          ))}
        </div>
      </div>

      {covered ? null : (
        <p data-testid="trend-all-gap" className="text-xs text-muted-foreground">
          {zh.overview.trend.allGap}
        </p>
      )}

      <div style={{ height: CHART_HEIGHT }}>
        <ResponsiveContainer width="100%" height="100%">
          <ComposedChart
            data={rows}
            margin={{ top: 8, right: 8, bottom: 4, left: 4 }}
            barCategoryGap={0}
          >
            <defs>
              <HatchPatterns />
              {seriesKeys.map((key) => (
                <linearGradient key={key} id={`overview-area-${key}`} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={SERIES_COLOR[key]} stopOpacity={0.35} />
                  <stop offset="100%" stopColor={SERIES_COLOR[key]} stopOpacity={0.02} />
                </linearGradient>
              ))}
            </defs>

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
              content={<TrendTooltip rows={rows} metric={metric} />}
            />
          </ComposedChart>
        </ResponsiveContainer>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-3">
        <CoverageLegend />
        <span className="text-xs text-muted-foreground">
          {SERIES_LABEL[primaryKey]}
          {rows.length > 0 && rowValue(rows[rows.length - 1], primaryKey) !== null
            ? ` · ${formatValue(metric, rowValue(rows[rows.length - 1], primaryKey) ?? 0)}`
            : ''}
        </span>
      </div>

      <TrendNotes rows={rows} />
    </div>
  )
}
