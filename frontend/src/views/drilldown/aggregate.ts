/**
 * EXCLUSIVE FILE BOUNDARY — todo 16 owns `src/views/drilldown/**`.
 *
 * Pure grouping math for the three drilldown levels. No React, no i18n, no IPC, so the
 * aggregation contract can be reasoned about (and asserted) independently of rendering.
 *
 * Contract decisions encoded here:
 * - Level 2 groups by `agentKey`, never by `agentRaw`. `agentKey` is the normalized join
 *   key produced by `agentlens_core::archive::normalize_agent_key`; two records of the same
 *   logical agent can carry different raw labels (`"Atlas - Plan Executor"` vs
 *   `"Atlas Plan Executor"`), and grouping on the raw string would split one agent into
 *   several rows. The raw label is display-only: the last row observed in backend order
 *   wins, which is the closest thing to "most recent" available in a `BreakdownRow`
 *   (the DTO carries no timestamp).
 * - Level 3 groups by `(providerId, modelId)` and keeps `variant` as expandable children,
 *   because prices are per model, not per reasoning-effort variant.
 * - Cost stays three-way separated (`actualSum` / `estimatedSum` / `unavailableCount`);
 *   nothing in this module ever adds actual to estimated or treats unavailable as 0 money.
 */
import type { BreakdownRow, CostTotals, TokenValues } from '@/generated'

export interface DrilldownMetric {
  tokens: TokenValues
  cost: CostTotals
  messageCount: number
  /** Records whose granularity is a whole session; disjoint from `messageCount`. */
  sessionRecordCount: number
  /** Number of archive breakdown rows folded into this metric. */
  rowCount: number
}

export interface SourceNode {
  source: string
  metric: DrilldownMetric
  agentKeyCount: number
}

export interface AgentNode {
  agentKey: string
  agentRaw: string
  metric: DrilldownMetric
  modelCount: number
}

export interface VariantNode {
  variant: string | null
  metric: DrilldownMetric
}

export interface ModelNode {
  key: string
  providerId: string
  modelId: string
  metric: DrilldownMetric
  variants: VariantNode[]
}

const ZERO_TOKENS: TokenValues = {
  tokInput: 0,
  tokOutput: 0,
  tokReasoning: 0,
  tokCacheRead: 0,
  tokCacheWrite: 0,
  totalInput: 0,
}

export function emptyMetric(): DrilldownMetric {
  return {
    tokens: { ...ZERO_TOKENS },
    cost: { actualSum: 0, estimatedSum: 0, unavailableCount: 0 },
    messageCount: 0,
    sessionRecordCount: 0,
    rowCount: 0,
  }
}

/**
 * Share denominator: every atomic token bucket summed.
 *
 * Deliberately not `totalInput` (= input + cacheRead + cacheWrite), which excludes output
 * and reasoning and would make a generation-heavy row look small.
 */
export function tokenTotal(tokens: TokenValues): number {
  return (
    tokens.tokInput +
    tokens.tokOutput +
    tokens.tokReasoning +
    tokens.tokCacheRead +
    tokens.tokCacheWrite
  )
}

function accumulate(target: DrilldownMetric, row: BreakdownRow): void {
  target.tokens.tokInput += row.tokens.tokInput
  target.tokens.tokOutput += row.tokens.tokOutput
  target.tokens.tokReasoning += row.tokens.tokReasoning
  target.tokens.tokCacheRead += row.tokens.tokCacheRead
  target.tokens.tokCacheWrite += row.tokens.tokCacheWrite
  target.tokens.totalInput += row.tokens.totalInput
  target.cost.actualSum += row.cost.actualSum
  target.cost.estimatedSum += row.cost.estimatedSum
  target.cost.unavailableCount += row.cost.unavailableCount
  target.messageCount += row.messageCount
  target.sessionRecordCount += row.sessionRecordCount
  target.rowCount += 1
}

export function sumMetrics(rows: BreakdownRow[]): DrilldownMetric {
  const total = emptyMetric()
  for (const row of rows) accumulate(total, row)
  return total
}

/** Descending token weight, ties broken by identity so ordering is deterministic. */
function byWeightThenId<T>(items: T[], weight: (item: T) => number, id: (item: T) => string): T[] {
  return [...items].sort((left, right) => {
    const delta = weight(right) - weight(left)
    return delta !== 0 ? delta : id(left).localeCompare(id(right))
  })
}

export function groupBySource(rows: BreakdownRow[]): SourceNode[] {
  const buckets = new Map<string, { metric: DrilldownMetric; agentKeys: Set<string> }>()
  for (const row of rows) {
    let bucket = buckets.get(row.source)
    if (bucket === undefined) {
      bucket = { metric: emptyMetric(), agentKeys: new Set() }
      buckets.set(row.source, bucket)
    }
    accumulate(bucket.metric, row)
    bucket.agentKeys.add(row.agentKey)
  }
  const nodes = [...buckets].map(([source, bucket]) => ({
    source,
    metric: bucket.metric,
    agentKeyCount: bucket.agentKeys.size,
  }))
  return byWeightThenId(
    nodes,
    (node) => tokenTotal(node.metric.tokens),
    (node) => node.source,
  )
}

export function groupByAgentKey(rows: BreakdownRow[]): AgentNode[] {
  const buckets = new Map<
    string,
    { agentRaw: string; metric: DrilldownMetric; models: Set<string> }
  >()
  for (const row of rows) {
    let bucket = buckets.get(row.agentKey)
    if (bucket === undefined) {
      bucket = { agentRaw: row.agentRaw, metric: emptyMetric(), models: new Set() }
      buckets.set(row.agentKey, bucket)
    }
    bucket.agentRaw = row.agentRaw
    accumulate(bucket.metric, row)
    bucket.models.add(modelKey(row.providerId, row.modelId))
  }
  const nodes = [...buckets].map(([agentKey, bucket]) => ({
    agentKey,
    agentRaw: bucket.agentRaw,
    metric: bucket.metric,
    modelCount: bucket.models.size,
  }))
  return byWeightThenId(
    nodes,
    (node) => tokenTotal(node.metric.tokens),
    (node) => node.agentKey,
  )
}

/** `\u0000` cannot appear in a provider or model id, so this key is collision-free. */
export function modelKey(providerId: string, modelId: string): string {
  return `${providerId}\u0000${modelId}`
}

export function groupByModel(rows: BreakdownRow[]): ModelNode[] {
  const buckets = new Map<
    string,
    {
      providerId: string
      modelId: string
      metric: DrilldownMetric
      variants: Map<string, VariantNode>
    }
  >()
  for (const row of rows) {
    const key = modelKey(row.providerId, row.modelId)
    let bucket = buckets.get(key)
    if (bucket === undefined) {
      bucket = {
        providerId: row.providerId,
        modelId: row.modelId,
        metric: emptyMetric(),
        variants: new Map(),
      }
      buckets.set(key, bucket)
    }
    accumulate(bucket.metric, row)
    const variantId = row.variant ?? ''
    let variant = bucket.variants.get(variantId)
    if (variant === undefined) {
      variant = { variant: row.variant, metric: emptyMetric() }
      bucket.variants.set(variantId, variant)
    }
    accumulate(variant.metric, row)
  }
  const nodes = [...buckets].map(([key, bucket]) => ({
    key,
    providerId: bucket.providerId,
    modelId: bucket.modelId,
    metric: bucket.metric,
    variants: byWeightThenId(
      [...bucket.variants.values()],
      (node) => tokenTotal(node.metric.tokens),
      (node) => node.variant ?? '',
    ),
  }))
  return byWeightThenId(
    nodes,
    (node) => tokenTotal(node.metric.tokens),
    (node) => node.key,
  )
}

/** Fraction in `[0, 1]`; a zero-token level yields 0 rather than `NaN`. */
export function shareOf(metric: DrilldownMetric, levelTotalTokens: number): number {
  if (levelTotalTokens <= 0) return 0
  return tokenTotal(metric.tokens) / levelTotalTokens
}
