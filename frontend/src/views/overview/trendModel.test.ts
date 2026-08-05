import { describe, expect, it } from 'vitest'

import type { CostTotals, CoverageStatus, SeriesPoint, TokenValues } from '@/generated'

import {
  TREND_METRICS,
  cacheTokens,
  hasAnyCoverage,
  rowValue,
  seriesKeysFor,
  toTrendRows,
  totalTokens,
  unavailableCount,
  valueAxisMax,
} from './trendModel'

/**
 * **coverage 三态**是本仓库最重要的语义不变量，计划为它花了三轮评审：
 *
 * | coverage  | 含义 | 绘图值 |
 * | --- | --- | --- |
 * | `none`    | 归档在这一桶里没有数据（真空洞）| `null` → 折线断开 |
 * | `full`    | 有数据且用量为 0（真空闲）| `0` → 折线贴底 |
 * | `partial` | 部分覆盖 | 按实际值绘制 + 覆盖带 |
 *
 * 把"空洞"和"零"混为一谈是这里唯一必须防住的回归：折线在没有数据的区间贴底，会让人
 * 以为那几天真的没在用，而不是那几天还没采集到。
 *
 * 第二条不变量：成本三分。`actualSum` 与 `estimatedSum` 是两条独立序列，永不相加；
 * `unavailableCount` 是计数，永不变成金额——连 0 元都不算。
 */

const TOKENS_ZERO: TokenValues = {
  tokInput: 0,
  tokOutput: 0,
  tokReasoning: 0,
  tokCacheRead: 0,
  tokCacheWrite: 0,
  totalInput: 0,
}

const TOKENS_SAMPLE: TokenValues = {
  tokInput: 1000,
  tokOutput: 200,
  tokReasoning: 30,
  tokCacheRead: 4000,
  tokCacheWrite: 500,
  totalInput: 5500,
}

const COST_ZERO: CostTotals = { actualSum: 0, estimatedSum: 0, unavailableCount: 0 }

const COST_SAMPLE: CostTotals = { actualSum: 0.0484, estimatedSum: 0.0075, unavailableCount: 3 }

function point(
  label: string,
  coverage: CoverageStatus,
  tokens: TokenValues | null,
  cost: CostTotals | null,
  messageCount: number | null,
): SeriesPoint {
  return {
    bucket: { startUtcMs: 1_700_000_000_000, endUtcMs: 1_700_086_400_000, label },
    coverage,
    tokens,
    cost,
    messageCount,
  }
}

describe('trendModel/coverage 三态：空洞 ≠ 零', () => {
  it("coverage 'none' 的每条绘图值都是 null（折线在这里必须断开）", () => {
    const [row] = toTrendRows([point('01-03', 'none', null, null, null)])
    expect(row.tokensValue).toBeNull()
    expect(row.actualValue).toBeNull()
    expect(row.estimatedValue).toBeNull()
    // 关键：不是 0。断言 not.toBe(0) 把"退化成零"这个具体回归钉死。
    expect(row.tokensValue).not.toBe(0)
    expect(row.actualValue).not.toBe(0)
    expect(row.estimatedValue).not.toBe(0)
  })

  it("coverage 'full' + 零用量得到字面量 0（真实空闲，折线贴底而非断开）", () => {
    const [row] = toTrendRows([point('01-04', 'full', TOKENS_ZERO, COST_ZERO, 0)])
    expect(row.tokensValue).toBe(0)
    expect(row.actualValue).toBe(0)
    expect(row.estimatedValue).toBe(0)
    // 关键：不是 null。这一侧的退化会把真实空闲画成缺数据。
    expect(row.tokensValue).not.toBeNull()
    expect(row.actualValue).not.toBeNull()
    expect(row.estimatedValue).not.toBeNull()
  })

  it("coverage 'partial' 照实绘制，并额外画覆盖带", () => {
    const [row] = toTrendRows([point('01-05', 'partial', TOKENS_SAMPLE, COST_SAMPLE, 42)])
    expect(row.tokensValue).toBe(5730)
    expect(row.actualValue).toBe(0.0484)
    expect(row.estimatedValue).toBe(0.0075)
    expect(row.coverageBand).toBe(1)
  })

  it("覆盖带只在非 'full' 桶出现（full 不画带、none 与 partial 都画）", () => {
    const rows = toTrendRows([
      point('full', 'full', TOKENS_SAMPLE, COST_SAMPLE, 1),
      point('partial', 'partial', TOKENS_SAMPLE, COST_SAMPLE, 1),
      point('none', 'none', null, null, null),
    ])
    expect(rows.map((row) => row.coverageBand)).toEqual([null, 1, 1])
  })

  it("即使 'none' 桶意外携带了非空 payload，绘图值仍是 null——coverage 说了算", () => {
    // 防御性断言：后端理论上不会这么发，但一旦发了，"空洞"语义不能被 payload 反转。
    const [row] = toTrendRows([point('01-06', 'none', TOKENS_SAMPLE, COST_SAMPLE, 99)])
    expect(row.tokensValue).toBeNull()
    expect(row.actualValue).toBeNull()
    expect(row.estimatedValue).toBeNull()
    // 原始 payload 仍原样保留，供 tooltip 使用。
    expect(row.tokens).toBe(TOKENS_SAMPLE)
    expect(row.messageCount).toBe(99)
  })

  it("'full' 桶但 payload 为 null 时仍是 null，不会凭空造 0", () => {
    const [row] = toTrendRows([point('01-07', 'full', null, null, null)])
    expect(row.tokensValue).toBeNull()
    expect(row.actualValue).toBeNull()
    expect(row.estimatedValue).toBeNull()
  })

  it('三态混合的序列逐桶独立判定，互不污染', () => {
    const rows = toTrendRows([
      point('01-01', 'full', TOKENS_SAMPLE, COST_SAMPLE, 10),
      point('01-02', 'none', null, null, null),
      point('01-03', 'full', TOKENS_ZERO, COST_ZERO, 0),
      point('01-04', 'partial', TOKENS_SAMPLE, COST_SAMPLE, 5),
    ])
    expect(rows.map((row) => row.tokensValue)).toEqual([5730, null, 0, 5730])
    expect(rows.map((row) => row.coverage)).toEqual(['full', 'none', 'full', 'partial'])
  })
})

describe('trendModel/toTrendRows 透传', () => {
  it('label 与 startUtcMs 原样搬运，行序与入参一致', () => {
    const rows = toTrendRows([
      point('01-01', 'full', TOKENS_SAMPLE, COST_SAMPLE, 1),
      point('01-02', 'full', TOKENS_SAMPLE, COST_SAMPLE, 2),
    ])
    expect(rows.map((row) => row.label)).toEqual(['01-01', '01-02'])
    expect(rows[0].startUtcMs).toBe(1_700_000_000_000)
  })

  it('空序列产出空数组', () => {
    expect(toTrendRows([])).toEqual([])
  })
})

describe('trendModel/totalTokens 与 cacheTokens', () => {
  it('总量是五个原子桶之和，不用派生的 totalInput', () => {
    // totalInput = input + cacheRead + cacheWrite = 5500，漏掉 output 与 reasoning；
    // 用它当总量会让生成密集的桶看起来很小。
    expect(totalTokens(TOKENS_SAMPLE)).toBe(5730)
    expect(totalTokens(TOKENS_SAMPLE)).not.toBe(TOKENS_SAMPLE.totalInput)
  })

  it('全零输入得 0', () => {
    expect(totalTokens(TOKENS_ZERO)).toBe(0)
  })

  it('缓存展示量是读 + 写', () => {
    expect(cacheTokens(TOKENS_SAMPLE)).toBe(4500)
    expect(cacheTokens(TOKENS_ZERO)).toBe(0)
  })
})

describe('trendModel/seriesKeysFor 与 rowValue', () => {
  it('tokens 指标一条序列，cost 指标两条独立序列', () => {
    expect(seriesKeysFor('tokens')).toEqual(['tokens'])
    expect(seriesKeysFor('cost')).toEqual(['actual', 'estimated'])
  })

  it('指标枚举与 UI 切换器一致', () => {
    expect(TREND_METRICS).toEqual(['tokens', 'cost'])
  })

  it('实际成本与估算成本永不相加——两条 key 各取各的字段', () => {
    const [row] = toTrendRows([point('01-01', 'full', TOKENS_SAMPLE, COST_SAMPLE, 1)])
    expect(rowValue(row, 'actual')).toBe(0.0484)
    expect(rowValue(row, 'estimated')).toBe(0.0075)
    // 若哪天有人把两者相加，这个断言会直接指出来。
    expect(rowValue(row, 'actual')).not.toBe(0.0484 + 0.0075)
  })

  it('rowValue 在空洞桶上对三条 key 都返回 null', () => {
    const [row] = toTrendRows([point('01-02', 'none', null, null, null)])
    expect(rowValue(row, 'tokens')).toBeNull()
    expect(rowValue(row, 'actual')).toBeNull()
    expect(rowValue(row, 'estimated')).toBeNull()
  })
})

describe('trendModel/valueAxisMax', () => {
  it('取指标下所有序列的最大值', () => {
    const rows = toTrendRows([
      point('01-01', 'full', TOKENS_SAMPLE, COST_SAMPLE, 1),
      point('01-02', 'full', TOKENS_ZERO, COST_ZERO, 0),
    ])
    expect(valueAxisMax(rows, 'tokens')).toBe(5730)
    expect(valueAxisMax(rows, 'cost')).toBe(0.0484)
  })

  it('全空洞时回落 1，避免 recharts 解不出空域产生 NaN 几何', () => {
    const rows = toTrendRows([
      point('01-01', 'none', null, null, null),
      point('01-02', 'none', null, null, null),
    ])
    expect(valueAxisMax(rows, 'tokens')).toBe(1)
    expect(valueAxisMax(rows, 'cost')).toBe(1)
  })

  it('真实全零区间同样回落 1（有数据但都是 0）', () => {
    const rows = toTrendRows([point('01-01', 'full', TOKENS_ZERO, COST_ZERO, 0)])
    expect(valueAxisMax(rows, 'tokens')).toBe(1)
  })

  it('空行集回落 1', () => {
    expect(valueAxisMax([], 'tokens')).toBe(1)
  })
})

describe('trendModel/hasAnyCoverage', () => {
  it('只要有一个非空洞桶就算有覆盖', () => {
    expect(
      hasAnyCoverage(
        toTrendRows([
          point('01-01', 'none', null, null, null),
          point('01-02', 'partial', TOKENS_SAMPLE, COST_SAMPLE, 1),
        ]),
      ),
    ).toBe(true)
  })

  it('全空洞与空行集都算无覆盖（界面据此显示空态而不是一条贴底的线）', () => {
    expect(hasAnyCoverage(toTrendRows([point('01-01', 'none', null, null, null)]))).toBe(false)
    expect(hasAnyCoverage([])).toBe(false)
  })
})

describe('trendModel/unavailableCount', () => {
  it('计数原样取出，不参与任何金额运算', () => {
    const [row] = toTrendRows([point('01-01', 'full', TOKENS_SAMPLE, COST_SAMPLE, 1)])
    expect(unavailableCount(row)).toBe(3)
  })

  it('cost 为 null 时计数是 0（这是"没有不可用条目"，不是金额 0）', () => {
    const [row] = toTrendRows([point('01-01', 'none', null, null, null)])
    expect(unavailableCount(row)).toBe(0)
    // 同一行的金额仍必须是 null，两者语义不同。
    expect(row.actualValue).toBeNull()
  })
})
