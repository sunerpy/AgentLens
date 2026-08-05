/**
 * EXCLUSIVE FILE BOUNDARY — todo 16 owns `src/views/drilldown/**` and the `zh.drilldown`
 * dictionary section. No other worker edits this directory; this worker edits no shell file.
 *
 * Three levels over one `get_breakdown` call:
 *   1. `source` (+ host filter)   2. `agent_key`   3. `(provider_id, model_id)` + variant
 *
 * Why a single query drives all three levels: `expandVariant: true` returns the finest grain
 * the backend has, and `source` / `agentKey` / `providerId` / `modelId` / `variant` all travel
 * on the row, so drilling is a lossless client-side selection. Re-querying with
 * `filters.agentKey` set would also discard the sibling rows the share bars are computed
 * from. `hostId` is the exception — `BreakdownRow` carries no host — so the host filter is a
 * real server-side filter and part of the query key.
 *
 * Range, timezone and week start come from `useReportRange()`; that shared state is how this
 * view stays in lock-step with the overview instead of owning a second range widget.
 */
import { useQuery } from '@tanstack/react-query'
import { ChevronRight } from 'lucide-react'
import { useMemo, useState } from 'react'

import { useReportRange } from '@/app/reportRange'
import { EmptyState, ErrorState, LoadingState } from '@/components/app-state'
import type { BreakdownDimensions, BreakdownRow, Host } from '@/generated'
import { zh } from '@/i18n/zh'
import { archiveQueryKey } from '@/lib/archiveQueries'
import { getBreakdown, hostsList } from '@/lib/ipc'

import { groupByAgentKey, groupByModel, groupBySource, sumMetrics, tokenTotal } from './aggregate'
import { AgentLevel, HostFilter, ModelLevel, SourceLevel } from './DrilldownLevels'
import { formatAmount, formatCount } from './format'

const NO_HOSTS: Host[] = []

function StatChip({ label, value }: { label: string; value: string }) {
  return (
    <span className="flex flex-col gap-0.5 rounded-lg border border-border bg-card px-3 py-1.5">
      <span className="text-[11px] text-muted-foreground">{label}</span>
      <span className="text-sm font-semibold tabular-nums">{value}</span>
    </span>
  )
}

export function DrilldownView() {
  const { range, timezone } = useReportRange()
  const [hostId, setHostId] = useState<string | null>(null)
  const [pickedSource, setPickedSource] = useState<string | null>(null)
  const [pickedAgentKey, setPickedAgentKey] = useState<string | null>(null)
  const [expandedKeys, setExpandedKeys] = useState<readonly string[]>([])

  const hostsQuery = useQuery({ queryKey: ['hosts'], queryFn: hostsList })

  const breakdownQuery = useQuery({
    queryKey: archiveQueryKey(
      'breakdown',
      range.startDate,
      range.endDateExclusive,
      range.weekStart,
      timezone,
      hostId,
    ),
    queryFn: () => {
      const dims: BreakdownDimensions = {
        timezone,
        filters: { hostId, source: null, agentKey: null, providerId: null, modelId: null },
        expandVariant: true,
      }
      return getBreakdown(range, dims)
    },
  })

  const rows = useMemo<BreakdownRow[]>(() => breakdownQuery.data ?? [], [breakdownQuery.data])
  const sources = useMemo(() => groupBySource(rows), [rows])

  /**
   * Selection is derived, not stored: when a range or host change makes the picked source or
   * agent disappear, the lower levels fall back to the strongest remaining row instead of
   * pointing at rows that are no longer in the result.
   */
  const activeSource =
    sources.find((node) => node.source === pickedSource)?.source ?? sources[0]?.source ?? null
  const sourceRows = useMemo(
    () => rows.filter((row) => row.source === activeSource),
    [rows, activeSource],
  )
  const agents = useMemo(() => groupByAgentKey(sourceRows), [sourceRows])
  const activeAgentKey =
    agents.find((node) => node.agentKey === pickedAgentKey)?.agentKey ?? agents[0]?.agentKey ?? null
  const agentRows = useMemo(
    () => sourceRows.filter((row) => row.agentKey === activeAgentKey),
    [sourceRows, activeAgentKey],
  )
  const models = useMemo(() => groupByModel(agentRows), [agentRows])
  const filteredTotal = useMemo(() => sumMetrics(rows), [rows])
  const activeAgentRaw = agents.find((node) => node.agentKey === activeAgentKey)?.agentRaw ?? null

  const toggleExpand = (key: string) => {
    setExpandedKeys((current) =>
      current.includes(key) ? current.filter((entry) => entry !== key) : [...current, key],
    )
  }

  return (
    <section data-testid="view-drilldown" className="flex flex-col gap-5">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div className="flex flex-col gap-1">
          <h2 className="text-2xl font-semibold tracking-tight">{zh.drilldown.title}</h2>
          <p className="text-sm text-muted-foreground">{zh.drilldown.subtitle}</p>
        </div>
        <div className="flex flex-wrap items-stretch gap-2">
          <HostFilter
            hosts={hostsQuery.data ?? NO_HOSTS}
            hostId={hostId}
            onSelectHost={(next) => {
              setHostId(next)
              setPickedSource(null)
              setPickedAgentKey(null)
              setExpandedKeys([])
            }}
            unavailable={hostsQuery.isError}
          />
          <dl
            data-testid="drilldown-range-chip"
            className="flex items-center gap-4 rounded-lg border border-border bg-card px-3 py-2 text-xs"
          >
            <div className="flex flex-col gap-0.5">
              <dt className="text-[11px] text-muted-foreground">{zh.drilldown.rangeLabel}</dt>
              <dd data-testid="drilldown-range-value" className="font-mono tabular-nums">
                {range.startDate}
                <span className="px-1 text-muted-foreground/60">→</span>
                {range.endDateExclusive}
              </dd>
            </div>
            <div className="flex flex-col gap-0.5">
              <dt className="text-[11px] text-muted-foreground">{zh.drilldown.timezoneLabel}</dt>
              <dd data-testid="drilldown-timezone-value" className="font-mono">
                {timezone}
              </dd>
            </div>
          </dl>
        </div>
      </header>

      {breakdownQuery.isPending ? <LoadingState /> : null}

      {breakdownQuery.isError ? (
        <ErrorState error={breakdownQuery.error} onRetry={() => void breakdownQuery.refetch()} />
      ) : null}

      {breakdownQuery.isSuccess && rows.length === 0 ? (
        <EmptyState label={zh.drilldown.empty}>
          <span className="text-xs">{zh.drilldown.emptyHint}</span>
        </EmptyState>
      ) : null}

      {breakdownQuery.isSuccess && rows.length > 0 ? (
        <>
          <div className="flex flex-wrap items-center gap-3">
            <nav
              data-testid="drilldown-breadcrumb"
              aria-label={zh.drilldown.breadcrumbLabel}
              className="flex min-w-0 items-center gap-1.5 text-sm"
            >
              <span className="truncate font-medium">{activeSource}</span>
              <ChevronRight
                aria-hidden="true"
                className="size-3.5 shrink-0 text-muted-foreground"
              />
              <span className="truncate font-medium">{activeAgentRaw}</span>
              <ChevronRight
                aria-hidden="true"
                className="size-3.5 shrink-0 text-muted-foreground"
              />
              <span className="shrink-0 tabular-nums text-muted-foreground">
                {formatCount(models.length)} {zh.drilldown.modelsLabel}
              </span>
            </nav>
            <div className="ml-auto flex flex-wrap items-center gap-2">
              <span className="text-[11px] text-muted-foreground">{zh.drilldown.summaryTitle}</span>
              <StatChip
                label={zh.drilldown.tokenTotalLabel}
                value={formatCount(tokenTotal(filteredTotal.tokens))}
              />
              <StatChip
                label={zh.common.cost.actual}
                value={formatAmount(filteredTotal.cost.actualSum)}
              />
              <StatChip
                label={zh.common.cost.estimated}
                value={formatAmount(filteredTotal.cost.estimatedSum)}
              />
              <StatChip
                label={zh.common.messageCount}
                value={formatCount(filteredTotal.messageCount)}
              />
            </div>
          </div>

          <SourceLevel
            nodes={sources}
            activeSource={activeSource}
            onSelectSource={(source) => {
              setPickedSource(source)
              setPickedAgentKey(null)
              setExpandedKeys([])
            }}
          />

          <AgentLevel
            nodes={agents}
            activeAgentKey={activeAgentKey}
            onSelectAgent={(agentKey) => {
              setPickedAgentKey(agentKey)
              setExpandedKeys([])
            }}
          />

          <ModelLevel nodes={models} expandedKeys={expandedKeys} onToggleExpand={toggleExpand} />
        </>
      ) : null}
    </section>
  )
}
