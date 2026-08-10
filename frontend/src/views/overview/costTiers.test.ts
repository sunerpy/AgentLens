/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * The numbers here are the user's real ones, verbatim from the third report of
 * 「实际 $83.5228 / 估算 $312,235.4418 为什么差那么多」. They are the point of the fixture: the
 * assertions below are what prove the answer is "coverage, not method" — the unit prices come out
 * one order of magnitude apart at most while the amounts differ 3,700×.
 */
import { describe, expect, it } from 'vitest'

import type { CostCoverage, CostTotals } from '@/generated'

import { costTiers } from './costTiers'
import { formatMoney, formatShare } from './format'

const REPORTED_COST: CostTotals = {
  actualSum: 83.5228,
  estimatedSum: 312_235.4418,
  unavailableCount: 0,
}

const REPORTED_COVERAGE: CostCoverage = {
  actual: { recordCount: 117, billableTokens: 20_278_199 },
  estimated: { recordCount: 287_747, billableTokens: 69_900_000_000 },
  unavailable: { recordCount: 0, billableTokens: 0 },
}

function coverage(
  actual: CostCoverage['actual'],
  estimated: CostCoverage['estimated'],
  unavailable: CostCoverage['unavailable'] = { recordCount: 0, billableTokens: 0 },
): CostCoverage {
  return { actual, estimated, unavailable }
}

describe('costTiers 覆盖占比与单价', () => {
  it('把用户报的 3700 倍金额差解释成覆盖量差，而不是算法差', () => {
    const tiers = costTiers(REPORTED_COST, REPORTED_COVERAGE)

    // 金额差 ~3738 倍。
    expect(REPORTED_COST.estimatedSum / REPORTED_COST.actualSum).toBeGreaterThan(3_000)
    // 单价只差 8%：两种算法本身一致，差的全是覆盖量。
    expect(formatMoney(tiers.actual.unitPricePerMillion ?? 0)).toBe('$4.1188')
    expect(formatMoney(tiers.estimated.unitPricePerMillion ?? 0)).toBe('$4.4669')
    expect(tiers.comparability).toBe('incomparable')
  })

  /**
   * 这两个百分比就是「无需读者做除法」的那一步：0.03% 对 99.97% 已经把 3700 倍的来源说完了。
   */
  it('覆盖占比按可计费 Token 计', () => {
    const tiers = costTiers(REPORTED_COST, REPORTED_COVERAGE)

    expect(formatShare(tiers.actual.tokenShare ?? 0)).toBe('0.03%')
    expect(formatShare(tiers.estimated.tokenShare ?? 0)).toBe('99.97%')
  })

  it('可计费 Token 合计是三层之和，金额从不相加', () => {
    const tiers = costTiers(REPORTED_COST, {
      ...REPORTED_COVERAGE,
      unavailable: { recordCount: 4, billableTokens: 1_000 },
    })

    expect(tiers.totalBillableTokens).toBe(20_278_199 + 69_900_000_000 + 1_000)
    expect(tiers.totalRecordCount).toBe(117 + 287_747 + 4)
    // 无可信成本没有金额，所以也不可能有单价。
    expect(tiers.unavailable.unitPricePerMillion).toBeNull()
  })

  it('可计费 Token 为 0 的层没有单价，也没有占比', () => {
    const tiers = costTiers(
      { actualSum: 5, estimatedSum: 0, unavailableCount: 0 },
      coverage({ recordCount: 3, billableTokens: 0 }, { recordCount: 0, billableTokens: 0 }),
    )

    expect(tiers.actual.unitPricePerMillion).toBeNull()
    expect(tiers.actual.tokenShare).toBeNull()
    expect(tiers.estimated.tokenShare).toBeNull()
  })

  it('只有实际层有记录时判为 actualOnly', () => {
    const tiers = costTiers(
      { actualSum: 12, estimatedSum: 0, unavailableCount: 0 },
      coverage(
        { recordCount: 9, billableTokens: 1_000_000 },
        { recordCount: 0, billableTokens: 0 },
      ),
    )

    expect(tiers.comparability).toBe('actualOnly')
    expect(formatShare(tiers.actual.tokenShare ?? 0)).toBe('100.00%')
    expect(tiers.estimated.unitPricePerMillion).toBeNull()
  })

  it('只有估算层有记录时判为 estimatedOnly', () => {
    const tiers = costTiers(
      { actualSum: 0, estimatedSum: 8, unavailableCount: 0 },
      coverage(
        { recordCount: 0, billableTokens: 0 },
        { recordCount: 4, billableTokens: 2_000_000 },
      ),
    )

    expect(tiers.comparability).toBe('estimatedOnly')
    expect(formatMoney(tiers.estimated.unitPricePerMillion ?? 0)).toBe('$4.0000')
    expect(tiers.actual.unitPricePerMillion).toBeNull()
  })

  it('两层都没有记录时判为 empty', () => {
    const tiers = costTiers(
      { actualSum: 0, estimatedSum: 0, unavailableCount: 0 },
      coverage({ recordCount: 0, billableTokens: 0 }, { recordCount: 0, billableTokens: 0 }),
    )

    expect(tiers.comparability).toBe('empty')
  })

  /** 无可信成本有记录不影响可比性判断：那一层本来就没有金额可比。 */
  it('只有无可信成本有记录时仍然判为 empty', () => {
    const tiers = costTiers(
      { actualSum: 0, estimatedSum: 0, unavailableCount: 6 },
      coverage(
        { recordCount: 0, billableTokens: 0 },
        { recordCount: 0, billableTokens: 0 },
        { recordCount: 6, billableTokens: 6_000 },
      ),
    )

    expect(tiers.comparability).toBe('empty')
    expect(formatShare(tiers.unavailable.tokenShare ?? 0)).toBe('100.00%')
  })
})

describe('formatShare 边界', () => {
  it('真正的 0 与真正的 1 照原样渲染，不加边界符号', () => {
    expect(formatShare(0)).toBe('0.00%')
    expect(formatShare(1)).toBe('100.00%')
  })

  it('可四舍五入表达的值不加边界符号', () => {
    expect(formatShare(0.5)).toBe('50.00%')
    expect(formatShare(0.0001)).toBe('0.01%')
    expect(formatShare(0.9999)).toBe('99.99%')
  })

  /**
   * 两位小数以下的非零占比必须写成下界。写 `0.00%` 会被读成「这一层什么都没覆盖」，
   * 而它覆盖的恰恰是唯一带真实金额的那些记录 —— 那是把可信数据说成不存在。
   */
  it('小到写不出的非零占比渲染为下界而不是 0.00%', () => {
    expect(formatShare(0.00009)).toBe('<0.01%')
    expect(formatShare(1 / 1_000_000)).toBe('<0.01%')
  })

  it('接近但不等于 1 的占比渲染为上界而不是 100.00%', () => {
    expect(formatShare(0.99991)).toBe('>99.99%')
    expect(formatShare(1 - 1 / 1_000_000)).toBe('>99.99%')
  })
})
