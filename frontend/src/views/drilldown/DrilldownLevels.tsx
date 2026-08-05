/**
 * EXCLUSIVE FILE BOUNDARY — todo 16 owns `src/views/drilldown/**`.
 *
 * The three level tables live in one module on purpose: they must present an identical
 * column contract (four token groups, three cost buckets, message count, share bar) and
 * keeping them adjacent is what makes a divergence obvious in review.
 */
import { ChevronDown, ChevronRight } from 'lucide-react'
import { Fragment } from 'react'

import type { Host } from '@/generated'
import { zh } from '@/i18n/zh'

import type { AgentNode, ModelNode, SourceNode } from './aggregate'
import { shareOf, tokenTotal } from './aggregate'
import { LevelCard, MetaChip, MetricCells, MetricHeadCells } from './DrilldownMetrics'
import { formatCount } from './format'

const NAME_HEAD = 'px-4 py-2 text-left text-xs font-medium text-muted-foreground'
const ROW =
  'border-t border-border data-[selected=true]:bg-accent hover:bg-muted/50 transition-colors'

function DrillName({
  label,
  secondary,
  selected,
  onSelect,
}: {
  label: string
  secondary: string
  selected: boolean
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={selected}
      title={zh.drilldown.drillHint}
      className="flex max-w-full flex-col items-start gap-0.5 rounded-md px-1 py-0.5 text-left outline-ring/50 focus-visible:outline-2"
    >
      <span className="truncate text-sm font-medium">{label}</span>
      <span className="truncate font-mono text-[11px] text-muted-foreground">{secondary}</span>
    </button>
  )
}

function levelTotal(weights: number[]): number {
  return weights.reduce((sum, value) => sum + value, 0)
}

/** MUST stay outside the level cards: those unmount on an empty result, and a filter that
 * unmounted with them would trap the user in a range with no rows. */
export function HostFilter({
  hosts,
  hostId,
  onSelectHost,
  unavailable,
}: {
  hosts: Host[]
  hostId: string | null
  onSelectHost: (hostId: string | null) => void
  unavailable: boolean
}) {
  return (
    <label className="flex items-center gap-2 rounded-lg border border-border bg-card px-3 py-2 text-xs">
      <span className="text-muted-foreground">{zh.drilldown.hostFilter}</span>
      <select
        data-testid="drilldown-host-filter"
        value={hostId ?? ''}
        disabled={unavailable}
        onChange={(event) => onSelectHost(event.target.value === '' ? null : event.target.value)}
        className="rounded-md border border-border bg-background px-2 py-1 text-xs text-foreground disabled:opacity-60"
      >
        <option value="">
          {unavailable ? zh.drilldown.hostUnavailable : zh.drilldown.hostAll}
        </option>
        {hosts.map((host) => (
          <option key={host.hostId} value={host.hostId}>
            {host.displayName}
          </option>
        ))}
      </select>
    </label>
  )
}

export function SourceLevel({
  nodes,
  activeSource,
  onSelectSource,
}: {
  nodes: SourceNode[]
  activeSource: string | null
  onSelectSource: (source: string) => void
}) {
  const total = levelTotal(nodes.map((node) => tokenTotal(node.metric.tokens)))
  return (
    <LevelCard
      step={1}
      stepLabel={zh.drilldown.levelSourceStep}
      title={zh.drilldown.levelSourceTitle}
      hint={zh.drilldown.levelSourceHint}
      testId="drilldown-level-source"
      meta={<MetaChip label={zh.drilldown.sourcesLabel} value={nodes.length} />}
    >
      <table className="w-full border-collapse">
        <thead>
          <tr className="bg-background">
            <th scope="col" className={NAME_HEAD}>
              {zh.drilldown.columnSource}
            </th>
            <MetricHeadCells />
          </tr>
        </thead>
        <tbody>
          {nodes.map((node) => (
            <tr
              key={node.source}
              data-testid="drilldown-source-row"
              data-source={node.source}
              data-selected={node.source === activeSource}
              className={ROW}
            >
              <td className="px-4 py-2">
                <DrillName
                  label={node.source}
                  secondary={`${formatCount(node.agentKeyCount)} ${zh.drilldown.agentsLabel}`}
                  selected={node.source === activeSource}
                  onSelect={() => onSelectSource(node.source)}
                />
              </td>
              <MetricCells metric={node.metric} share={shareOf(node.metric, total)} />
            </tr>
          ))}
        </tbody>
      </table>
    </LevelCard>
  )
}

export function AgentLevel({
  nodes,
  activeAgentKey,
  onSelectAgent,
}: {
  nodes: AgentNode[]
  activeAgentKey: string | null
  onSelectAgent: (agentKey: string) => void
}) {
  const total = levelTotal(nodes.map((node) => tokenTotal(node.metric.tokens)))
  return (
    <LevelCard
      step={2}
      stepLabel={zh.drilldown.levelAgentStep}
      title={zh.drilldown.levelAgentTitle}
      hint={zh.drilldown.levelAgentHint}
      testId="drilldown-level-agent"
      meta={<MetaChip label={zh.drilldown.agentsLabel} value={nodes.length} />}
    >
      <table className="w-full border-collapse">
        <thead>
          <tr className="bg-background">
            <th scope="col" className={NAME_HEAD}>
              {zh.drilldown.columnAgent}
            </th>
            <MetricHeadCells />
          </tr>
        </thead>
        <tbody>
          {nodes.map((node) => (
            <tr
              key={node.agentKey}
              data-testid="drilldown-agent-row"
              data-agent-key={node.agentKey}
              data-selected={node.agentKey === activeAgentKey}
              className={ROW}
            >
              <td className="max-w-56 px-4 py-2">
                <DrillName
                  label={node.agentRaw}
                  secondary={node.agentKey}
                  selected={node.agentKey === activeAgentKey}
                  onSelect={() => onSelectAgent(node.agentKey)}
                />
              </td>
              <MetricCells metric={node.metric} share={shareOf(node.metric, total)} />
            </tr>
          ))}
        </tbody>
      </table>
    </LevelCard>
  )
}

export function ModelLevel({
  nodes,
  expandedKeys,
  onToggleExpand,
}: {
  nodes: ModelNode[]
  expandedKeys: readonly string[]
  onToggleExpand: (key: string) => void
}) {
  const total = levelTotal(nodes.map((node) => tokenTotal(node.metric.tokens)))
  return (
    <LevelCard
      step={3}
      stepLabel={zh.drilldown.levelModelStep}
      title={zh.drilldown.levelModelTitle}
      hint={zh.drilldown.levelModelHint}
      testId="drilldown-level-model"
      meta={<MetaChip label={zh.drilldown.modelsLabel} value={nodes.length} />}
    >
      <table className="w-full border-collapse">
        <thead>
          <tr className="bg-background">
            <th scope="col" className={NAME_HEAD}>
              {zh.drilldown.columnModel}
            </th>
            <MetricHeadCells />
          </tr>
        </thead>
        <tbody>
          {nodes.map((node) => {
            const expanded = expandedKeys.includes(node.key)
            return (
              <Fragment key={node.key}>
                <tr
                  data-testid="drilldown-model-row"
                  data-model-key={node.key}
                  data-expanded={expanded}
                  className={ROW}
                >
                  <td className="px-4 py-2">
                    <button
                      type="button"
                      data-testid="drilldown-model-expand"
                      onClick={() => onToggleExpand(node.key)}
                      aria-expanded={expanded}
                      aria-label={
                        expanded ? zh.drilldown.collapseVariants : zh.drilldown.expandVariants
                      }
                      className="flex max-w-full items-start gap-2 rounded-md px-1 py-0.5 text-left outline-ring/50 focus-visible:outline-2"
                    >
                      {expanded ? (
                        <ChevronDown className="mt-1 size-3.5 shrink-0 text-muted-foreground" />
                      ) : (
                        <ChevronRight className="mt-1 size-3.5 shrink-0 text-muted-foreground" />
                      )}
                      <span className="flex min-w-0 flex-col gap-0.5">
                        <span className="truncate text-sm font-medium">{node.modelId}</span>
                        <span className="truncate font-mono text-[11px] text-muted-foreground">
                          {node.providerId}
                          <span className="px-1 text-muted-foreground/60">·</span>
                          {formatCount(node.variants.length)} {zh.drilldown.variantsLabel}
                        </span>
                      </span>
                    </button>
                  </td>
                  <MetricCells metric={node.metric} share={shareOf(node.metric, total)} />
                </tr>
                {expanded
                  ? node.variants.map((variant) => (
                      <tr
                        key={`${node.key}\u0000${variant.variant ?? ''}`}
                        data-testid="drilldown-variant-row"
                        data-variant={variant.variant ?? ''}
                        className="border-t border-border bg-muted/30"
                      >
                        <td className="px-4 py-1.5 pl-11">
                          <span className="flex flex-col gap-0.5">
                            <span
                              data-testid="drilldown-variant-label"
                              className="truncate text-xs font-medium"
                            >
                              {variant.variant ?? zh.drilldown.variantNone}
                            </span>
                            <span className="truncate font-mono text-[11px] text-muted-foreground">
                              {formatCount(variant.metric.rowCount)} {zh.drilldown.rowsLabel}
                            </span>
                          </span>
                        </td>
                        <MetricCells
                          metric={variant.metric}
                          share={shareOf(variant.metric, total)}
                        />
                      </tr>
                    ))
                  : null}
              </Fragment>
            )
          })}
        </tbody>
      </table>
      <p className="border-t border-border px-4 py-2 text-[11px] text-muted-foreground">
        {zh.drilldown.shareNote}
      </p>
    </LevelCard>
  )
}
