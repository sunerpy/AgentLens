import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import type {
  CostTotals,
  ObservedModelPrice,
  PriceEntry,
  SeriesGroup,
  SeriesPoint,
  Summary,
} from '@/generated'
import { zh } from '@/i18n/zh'

import { CostMissingPrices } from './CostMissingPrices'
import {
  MISSING_PRICE_PREVIEW,
  missingPriceEntries,
  rangeMissingPriceEntries,
  unattributedCount,
} from './costMissing'
import { SummaryCards } from './SummaryCards'

const PRICE: PriceEntry = {
  providerId: 'anthropic',
  modelId: 'claude-opus-4-8',
  inputPerMtok: 5,
  outputPerMtok: 25,
  cacheReadPerMtok: 0.5,
  cacheWritePerMtok: 6.25,
  extra: {},
}

function observed(
  providerId: string,
  modelId: string,
  usageCount: number,
  matchKind: ObservedModelPrice['matchKind'],
): ObservedModelPrice {
  return {
    providerId,
    modelId,
    usageCount,
    matchKind,
    matchedPrice: matchKind === 'unknown' ? null : PRICE,
  }
}

afterEach(cleanup)

describe('missingPriceEntries', () => {
  /** 只有 unknown 才是「没有价格」；另外三种都已经解析出了某个价格。 */
  it('只保留 unknown，其余三种匹配都算已有价格', () => {
    const entries = missingPriceEntries([
      observed('kiro-auth', 'opus-cross', 10, 'crossProvider'),
      observed('aws', 'sonnet-normalized', 20, 'normalized'),
      observed('aws', 'sonnet-family', 30, 'family'),
      observed('anthropic', 'claude-opus-4-8', 40, 'exact'),
      observed('private', 'secret-v7', 50, 'unknown'),
    ])

    expect(entries).toEqual([{ providerId: 'private', modelId: 'secret-v7', usageCount: 50 }])
  })

  it('按用量倒序，同用量按 provider 再按 model 排', () => {
    const entries = missingPriceEntries([
      observed('zeta', 'model-a', 5, 'unknown'),
      observed('alpha', 'model-b', 5, 'unknown'),
      observed('alpha', 'model-a', 5, 'unknown'),
      observed('mid', 'model-x', 99, 'unknown'),
    ])

    expect(entries.map((entry) => `${entry.providerId}/${entry.modelId}`)).toEqual([
      'mid/model-x',
      'alpha/model-a',
      'alpha/model-b',
      'zeta/model-a',
    ])
  })

  it('空输入得到空列表', () => {
    expect(missingPriceEntries([])).toEqual([])
  })
})

function manyMissing(count: number) {
  return Array.from({ length: count }, (_unused, index) => ({
    providerId: 'private',
    modelId: `model-${String(index).padStart(2, '0')}`,
    usageCount: 100 - index,
  }))
}

/**
 * 用户原话：「部分缺失 看不出来是什么没有写价格」。所以关键信息必须能点开看到，而不是只靠
 * hover —— 下面几条锁住「点开能看到具体条目」「截断数量正确」「没有缺失时整块不渲染」。
 */
describe('CostMissingPrices', () => {
  it('unavailableCount 为 0 时整块不渲染', () => {
    render(<CostMissingPrices entries={manyMissing(3)} unavailableCount={0} />)

    expect(screen.queryByTestId('cost-missing')).toBeNull()
    expect(screen.queryByTestId('cost-badge-partial')).toBeNull()
  })

  it('默认只显示徽章与展开入口，明细收起', () => {
    render(<CostMissingPrices entries={manyMissing(3)} unavailableCount={12} />)

    expect(screen.getByTestId('cost-badge-partial').textContent).toContain(zh.common.cost.partial)
    expect(screen.getByTestId('cost-missing-toggle').textContent).toBe(
      zh.overview.summary.missingShow,
    )
    expect(screen.queryByTestId('cost-missing-list')).toBeNull()
  })

  it('点开后逐条列出缺价的 provider / model 与记录数', () => {
    render(<CostMissingPrices entries={manyMissing(3)} unavailableCount={12} />)

    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    const list = screen.getByTestId('cost-missing-list')
    expect(list.children).toHaveLength(3)
    expect(screen.getByTestId('cost-missing-entry-private-model-00').textContent).toContain(
      'private / model-00',
    )
    expect(screen.getByTestId('cost-missing-count').textContent).toBe(
      zh.overview.summary.missingSummary(3, '12'),
    )
    expect(screen.getByTestId('cost-missing-toggle').textContent).toBe(
      zh.overview.summary.missingHide,
    )
  })

  it('再点一次收起', () => {
    render(<CostMissingPrices entries={manyMissing(3)} unavailableCount={12} />)

    fireEvent.click(screen.getByTestId('cost-missing-toggle'))
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.queryByTestId('cost-missing-list')).toBeNull()
  })

  it('条目超过预览数时截断，并给出展开全部的入口', () => {
    const total = MISSING_PRICE_PREVIEW + 4
    render(<CostMissingPrices entries={manyMissing(total)} unavailableCount={99} />)
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.getByTestId('cost-missing-list').children).toHaveLength(MISSING_PRICE_PREVIEW)
    expect(screen.getByTestId('cost-missing-expand').textContent).toBe(
      zh.overview.summary.missingExpand(4),
    )

    fireEvent.click(screen.getByTestId('cost-missing-expand'))

    expect(screen.getByTestId('cost-missing-list').children).toHaveLength(total)
    expect(screen.queryByTestId('cost-missing-expand')).toBeNull()

    fireEvent.click(screen.getByTestId('cost-missing-collapse'))
    expect(screen.getByTestId('cost-missing-list').children).toHaveLength(MISSING_PRICE_PREVIEW)
  })

  it('刚好等于预览数时不出现展开入口', () => {
    render(<CostMissingPrices entries={manyMissing(MISSING_PRICE_PREVIEW)} unavailableCount={7} />)
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.getByTestId('cost-missing-list').children).toHaveLength(MISSING_PRICE_PREVIEW)
    expect(screen.queryByTestId('cost-missing-expand')).toBeNull()
  })

  /** 有 unavailable 记录但拿不到模型身份时，必须说明原因而不是显示一个空列表。 */
  it('有缺失记录但没有模型身份时给出解释', () => {
    render(<CostMissingPrices entries={[]} unavailableCount={5} />)
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.queryByTestId('cost-missing-list')).toBeNull()
    expect(screen.getByTestId('cost-missing-empty').textContent).toBe(
      zh.overview.summary.missingNoIdentity,
    )
  })
})

function summary(unavailableCount: number): Summary {
  return {
    tokens: {
      tokInput: 10,
      tokOutput: 20,
      tokReasoning: 30,
      tokCacheRead: 40,
      tokCacheWrite: 50,
      totalInput: 100,
    },
    cost: { actualSum: 1.5, estimatedSum: 2.5, unavailableCount },
    costCoverage: {
      actual: { recordCount: 117, billableTokens: 20_278_199 },
      estimated: { recordCount: 251_365, billableTokens: 60_130_238_648 },
      unavailable: { recordCount: unavailableCount, billableTokens: unavailableCount * 1_000 },
    },
    messageCount: 6,
    sessionRecordCount: 0,
    activeSessionCount: 2,
  }
}

describe('SummaryCards 的成本卡片', () => {
  it('实际与估算金额分别显示各自覆盖的记录数和可计费 Token', () => {
    render(<SummaryCards summary={summary(7)} />)

    expect(screen.getByTestId('summary-cost-actual-coverage').textContent).toBe(
      zh.overview.summary.costCoverage('117', '20.3M'),
    )
    expect(screen.getByTestId('summary-cost-estimated-coverage').textContent).toBe(
      zh.overview.summary.costCoverage('251,365', '60.1B'),
    )
    expect(screen.getByTestId('summary-cost-unavailable-coverage').textContent).toBe(
      zh.overview.summary.costCoverage('7', '7K'),
    )
  })

  it('长金额保留完整精度并允许在狭窄卡片内安全换行', () => {
    const longCost = summary(0)
    longCost.cost.actualSum = 297_017.5844

    render(<SummaryCards summary={longCost} />)

    const actual = screen.getByTestId('summary-cost-actual')
    expect(actual.textContent).toBe('$297,017.5844')
    expect(actual.className).toContain('[overflow-wrap:anywhere]')
    expect(actual.className).not.toContain('truncate')
    expect(actual.className).not.toContain('overflow-hidden')
  })

  it('有缺价记录时把明细入口挂在成本卡片里', () => {
    render(<SummaryCards summary={summary(3)} missingPrices={manyMissing(2)} />)

    expect(screen.getByTestId('cost-missing')).toBeTruthy()
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))
    expect(screen.getByTestId('cost-missing-list').children).toHaveLength(2)
  })

  it('没有缺价记录时既无徽章也无明细', () => {
    render(<SummaryCards summary={summary(0)} missingPrices={[]} />)

    expect(screen.queryByTestId('cost-missing')).toBeNull()
    expect(screen.queryByTestId('cost-badge-partial')).toBeNull()
  })

  // 「这些记录不计入任何金额，也不当 0」是解释性文案：0 条时没有要解释的对象，
  // 却会被读成「有问题」。所以整段（标签、计数、说明）在 0 条时都不该出现。
  it('无可信成本为 0 条时整段文案都不渲染', () => {
    render(<SummaryCards summary={summary(0)} missingPrices={[]} />)

    expect(screen.queryByTestId('summary-cost-unavailable-block')).toBeNull()
    expect(screen.queryByTestId('summary-cost-unavailable')).toBeNull()
    expect(screen.queryByText(zh.overview.summary.costUnavailableHint)).toBeNull()
    expect(screen.queryByText(zh.overview.summary.costUnavailableLabel)).toBeNull()
  })

  it('无可信成本大于 0 条时才渲染计数与说明', () => {
    render(<SummaryCards summary={summary(7)} missingPrices={[]} />)

    expect(screen.getByTestId('summary-cost-unavailable-block')).toBeTruthy()
    expect(screen.getByTestId('summary-cost-unavailable').textContent).toBe('7')
    expect(screen.getByText(zh.overview.summary.costUnavailableHint)).toBeTruthy()
  })

  // 金额是真实合计，$0.00 与「没有金额」是两件事，因此它们不随计数门控。
  it('金额两格即使自身为 0 也照常渲染', () => {
    const zeroCost = summary(0)
    zeroCost.cost.actualSum = 0
    zeroCost.cost.estimatedSum = 0

    render(<SummaryCards summary={zeroCost} missingPrices={[]} />)

    expect(screen.getByTestId('summary-cost-actual').textContent).toBe('$0.0000')
    expect(screen.getByTestId('summary-cost-estimated').textContent).toBe('$0.0000')
    expect(screen.queryByTestId('summary-cost-unavailable-block')).toBeNull()
  })
})

/**
 * 用户第三次问「实际 $83.5228 和估算 $312,235.4418 为什么差那么多」。
 *
 * 这不是计算缺陷：单价是 $4.12/M 与 $4.47/M，同一量级；差额全来自覆盖量。上两轮补的覆盖量
 * 小字没救回来，因为并排等重的两个大金额本身就在邀请相减。所以这一组断言锁的是**呈现结构**：
 * 每层自带覆盖占比与单价，且不管三态怎么组合，都有一句写明这两个数不可比的说明。
 */
describe('成本卡的不可比呈现', () => {
  /** 用户报的那组真实数字。 */
  function reported(unavailableCount = 0): Summary {
    const base = summary(unavailableCount)
    base.cost.actualSum = 83.5228
    base.cost.estimatedSum = 312_235.4418
    base.costCoverage = {
      actual: { recordCount: 117, billableTokens: 20_278_199 },
      estimated: { recordCount: 287_747, billableTokens: 69_900_000_000 },
      unavailable: { recordCount: unavailableCount, billableTokens: unavailableCount * 1_000 },
    }
    return base
  }

  it('每层都带覆盖占比，读者不必自己做除法就能看出差距来自覆盖量', () => {
    render(<SummaryCards summary={reported()} />)

    expect(screen.getByTestId('summary-cost-actual-share').textContent).toBe(
      zh.overview.summary.costTierShare('0.03%'),
    )
    expect(screen.getByTestId('summary-cost-estimated-share').textContent).toBe(
      zh.overview.summary.costTierShare('99.97%'),
    )
  })

  it('单价是可比的那一个，两层单价同量级', () => {
    render(<SummaryCards summary={reported()} />)

    expect(screen.getByTestId('summary-cost-actual-unit-price').textContent).toBe('$4.1188')
    expect(screen.getByTestId('summary-cost-estimated-unit-price').textContent).toBe('$4.4669')
  })

  it('两层都有覆盖时明确写出「不要相减」并指向单价', () => {
    render(<SummaryCards summary={reported()} />)

    const note = screen.getByTestId('summary-cost-comparability')
    expect(note.getAttribute('data-comparability')).toBe('incomparable')
    expect(note.textContent).toContain(
      zh.overview.summary.costIncomparable('117', '287,747').slice(0, 20),
    )
    expect(note.textContent).toContain(zh.overview.summary.costUnitPriceHint)
  })

  // 两个金额竖排各占整行，而不是并排两格 —— 并排的对称本身就是「可以比较」的暗示。
  it('两层竖排成各自独立的块，而不是等重并排', () => {
    render(<SummaryCards summary={reported()} />)

    const actual = screen.getByTestId('summary-cost-tier-actual')
    const estimated = screen.getByTestId('summary-cost-tier-estimated')
    expect(actual.getAttribute('data-coverage-records')).toBe('117')
    expect(estimated.getAttribute('data-coverage-records')).toBe('287747')
    expect(actual.parentElement).toBe(estimated.parentElement)
    expect(actual.className).toContain('flex-col')
  })

  it('estimated 覆盖为 0 时说明 $0 是「没有记录」而不是「估算出来是 0」', () => {
    const onlyActual = reported()
    onlyActual.cost.estimatedSum = 0
    onlyActual.costCoverage.estimated = { recordCount: 0, billableTokens: 0 }

    render(<SummaryCards summary={onlyActual} />)

    const note = screen.getByTestId('summary-cost-comparability')
    expect(note.getAttribute('data-comparability')).toBe('actualOnly')
    expect(note.textContent).toContain(zh.overview.summary.costActualOnly)
    expect(screen.getByTestId('summary-cost-estimated').textContent).toBe('$0.0000')
    // 没有可计费 Token 的层没有单价可言；写 $0 会被读成「不要钱」。
    expect(screen.getByTestId('summary-cost-estimated-unit-price').textContent).toBe(
      zh.overview.summary.costUnitPriceUndefined,
    )
  })

  it('actual 覆盖为 0 时说明 $0 是「没有真实金额」而不是「实际花了 0」', () => {
    const onlyEstimated = reported()
    onlyEstimated.cost.actualSum = 0
    onlyEstimated.costCoverage.actual = { recordCount: 0, billableTokens: 0 }

    render(<SummaryCards summary={onlyEstimated} />)

    const note = screen.getByTestId('summary-cost-comparability')
    expect(note.getAttribute('data-comparability')).toBe('estimatedOnly')
    expect(note.textContent).toContain(zh.overview.summary.costEstimatedOnly)
    expect(screen.getByTestId('summary-cost-actual-unit-price').textContent).toBe(
      zh.overview.summary.costUnitPriceUndefined,
    )
    expect(screen.getByTestId('summary-cost-estimated-share').textContent).toBe(
      zh.overview.summary.costTierShare('100.00%'),
    )
  })

  it('两层都没有覆盖时说明两个 $0 都是「没有数据」', () => {
    const empty = reported()
    empty.cost.actualSum = 0
    empty.cost.estimatedSum = 0
    empty.costCoverage = {
      actual: { recordCount: 0, billableTokens: 0 },
      estimated: { recordCount: 0, billableTokens: 0 },
      unavailable: { recordCount: 0, billableTokens: 0 },
    }

    render(<SummaryCards summary={empty} />)

    const note = screen.getByTestId('summary-cost-comparability')
    expect(note.getAttribute('data-comparability')).toBe('empty')
    expect(note.textContent).toContain(zh.overview.summary.costNoCoverage)
    // 没有任何可计费 Token 时占比无定义，不能写 0.00%（那等于宣称「占了 0%」）。
    expect(screen.getByTestId('summary-cost-actual-share').textContent).toBe(
      zh.overview.summary.costTierShareUnknown,
    )
  })

  // 三态永不相加：卡片里不得出现 actual + estimated 的和。
  it('卡片里不出现两个金额的和', () => {
    render(<SummaryCards summary={reported(2)} />)

    const card = screen.getByTestId('summary-cost-card')
    const sum = 83.5228 + 312_235.4418
    expect(card.textContent).not.toContain(
      sum.toLocaleString('en-US', { minimumFractionDigits: 4, maximumFractionDigits: 4 }),
    )
    expect(card.textContent).toContain('$83.5228')
    expect(card.textContent).toContain('$312,235.4418')
  })

  it('无可信成本那层仍然不给单价，只报记录数与覆盖量', () => {
    render(<SummaryCards summary={reported(7)} />)

    expect(screen.getByTestId('summary-cost-unavailable').textContent).toBe('7')
    expect(screen.getByTestId('summary-cost-unavailable-coverage').textContent).toBe(
      zh.overview.summary.costCoverage('7', '7K'),
    )
    expect(screen.queryByTestId('summary-cost-unavailable-unit-price')).toBeNull()
  })
})

// ---------------------------------------------------------------------------
// 区间口径：清单必须是表头总数的分解
// ---------------------------------------------------------------------------

const NO_TOKENS = {
  tokInput: 0,
  tokOutput: 0,
  tokReasoning: 0,
  tokCacheRead: 0,
  tokCacheWrite: 0,
  totalInput: 0,
}

const BILLABLE_TOKENS = {
  ...NO_TOKENS,
  tokInput: 1,
  totalInput: 1,
}

function point(cost: CostTotals | null, tokens = BILLABLE_TOKENS): SeriesPoint {
  return {
    bucket: { startUtcMs: 0, endUtcMs: 1, label: '2026-01-01' },
    coverage: cost === null ? 'none' : 'full',
    tokens: cost === null ? null : tokens,
    cost,
    messageCount: cost === null ? null : 0,
    sessionRecordCount: cost === null ? null : 0,
  }
}

function totals(unavailableCount: number): CostTotals {
  return { actualSum: 0, estimatedSum: 0, unavailableCount }
}

function modelGroup(
  providerId: string,
  modelId: string,
  buckets: (CostTotals | null)[],
  tokens = BILLABLE_TOKENS,
): SeriesGroup {
  return {
    dimension: 'model',
    id: `${providerId}\u0000${modelId}`,
    label: `${providerId} / ${modelId}`,
    series: buckets.map((cost) => point(cost, tokens)),
  }
}

/**
 * 用户看到的实际内容是「本范围内 21,947 条」压在一份合计 50,923 条的清单上，于是以为算错了。
 * 根因是两个口径并排：表头按区间统计，清单来自不带时间条件的 `price_catalog_get`。
 *
 * 修法是让清单也走区间口径——趋势查询的 model 分组与 `get_summary` 同范围、同筛选、同
 * `is_incomplete` 排除，所以逐条相加必须**恰好**等于表头。下面第一条用例就是这个不变式。
 */
describe('rangeMissingPriceEntries', () => {
  it('按 model 分组跨桶累加 unavailableCount，且合计等于区间总数', () => {
    const groups = [
      modelGroup('kiro-auth', 'claude-opus-5-max', [totals(10), totals(22), totals(0)]),
      modelGroup('nwcdai', 'gemini-3-pro-preview', [totals(5), null, totals(3)]),
    ]

    const entries = rangeMissingPriceEntries(groups)

    expect(entries).toEqual([
      { providerId: 'kiro-auth', modelId: 'claude-opus-5-max', usageCount: 32 },
      { providerId: 'nwcdai', modelId: 'gemini-3-pro-preview', usageCount: 8 },
    ])
    // 表头总数 40 = 32 + 8：清单是它的分解，没有残差。
    expect(unattributedCount(entries, 40)).toBe(0)
  })

  it('有价格的模型合计为 0，不进缺价清单', () => {
    const entries = rangeMissingPriceEntries([
      modelGroup('openai', 'gpt-5-codex', [totals(0), totals(0)]),
      modelGroup('private', 'secret-v7', [totals(4)]),
    ])

    expect(entries.map((entry) => entry.modelId)).toEqual(['secret-v7'])
  })

  it('没有四桶可计费用量的模型不进缺价清单', () => {
    const reasoningOnly = { ...NO_TOKENS, tokReasoning: 99 }
    const entries = rangeMissingPriceEntries([
      modelGroup('kiro-auth', 'auto', [totals(1)], NO_TOKENS),
      modelGroup('private', 'reasoning-only', [totals(1)], reasoningOnly),
      modelGroup('private', 'billable', [totals(1)], BILLABLE_TOKENS),
    ])

    expect(entries).toEqual([{ providerId: 'private', modelId: 'billable', usageCount: 1 }])
  })

  /** `cost: null` 是「该窗口无数据覆盖」，不是 0；把它当 0 累加会凭空造出记录数。 */
  it('未覆盖的桶（cost 为 null）不贡献记录数', () => {
    expect(rangeMissingPriceEntries([modelGroup('a', 'b', [null, null])])).toEqual([])
    expect(rangeMissingPriceEntries([modelGroup('a', 'b', [null, totals(2), null])])).toEqual([
      { providerId: 'a', modelId: 'b', usageCount: 2 },
    ])
  })

  it('只吃 model 维度，其他分组维度一律忽略', () => {
    const groups: SeriesGroup[] = [
      { dimension: 'source', id: 'opencode', label: 'opencode', series: [point(totals(9))] },
      { dimension: 'agent', id: 'build', label: 'build', series: [point(totals(9))] },
      { dimension: 'provider', id: 'kiro-auth', label: 'kiro-auth', series: [point(totals(9))] },
      modelGroup('kiro-auth', 'claude-opus-5-max', [totals(9)]),
    ]

    expect(rangeMissingPriceEntries(groups)).toEqual([
      { providerId: 'kiro-auth', modelId: 'claude-opus-5-max', usageCount: 9 },
    ])
  })

  /**
   * 分组 id 是 Rust 的 `format!("{provider_id}\0{model_id}")`。model id 本身可能带斜杠
   * （`openai.gpt-5.6-sol`、`us.anthropic.claude-…`），所以必须按 NUL 而不是 `/` 切分。
   */
  it('按 NUL 切分，model id 里的点号与斜杠不被破坏', () => {
    const entries = rangeMissingPriceEntries([
      modelGroup('amazon-bedrock', 'openai.gpt-5.6-sol', [totals(120)]),
      modelGroup('google', 'antigravity/gemini-3', [totals(7)]),
    ])

    expect(entries).toEqual([
      { providerId: 'amazon-bedrock', modelId: 'openai.gpt-5.6-sol', usageCount: 120 },
      { providerId: 'google', modelId: 'antigravity/gemini-3', usageCount: 7 },
    ])
  })

  it('缺分隔符的畸形 id 被跳过而不是产出半个身份', () => {
    const groups: SeriesGroup[] = [
      { dimension: 'model', id: 'no-separator', label: 'x', series: [point(totals(3))] },
      modelGroup('ok', 'model', [totals(1)]),
    ]

    expect(rangeMissingPriceEntries(groups)).toEqual([
      { providerId: 'ok', modelId: 'model', usageCount: 1 },
    ])
  })

  it('按记录数倒序，同数按 provider 再按 model 排', () => {
    const entries = rangeMissingPriceEntries([
      modelGroup('zeta', 'model-a', [totals(5)]),
      modelGroup('alpha', 'model-b', [totals(5)]),
      modelGroup('alpha', 'model-a', [totals(5)]),
      modelGroup('mid', 'model-x', [totals(99)]),
    ])

    expect(entries.map((entry) => `${entry.providerId}/${entry.modelId}`)).toEqual([
      'mid/model-x',
      'alpha/model-a',
      'alpha/model-b',
      'zeta/model-a',
    ])
  })

  it('空输入得到空列表', () => {
    expect(rangeMissingPriceEntries([])).toEqual([])
  })
})

describe('unattributedCount', () => {
  it('分解完整时为 0', () => {
    expect(unattributedCount(manyMissing(0), 0)).toBe(0)
    expect(unattributedCount([{ providerId: 'a', modelId: 'b', usageCount: 7 }], 7)).toBe(0)
  })

  it('清单少于总数时报出差额', () => {
    expect(unattributedCount([{ providerId: 'a', modelId: 'b', usageCount: 5 }], 12)).toBe(7)
  })

  /** 夹到 0：清单多于总数时不显示负数残差，那只会再造一个看不懂的数。 */
  it('清单多于总数时夹到 0，不产出负数', () => {
    expect(unattributedCount([{ providerId: 'a', modelId: 'b', usageCount: 30 }], 12)).toBe(0)
  })
})

describe('CostMissingPrices 的口径分区', () => {
  /** 表头说的数与清单相加一致时，不能再冒出第三个数干扰对账。 */
  it('分解完整时不显示残差行，只显示区间口径说明', () => {
    render(
      <CostMissingPrices
        entries={[{ providerId: 'kiro-auth', modelId: 'claude-opus-5-max', usageCount: 12 }]}
        unavailableCount={12}
      />,
    )
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.queryByTestId('cost-missing-unattributed')).toBeNull()
    expect(screen.getByTestId('cost-missing-count').textContent).toBe(
      zh.overview.summary.missingSummary(1, '12'),
    )
    expect(screen.getByTestId('cost-missing').textContent).toContain(
      zh.overview.summary.missingRangeScopeHint,
    )
  })

  it('清单不足以解释总数时把差额显式说出来', () => {
    render(
      <CostMissingPrices
        entries={[{ providerId: 'a', modelId: 'b', usageCount: 5 }]}
        unavailableCount={12}
      />,
    )
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.getByTestId('cost-missing-unattributed').textContent).toBe(
      zh.overview.summary.missingUnattributed('7'),
    )
  })

  /**
   * 这是本轮的核心回归：全库清单与区间记录数**不得并排**。有区间清单时全库分区必须完全不渲染，
   * 否则 21,947 与 50,923 又会同屏出现。
   */
  it('有区间清单时全库分区完全不渲染', () => {
    render(
      <CostMissingPrices
        entries={[{ providerId: 'a', modelId: 'b', usageCount: 12 }]}
        unavailableCount={12}
        archiveEntries={manyMissing(3)}
      />,
    )
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.queryByTestId('cost-missing-archive')).toBeNull()
    expect(screen.queryByTestId('cost-missing-archive-list')).toBeNull()
  })

  it('拿不到区间清单时全库清单降级到独立分区，并自带口径说明', () => {
    render(<CostMissingPrices entries={[]} unavailableCount={12} archiveEntries={manyMissing(3)} />)
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    const archive = screen.getByTestId('cost-missing-archive')
    expect(archive.textContent).toContain(zh.overview.summary.missingArchiveTitle)
    expect(archive.textContent).toContain(zh.overview.summary.missingArchiveScopeHint)
    expect(screen.getByTestId('cost-missing-archive-list').children).toHaveLength(3)
    // 区间清单的行与口径说明都不出现，避免两个口径同屏。
    expect(screen.queryByTestId('cost-missing-list')).toBeNull()
    expect(screen.getByTestId('cost-missing').textContent).not.toContain(
      zh.overview.summary.missingRangeScopeHint,
    )
  })

  it('全库降级分区也按预览条数截断，不铺满卡片', () => {
    render(
      <CostMissingPrices
        entries={[]}
        unavailableCount={12}
        archiveEntries={manyMissing(MISSING_PRICE_PREVIEW + 4)}
      />,
    )
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.getByTestId('cost-missing-archive-list').children).toHaveLength(
      MISSING_PRICE_PREVIEW,
    )
  })

  /** 成因分不了就要说分不了，不能让用户以为「补个单价」对每一条都成立。 */
  it('始终说明本页不区分缺价成因，并给出补价格入口', () => {
    render(<CostMissingPrices entries={manyMissing(2)} unavailableCount={12} />)
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    const panel = screen.getByTestId('cost-missing').textContent ?? ''
    expect(panel).toContain(zh.overview.summary.missingCauseHint)
    expect(panel).toContain(zh.overview.summary.missingFixHint)
  })

  /** 数据可选中复制：口径混乱时用户第一件事就是把数字复制出来自己算。 */
  it('清单里的 provider / model 与记录数都可选中', () => {
    render(<CostMissingPrices entries={manyMissing(1)} unavailableCount={12} />)
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    const row = screen.getByTestId('cost-missing-entry-private-model-00')
    for (const span of Array.from(row.querySelectorAll('span'))) {
      expect(span.className).toContain('select-text')
    }
  })
})

describe('SummaryCards 的口径传递', () => {
  it('区间清单为空时才把全库清单传下去', () => {
    render(
      <SummaryCards
        summary={summary(3)}
        missingPrices={[]}
        archiveMissingPrices={manyMissing(2)}
      />,
    )
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.getByTestId('cost-missing-archive-list').children).toHaveLength(2)
  })

  it('两个清单都为空时只留解释文案，不渲染任何列表', () => {
    render(<SummaryCards summary={summary(3)} missingPrices={[]} archiveMissingPrices={[]} />)
    fireEvent.click(screen.getByTestId('cost-missing-toggle'))

    expect(screen.queryByTestId('cost-missing-list')).toBeNull()
    expect(screen.queryByTestId('cost-missing-archive')).toBeNull()
    expect(screen.getByTestId('cost-missing-empty').textContent).toBe(
      zh.overview.summary.missingNoIdentity,
    )
  })
})
