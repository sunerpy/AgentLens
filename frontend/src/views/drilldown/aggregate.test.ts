import { describe, expect, it } from 'vitest'

import type { BreakdownRow, CostTotals, TokenValues } from '@/generated'

import {
  emptyMetric,
  groupByAgentKey,
  groupByModel,
  groupBySource,
  modelKey,
  shareOf,
  sumMetrics,
  tokenTotal,
} from './aggregate'

/**
 * 下钻三级的纯分组数学。这里钉住三条契约决策：
 *
 * 1. **二级按 `agentKey` 分组，绝不按 `agentRaw`。** `agentKey` 是
 *    `agentlens_core::archive::normalize_agent_key` 产出的归一化连接键；同一个逻辑 agent
 *    可能带着不同的原始标签（`"Atlas - Plan Executor"` vs `"Atlas Plan Executor"`），
 *    按原始串分组会把一个 agent 拆成好几行。
 * 2. **三级按 `(providerId, modelId)` 分组，`variant` 作为可展开子项。** 价格是按模型定的，
 *    不是按推理强度变体定的。
 * 3. **成本三分。** `actualSum` / `estimatedSum` / `unavailableCount` 从不互相合并，
 *    `unavailableCount` 永不变成金额。
 */

function tokens(overrides: Partial<TokenValues> = {}): TokenValues {
  return {
    tokInput: 0,
    tokOutput: 0,
    tokReasoning: 0,
    tokCacheRead: 0,
    tokCacheWrite: 0,
    totalInput: 0,
    ...overrides,
  }
}

function cost(overrides: Partial<CostTotals> = {}): CostTotals {
  return { actualSum: 0, estimatedSum: 0, unavailableCount: 0, ...overrides }
}

function row(overrides: Partial<BreakdownRow> = {}): BreakdownRow {
  return {
    source: 'local',
    agentKey: 'atlas-plan-executor',
    agentRaw: 'Atlas Plan Executor',
    providerId: 'anthropic',
    modelId: 'claude-opus',
    variant: null,
    tokens: tokens(),
    cost: cost(),
    messageCount: 0,
    activeSessionCount: 0,
    ...overrides,
  }
}

describe('aggregate/tokenTotal 与 emptyMetric', () => {
  it('总量是五个原子桶之和，不用派生的 totalInput', () => {
    const value = tokens({
      tokInput: 100,
      tokOutput: 20,
      tokReasoning: 3,
      tokCacheRead: 400,
      tokCacheWrite: 50,
      totalInput: 550,
    })
    expect(tokenTotal(value)).toBe(573)
    // totalInput 漏掉 output 与 reasoning，用它当分母会低估生成密集的行。
    expect(tokenTotal(value)).not.toBe(value.totalInput)
  })

  it('emptyMetric 每次返回全新对象（累加时不会互相污染）', () => {
    const first = emptyMetric()
    const second = emptyMetric()
    expect(first).toEqual(second)
    expect(first.tokens).not.toBe(second.tokens)
    first.tokens.tokInput = 999
    expect(second.tokens.tokInput).toBe(0)
  })
})

describe('aggregate/sumMetrics', () => {
  it('逐桶累加并记录折叠的行数', () => {
    const total = sumMetrics([
      row({ tokens: tokens({ tokInput: 10, totalInput: 10 }), messageCount: 1 }),
      row({ tokens: tokens({ tokOutput: 5 }), messageCount: 2 }),
    ])
    expect(total.tokens.tokInput).toBe(10)
    expect(total.tokens.tokOutput).toBe(5)
    expect(total.tokens.totalInput).toBe(10)
    expect(total.messageCount).toBe(3)
    expect(total.rowCount).toBe(2)
  })

  it('成本三路各自累加，永不合并', () => {
    const total = sumMetrics([
      row({ cost: cost({ actualSum: 0.01, unavailableCount: 1 }) }),
      row({ cost: cost({ estimatedSum: 0.02, unavailableCount: 2 }) }),
    ])
    expect(total.cost.actualSum).toBe(0.01)
    expect(total.cost.estimatedSum).toBe(0.02)
    expect(total.cost.unavailableCount).toBe(3)
    // 关键：三个数字互不串台，也不出现 0.03 这种"合计"。
    expect(total.cost.actualSum).not.toBe(0.03)
  })

  it('空输入得零指标而不是抛错', () => {
    expect(sumMetrics([])).toEqual(emptyMetric())
  })
})

describe('aggregate/groupBySource', () => {
  it('按来源折叠，并统计各来源下的 agentKey 去重数', () => {
    const nodes = groupBySource([
      row({ source: 'local', agentKey: 'a', tokens: tokens({ tokInput: 30 }) }),
      row({ source: 'local', agentKey: 'b', tokens: tokens({ tokInput: 20 }) }),
      row({ source: 'local', agentKey: 'a', tokens: tokens({ tokInput: 10 }) }),
      row({ source: 'remote', agentKey: 'c', tokens: tokens({ tokInput: 5 }) }),
    ])
    expect(nodes.map((node) => node.source)).toEqual(['local', 'remote'])
    expect(nodes[0].metric.tokens.tokInput).toBe(60)
    expect(nodes[0].agentKeyCount).toBe(2)
    expect(nodes[0].metric.rowCount).toBe(3)
    expect(nodes[1].agentKeyCount).toBe(1)
  })

  it('按 token 权重降序排列', () => {
    const nodes = groupBySource([
      row({ source: 'small', tokens: tokens({ tokInput: 1 }) }),
      row({ source: 'big', tokens: tokens({ tokInput: 100 }) }),
    ])
    expect(nodes.map((node) => node.source)).toEqual(['big', 'small'])
  })

  it('权重相同时按标识符稳定排序（结果可复现）', () => {
    const nodes = groupBySource([
      row({ source: 'zeta', tokens: tokens({ tokInput: 5 }) }),
      row({ source: 'alpha', tokens: tokens({ tokInput: 5 }) }),
      row({ source: 'mid', tokens: tokens({ tokInput: 5 }) }),
    ])
    expect(nodes.map((node) => node.source)).toEqual(['alpha', 'mid', 'zeta'])
  })

  it('不改写入参数组（排序走副本）', () => {
    const rows = [
      row({ source: 'small', tokens: tokens({ tokInput: 1 }) }),
      row({ source: 'big', tokens: tokens({ tokInput: 100 }) }),
    ]
    const snapshot = rows.map((item) => item.source)
    groupBySource(rows)
    expect(rows.map((item) => item.source)).toEqual(snapshot)
  })
})

describe('aggregate/groupByAgentKey', () => {
  it('同一 agentKey 的不同原始标签折叠成一行（不按 agentRaw 拆分）', () => {
    const nodes = groupByAgentKey([
      row({
        agentKey: 'atlas-plan-executor',
        agentRaw: 'Atlas - Plan Executor',
        tokens: tokens({ tokInput: 10 }),
      }),
      row({
        agentKey: 'atlas-plan-executor',
        agentRaw: 'Atlas Plan Executor',
        tokens: tokens({ tokInput: 20 }),
      }),
    ])
    expect(nodes).toHaveLength(1)
    expect(nodes[0].metric.tokens.tokInput).toBe(30)
    // 展示标签取后端顺序里最后见到的那个，这是 BreakdownRow 能提供的最接近"最新"的语义。
    expect(nodes[0].agentRaw).toBe('Atlas Plan Executor')
  })

  it('不同 agentKey 保持独立行', () => {
    const nodes = groupByAgentKey([
      row({ agentKey: 'alpha', tokens: tokens({ tokInput: 10 }) }),
      row({ agentKey: 'beta', tokens: tokens({ tokInput: 20 }) }),
    ])
    expect(nodes.map((node) => node.agentKey)).toEqual(['beta', 'alpha'])
  })

  it('统计各 agent 下的模型去重数（按 provider+model 组合计）', () => {
    const nodes = groupByAgentKey([
      row({ agentKey: 'a', providerId: 'p1', modelId: 'm1' }),
      row({ agentKey: 'a', providerId: 'p1', modelId: 'm2' }),
      row({ agentKey: 'a', providerId: 'p2', modelId: 'm1' }),
      row({ agentKey: 'a', providerId: 'p1', modelId: 'm1' }),
    ])
    expect(nodes[0].modelCount).toBe(3)
  })
})

describe('aggregate/modelKey', () => {
  it('用 NUL 分隔，provider 与 model 的拼接不可能撞车', () => {
    expect(modelKey('anthropic', 'claude-opus')).toBe('anthropic\u0000claude-opus')
  })

  it('易混拼接不会产生相同 key', () => {
    // 若用 '-' 之类的分隔符，('a','b-c') 与 ('a-b','c') 会撞成同一个 key。
    expect(modelKey('a', 'b-c')).not.toBe(modelKey('a-b', 'c'))
  })
})

describe('aggregate/groupByModel', () => {
  it('按 (providerId, modelId) 分组，variant 作为子项保留', () => {
    const nodes = groupByModel([
      row({ providerId: 'p', modelId: 'm', variant: 'high', tokens: tokens({ tokInput: 30 }) }),
      row({ providerId: 'p', modelId: 'm', variant: 'low', tokens: tokens({ tokInput: 10 }) }),
    ])
    expect(nodes).toHaveLength(1)
    expect(nodes[0].providerId).toBe('p')
    expect(nodes[0].modelId).toBe('m')
    expect(nodes[0].metric.tokens.tokInput).toBe(40)
    expect(nodes[0].variants.map((variant) => variant.variant)).toEqual(['high', 'low'])
    expect(nodes[0].variants[0].metric.tokens.tokInput).toBe(30)
  })

  it('同 modelId 不同 providerId 是两行（价格按 provider 定）', () => {
    const nodes = groupByModel([
      row({ providerId: 'p1', modelId: 'm', tokens: tokens({ tokInput: 10 }) }),
      row({ providerId: 'p2', modelId: 'm', tokens: tokens({ tokInput: 20 }) }),
    ])
    expect(nodes).toHaveLength(2)
    expect(nodes.map((node) => node.providerId)).toEqual(['p2', 'p1'])
  })

  it('variant 为 null 的行归入一个独立子项，且 null 被保留而非变成空串', () => {
    const nodes = groupByModel([
      row({ variant: null, tokens: tokens({ tokInput: 10 }) }),
      row({ variant: null, tokens: tokens({ tokInput: 5 }) }),
      row({ variant: 'high', tokens: tokens({ tokInput: 20 }) }),
    ])
    expect(nodes[0].variants).toHaveLength(2)
    const nullVariant = nodes[0].variants.find((variant) => variant.variant === null)
    expect(nullVariant?.metric.tokens.tokInput).toBe(15)
    expect(nullVariant?.metric.rowCount).toBe(2)
  })

  it('子项合计等于父项合计（不重不漏）', () => {
    const nodes = groupByModel([
      row({ variant: 'high', tokens: tokens({ tokInput: 30 }), messageCount: 3 }),
      row({ variant: 'low', tokens: tokens({ tokInput: 10 }), messageCount: 1 }),
      row({ variant: null, tokens: tokens({ tokInput: 7 }), messageCount: 2 }),
    ])
    const parent = nodes[0]
    const childSum = parent.variants.reduce(
      (accumulator, variant) => accumulator + tokenTotal(variant.metric.tokens),
      0,
    )
    expect(childSum).toBe(tokenTotal(parent.metric.tokens))
    const childMessages = parent.variants.reduce(
      (accumulator, variant) => accumulator + variant.metric.messageCount,
      0,
    )
    expect(childMessages).toBe(parent.metric.messageCount)
  })
})

describe('aggregate/shareOf', () => {
  it('返回 [0, 1] 区间的占比', () => {
    const metric = sumMetrics([row({ tokens: tokens({ tokInput: 25 }) })])
    expect(shareOf(metric, 100)).toBe(0.25)
    expect(shareOf(metric, 25)).toBe(1)
  })

  it('分母为 0 或负数时返回 0 而不是 NaN / Infinity', () => {
    const metric = sumMetrics([row({ tokens: tokens({ tokInput: 25 }) })])
    expect(shareOf(metric, 0)).toBe(0)
    expect(shareOf(metric, -10)).toBe(0)
    expect(Number.isNaN(shareOf(emptyMetric(), 0))).toBe(false)
  })

  it('零分子在正分母下是 0', () => {
    expect(shareOf(emptyMetric(), 100)).toBe(0)
  })
})
