import { describe, expect, it } from 'vitest'

import type {
  CostTotals,
  SeriesGroup,
  SeriesGroupDimension,
  SeriesPoint,
  TokenValues,
} from '@/generated'
import {
  OTHER_SERIES_KEY,
  TREND_GROUP_LIMIT,
  buildGroupedTrend,
  groupedSeriesTotal,
  splitGroups,
  trendGroups,
  visibleSeries,
  type TrendGroup,
  type TrendGroupPart,
} from '@/views/overview/trendGrouping'
import { toTrendRows, type TrendRow } from '@/views/overview/trendModel'

const PALETTE = ['c1', 'c2', 'c3', 'c4', 'c5', 'c6'] as const
const OTHER_COLOR = 'cOther'
const OTHER_LABEL = '其他'

function tokens(input: number): TokenValues {
  return {
    tokInput: input,
    tokOutput: 0,
    tokReasoning: 0,
    tokCacheRead: 0,
    tokCacheWrite: 0,
    totalInput: input,
  }
}

function cost(actualSum: number, estimatedSum = 0): CostTotals {
  return { actualSum, estimatedSum, unavailableCount: 0 }
}

function group(
  dimension: SeriesGroupDimension,
  id: string,
  label: string,
  values: readonly (number | null)[],
): SeriesGroup {
  return {
    dimension,
    id,
    label,
    series: values.map((value, day) => bucket(day, value)),
  }
}

function bucket(day: number, value: number | null): SeriesPoint {
  const start = Date.UTC(2026, 0, 1) + day * 86_400_000
  return {
    bucket: { startUtcMs: start, endUtcMs: start + 86_400_000, label: `2026-01-0${day + 1}` },
    coverage: value === null ? 'none' : 'full',
    tokens: value === null ? null : tokens(value),
    cost: value === null ? null : cost(value / 1000, value / 2000),
    messageCount: value === null ? null : 1,
    sessionRecordCount: value === null ? null : 0,
  }
}

function series(values: readonly (number | null)[]): TrendRow[] {
  return toTrendRows(values.map((value, day) => bucket(day, value)))
}

function part(id: string, label: string, rows: TrendRow[]): TrendGroupPart {
  const group: TrendGroup = {
    id,
    label,
    series: [],
    weight: 0,
  }
  return { group, rows }
}

describe('trendGroups：分组维度取值', () => {
  const groups: SeriesGroup[] = [
    group('source', 'opencode', 'opencode', [100, 200]),
    group('source', 'codex', 'codex', [250]),
    group('agent', 'build', 'build', [350]),
    group('agent', 'atlas', 'Atlas - Plan Executor', [200]),
    group('model', 'openai\0gpt-5-codex', 'openai / gpt-5-codex', [350]),
    group('model', 'kiro-auth\0claude-opus-5-max', 'kiro-auth / claude-opus-5-max', [200]),
    group('provider', 'openai', 'openai', [350]),
  ]

  it('不分组时不产生任何维度值', () => {
    expect(trendGroups(groups, 'none')).toEqual([])
  })

  it('按工具分组读 source 预聚合序列，并按 token 合计降序', () => {
    const selected = trendGroups(groups, 'tool')
    expect(selected.map((entry) => entry.id)).toEqual(['opencode', 'codex'])
    expect(selected.map((entry) => entry.weight)).toEqual([300, 250])
  })

  it('权重相同时按 id 排序，保证颜色分配跨刷新稳定', () => {
    const tied: SeriesGroup[] = [
      group('source', 'opencode', 'opencode', [100]),
      group('source', 'codex', 'codex', [100]),
    ]
    expect(trendGroups(tied, 'tool').map((group) => group.id)).toEqual(['codex', 'opencode'])
    expect(trendGroups([...tied].reverse(), 'tool').map((group) => group.id)).toEqual([
      'codex',
      'opencode',
    ])
  })

  it('按 agent 分组用 agentKey 聚合、用 agentRaw 展示', () => {
    const selected = trendGroups(groups, 'agent')
    expect(selected.map((entry) => entry.id)).toEqual(['build', 'atlas'])
    expect(selected[0].weight).toBe(350)
    expect(selected[1].label).toBe('Atlas - Plan Executor')
  })

  it('按模型分组使用后端的 provider 与 model 组合键', () => {
    const selected = trendGroups(groups, 'model')
    expect(selected.map((entry) => entry.label)).toEqual([
      'openai / gpt-5-codex',
      'kiro-auth / claude-opus-5-max',
    ])
    expect(selected[0].id).toBe('openai\0gpt-5-codex')
  })

  it('空归档结果产生空分组列表', () => {
    expect(trendGroups([], 'model')).toEqual([])
  })

  it('全区间五桶均为 0 的分组不进入图例候选', () => {
    const selected = trendGroups(
      [
        group('model', 'kiro-auth\0auto', 'kiro-auth / auto', [0, 0]),
        group('model', 'kiro-auth\0used', 'kiro-auth / used', [1, 0]),
      ],
      'model',
    )

    expect(selected.map((entry) => entry.id)).toEqual(['kiro-auth\0used'])
  })
})

describe('splitGroups：Top-N 收敛边界', () => {
  function groups(count: number): TrendGroup[] {
    return Array.from({ length: count }, (_unused, index) => ({
      id: `g${index}`,
      label: `g${index}`,
      series: [],
      weight: count - index,
    }))
  }

  it('数量刚好等于上限时不产生「其他」', () => {
    const split = splitGroups(groups(TREND_GROUP_LIMIT))
    expect(split.kept).toHaveLength(TREND_GROUP_LIMIT)
    expect(split.droppedCount).toBe(0)
  })

  it('刚好超过上限一项时也会折叠，而不是放宽到 N+1', () => {
    const split = splitGroups(groups(TREND_GROUP_LIMIT + 1))
    expect(split.kept).toHaveLength(TREND_GROUP_LIMIT)
    expect(split.droppedCount).toBe(1)
  })

  it('远超上限时只保留最重的 N 项', () => {
    const split = splitGroups(groups(40))
    expect(split.kept.map((group) => group.id)).toEqual(
      Array.from({ length: TREND_GROUP_LIMIT }, (_unused, index) => `g${index}`),
    )
    expect(split.droppedCount).toBe(40 - TREND_GROUP_LIMIT)
  })

  it('一个零用量分组和七个有用量分组只保留六个有用量分组', () => {
    const ranked = trendGroups(
      [
        group('model', 'a\0zero', 'a / zero', [0]),
        ...Array.from({ length: TREND_GROUP_LIMIT + 1 }, (_unused, index) =>
          group('model', `p\0used-${index}`, `p / used-${index}`, [index + 1]),
        ),
      ],
      'model',
    )

    const split = splitGroups(ranked)
    expect(split.kept).toHaveLength(TREND_GROUP_LIMIT)
    expect(split.kept.every((entry) => entry.weight > 0)).toBe(true)
    expect(split.kept.some((entry) => entry.id === 'a\0zero')).toBe(false)
  })

  it('数量不足上限或为零时都不折叠', () => {
    expect(splitGroups(groups(1)).droppedCount).toBe(0)
    expect(splitGroups(groups(0))).toEqual({ kept: [], droppedCount: 0 })
  })

  it('上限为 0 时全部折叠，且不会产生负数', () => {
    const split = splitGroups(groups(3), 0)
    expect(split.kept).toEqual([])
    expect(split.droppedCount).toBe(3)
    expect(splitGroups(groups(3), -5).droppedCount).toBe(3)
  })
})

describe('buildGroupedTrend：多序列行', () => {
  const total = series([null, 100, 0, 60])

  it('无覆盖桶让每条曲线都取 null，绝不补 0', () => {
    const grouped = buildGroupedTrend({
      total,
      parts: [part('a', 'A', series([null, 40, 0, 20]))],
      metric: 'tokens',
      droppedCount: 0,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })

    expect(grouped.rows[0].coverage).toBe('none')
    expect(grouped.rows[0].values.g0).toBeNull()
    expect(grouped.rows[0].coverageBand).toBe(1)
    // 覆盖完整但为 0 的桶是真实的 0。
    expect(grouped.rows[2].values.g0).toBe(0)
    expect(grouped.rows[2].coverageBand).toBeNull()
  })

  it('没有折叠时不产生「其他」序列', () => {
    const grouped = buildGroupedTrend({
      total,
      parts: [part('a', 'A', series([null, 40, 0, 20]))],
      metric: 'tokens',
      droppedCount: 0,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })
    expect(grouped.series.map((entry) => entry.key)).toEqual(['g0'])
    expect(grouped.rows[1].values[OTHER_SERIES_KEY]).toBeUndefined()
  })

  it('图例数值是整个区间合计，不是最后一个有覆盖桶', () => {
    const grouped = buildGroupedTrend({
      total,
      parts: [part('a', 'A', series([null, 40, 0, 20]))],
      metric: 'tokens',
      droppedCount: 0,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })

    expect(groupedSeriesTotal(grouped.rows, 'g0')).toBe(60)
    expect(groupedSeriesTotal(grouped.rows, 'missing')).toBeNull()
  })

  it('折叠时「其他」= 合计 − 已列出各项', () => {
    const grouped = buildGroupedTrend({
      total,
      parts: [part('a', 'A', series([null, 40, 0, 20])), part('b', 'B', series([null, 25, 0, 5]))],
      metric: 'tokens',
      droppedCount: 7,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })

    const other = grouped.series.at(-1)
    expect(other).toMatchObject({ key: OTHER_SERIES_KEY, label: OTHER_LABEL, isOther: true })
    expect(other?.color).toBe(OTHER_COLOR)
    expect(grouped.rows[1].values[OTHER_SERIES_KEY]).toBe(100 - 40 - 25)
    expect(grouped.rows[3].values[OTHER_SERIES_KEY]).toBe(60 - 20 - 5)
  })

  it('各分项之和超过合计时「其他」夹到 0，不出现负数', () => {
    const grouped = buildGroupedTrend({
      total,
      parts: [part('a', 'A', series([null, 90, 0, 50])), part('b', 'B', series([null, 80, 0, 40]))],
      metric: 'tokens',
      droppedCount: 3,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })
    expect(grouped.rows[1].values[OTHER_SERIES_KEY]).toBe(0)
    expect(grouped.rows[3].values[OTHER_SERIES_KEY]).toBe(0)
  })

  it('某分组在该桶完全缺行时按 0 计，而不是让整条线断掉', () => {
    const grouped = buildGroupedTrend({
      total,
      parts: [part('a', 'A', series([null, 40]))],
      metric: 'tokens',
      droppedCount: 0,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })
    expect(grouped.rows[1].values.g0).toBe(40)
    expect(grouped.rows[3].values.g0).toBe(0)
  })

  it('成本指标只画实际成本，不混入估算', () => {
    const grouped = buildGroupedTrend({
      total,
      parts: [part('a', 'A', series([null, 100, 0, 60]))],
      metric: 'cost',
      droppedCount: 0,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })
    expect(grouped.rows[1].values.g0).toBe(100 / 1000)
    expect(grouped.axisMax).toBeCloseTo(0.1)
  })

  it('颜色按顺序取自调色板，超出长度时循环', () => {
    const parts = Array.from({ length: PALETTE.length + 2 }, (_unused, index) =>
      part(`g${index}`, `G${index}`, series([null, 1, 0, 1])),
    )
    const grouped = buildGroupedTrend({
      total,
      parts,
      metric: 'tokens',
      droppedCount: 0,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })
    expect(grouped.series[0].color).toBe(PALETTE[0])
    expect(grouped.series[PALETTE.length].color).toBe(PALETTE[0])
  })

  it('全部无覆盖时轴上界回落到 1，避免 recharts 解析空 domain', () => {
    const grouped = buildGroupedTrend({
      total: series([null, null]),
      parts: [part('a', 'A', series([null, null]))],
      metric: 'tokens',
      droppedCount: 0,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })
    expect(grouped.axisMax).toBe(1)
  })

  it('没有任何分组时序列为空，行仍与合计一一对应', () => {
    const grouped = buildGroupedTrend({
      total,
      parts: [],
      metric: 'tokens',
      droppedCount: 0,
      palette: PALETTE,
      otherColor: OTHER_COLOR,
      otherLabel: OTHER_LABEL,
    })
    expect(grouped.series).toEqual([])
    expect(grouped.rows.map((r) => r.label)).toEqual(total.map((r) => r.label))
  })
})

describe('visibleSeries', () => {
  const series = [
    { key: 'g0', label: 'A', color: 'c1', isOther: false },
    { key: 'g1', label: 'B', color: 'c2', isOther: false },
    { key: OTHER_SERIES_KEY, label: OTHER_LABEL, color: OTHER_COLOR, isOther: true },
  ]

  it('未选中时返回全部曲线', () => {
    expect(visibleSeries(series, null).map((entry) => entry.key)).toEqual([
      'g0',
      'g1',
      OTHER_SERIES_KEY,
    ])
  })

  it('选中一项时只留那一条', () => {
    expect(visibleSeries(series, 'g1')).toEqual([series[1]])
  })

  // 「其他」是 合计 − 已列出各项，单独看它等于「被折叠掉的那些源加起来多少」，因此必须可单选。
  it('「其他」也能被单选', () => {
    expect(visibleSeries(series, OTHER_SERIES_KEY)).toEqual([series[2]])
  })

  // 分组键是按下标生成的（g0/g1/…），切换维度后同一个键会指向另一条线；
  // 因此认不出的键必须退回全览，而不是画出一条用户没选的曲线。
  it('认不出的键退回全览而不是画错的线', () => {
    expect(visibleSeries(series, 'g9')).toHaveLength(3)
  })

  it('返回新数组，不会把调用方的序列改掉', () => {
    const all = visibleSeries(series, null)
    all.pop()
    expect(series).toHaveLength(3)
  })
})
