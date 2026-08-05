import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { DateRange } from '@/generated'

import {
  SETTINGS_QUERY_KEY,
  SETTING_KEY_TIMEZONE,
  SETTING_KEY_WEEK_START,
  defaultGranularity,
  initialReportRangeState,
  reportRangeReducer,
  type ReportRangeAction,
  type ReportRangeState,
} from './reportRange'

/**
 * 共享报表区间的 reducer。三个视图（总览 / 下钻 / 明细）都从它取"我在看哪个窗口"，
 * 所以它必须是**纯函数**：同样的 (state, action) 恒等产出，且绝不原地改写入参——
 * 一旦就地改写，React 会因为引用没变而跳过重渲染，界面停在旧区间上。
 *
 * `selectPreset` 这条分支内部会读"现在"，所以本文件用 `vi.setSystemTime` 钉住一个固定
 * 瞬时。没有任何断言依赖真实时钟。
 */
const FIXED_NOW = new Date(Date.UTC(2024, 0, 15, 23, 30))

const range = (startDate: string, endDateExclusive: string): DateRange => ({
  startDate,
  endDateExclusive,
  weekStart: 'monday',
})

function baseState(): ReportRangeState {
  return initialReportRangeState('UTC', 'monday', FIXED_NOW)
}

/** 递归冻结，用来证明 reducer 没有原地改写任何一层。 */
function deepFreeze<T>(value: T): T {
  if (typeof value === 'object' && value !== null) {
    for (const nested of Object.values(value)) deepFreeze(nested)
    Object.freeze(value)
  }
  return value
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(FIXED_NOW)
})

afterEach(() => {
  vi.useRealTimers()
})

describe('reportRange/defaultGranularity', () => {
  it('单日及以下用小时桶', () => {
    expect(defaultGranularity(range('2024-01-15', '2024-01-15'))).toBe('hour')
    expect(defaultGranularity(range('2024-01-15', '2024-01-16'))).toBe('hour')
  })

  it('两天起用天桶', () => {
    expect(defaultGranularity(range('2024-01-15', '2024-01-17'))).toBe('day')
    expect(defaultGranularity(range('2024-01-01', '2024-01-31'))).toBe('day')
  })

  it('阈值恰好落在 1 天与 2 天之间', () => {
    expect(defaultGranularity(range('2024-02-28', '2024-02-29'))).toBe('hour')
    expect(defaultGranularity(range('2024-02-28', '2024-03-01'))).toBe('day')
  })
})

describe('reportRange/initialReportRangeState', () => {
  it('默认落在 last7Days，粒度自动推导且未被钉住', () => {
    const state = baseState()
    expect(state.preset).toBe('last7Days')
    expect(state.range).toEqual(range('2024-01-09', '2024-01-16'))
    expect(state.timezone).toBe('UTC')
    expect(state.granularity).toBe('day')
    expect(state.granularityPinned).toBe(false)
  })

  it('时区与周起始按入参落地', () => {
    const state = initialReportRangeState('Asia/Shanghai', 'sunday', FIXED_NOW)
    expect(state.timezone).toBe('Asia/Shanghai')
    expect(state.range.weekStart).toBe('sunday')
    // 东八区在这一刻已进入次日，区间随之整体后移一天。
    expect(state.range).toEqual({
      startDate: '2024-01-10',
      endDateExclusive: '2024-01-17',
      weekStart: 'sunday',
    })
  })
})

describe('reportRange/reducer selectPreset', () => {
  it('today 切到小时桶', () => {
    const next = reportRangeReducer(baseState(), { type: 'selectPreset', preset: 'today' })
    expect(next.preset).toBe('today')
    expect(next.range).toEqual(range('2024-01-15', '2024-01-16'))
    expect(next.granularity).toBe('hour')
  })

  it('last30Days 停在天桶', () => {
    const next = reportRangeReducer(baseState(), { type: 'selectPreset', preset: 'last30Days' })
    expect(next.range).toEqual(range('2023-12-17', '2024-01-16'))
    expect(next.granularity).toBe('day')
  })

  it('沿用当前时区与周起始，不重置它们', () => {
    const state = initialReportRangeState('Asia/Shanghai', 'sunday', FIXED_NOW)
    const next = reportRangeReducer(state, { type: 'selectPreset', preset: 'today' })
    expect(next.timezone).toBe('Asia/Shanghai')
    expect(next.range.weekStart).toBe('sunday')
    expect(next.range.startDate).toBe('2024-01-16')
  })
})

describe('reportRange/reducer 粒度钉住语义', () => {
  it('手动设粒度即钉住', () => {
    const next = reportRangeReducer(baseState(), { type: 'setGranularity', granularity: 'week' })
    expect(next.granularity).toBe('week')
    expect(next.granularityPinned).toBe(true)
  })

  it('钉住后切换预设不再覆盖粒度', () => {
    const pinned = reportRangeReducer(baseState(), {
      type: 'setGranularity',
      granularity: 'week',
    })
    const next = reportRangeReducer(pinned, { type: 'selectPreset', preset: 'today' })
    // 区间照常更新，但粒度守住用户的选择。
    expect(next.range).toEqual(range('2024-01-15', '2024-01-16'))
    expect(next.granularity).toBe('week')
    expect(next.granularityPinned).toBe(true)
  })

  it('钉住后选自定义区间同样不覆盖粒度', () => {
    const pinned = reportRangeReducer(baseState(), { type: 'setGranularity', granularity: 'month' })
    const next = reportRangeReducer(pinned, {
      type: 'selectCustomRange',
      startDate: '2024-01-15',
      endDateExclusive: '2024-01-16',
    })
    expect(next.granularity).toBe('month')
  })

  it('resetGranularity 解钉并按当前区间重新推导', () => {
    const pinned = reportRangeReducer(
      reportRangeReducer(baseState(), { type: 'selectPreset', preset: 'today' }),
      { type: 'setGranularity', granularity: 'month' },
    )
    const next = reportRangeReducer(pinned, { type: 'resetGranularity' })
    expect(next.granularityPinned).toBe(false)
    // 当前区间是单日，重新推导应回到小时桶。
    expect(next.granularity).toBe('hour')
  })

  it('未钉住时切预设会自动改粒度（这是"自动"的定义）', () => {
    const today = reportRangeReducer(baseState(), { type: 'selectPreset', preset: 'today' })
    expect(today.granularity).toBe('hour')
    const back = reportRangeReducer(today, { type: 'selectPreset', preset: 'last30Days' })
    expect(back.granularity).toBe('day')
  })
})

describe('reportRange/reducer selectCustomRange', () => {
  it('原样采用显式端点，并把 preset 切成 custom', () => {
    const next = reportRangeReducer(baseState(), {
      type: 'selectCustomRange',
      startDate: '2023-11-01',
      endDateExclusive: '2023-12-01',
    })
    expect(next.preset).toBe('custom')
    expect(next.range).toEqual(range('2023-11-01', '2023-12-01'))
    expect(next.granularity).toBe('day')
  })

  it('保留既有 weekStart，不因自定义区间而重置', () => {
    const state = initialReportRangeState('UTC', 'sunday', FIXED_NOW)
    const next = reportRangeReducer(state, {
      type: 'selectCustomRange',
      startDate: '2024-01-01',
      endDateExclusive: '2024-01-02',
    })
    expect(next.range.weekStart).toBe('sunday')
    expect(next.granularity).toBe('hour')
  })
})

describe('reportRange/reducer setTimezone', () => {
  it('时区未变即返回同一个对象引用（避免无意义重渲染）', () => {
    const state = baseState()
    expect(reportRangeReducer(state, { type: 'setTimezone', timezone: 'UTC' })).toBe(state)
  })

  it('非 custom 预设下换时区会按新时区重算区间', () => {
    const next = reportRangeReducer(baseState(), {
      type: 'setTimezone',
      timezone: 'Asia/Shanghai',
    })
    expect(next.timezone).toBe('Asia/Shanghai')
    expect(next.range).toEqual(range('2024-01-10', '2024-01-17'))
  })

  it('custom 预设下换时区只改时区，用户选定的端点不动', () => {
    const custom = reportRangeReducer(baseState(), {
      type: 'selectCustomRange',
      startDate: '2023-11-01',
      endDateExclusive: '2023-12-01',
    })
    const next = reportRangeReducer(custom, { type: 'setTimezone', timezone: 'Asia/Shanghai' })
    expect(next.timezone).toBe('Asia/Shanghai')
    expect(next.range).toEqual(range('2023-11-01', '2023-12-01'))
    expect(next.preset).toBe('custom')
  })
})

describe('reportRange/reducer setWeekStart', () => {
  it('周起始未变即返回同一个对象引用', () => {
    const state = baseState()
    expect(reportRangeReducer(state, { type: 'setWeekStart', weekStart: 'monday' })).toBe(state)
  })

  it('换周起始只改这一个字段，端点与粒度不动', () => {
    const state = baseState()
    const next = reportRangeReducer(state, { type: 'setWeekStart', weekStart: 'sunday' })
    expect(next.range).toEqual({
      startDate: state.range.startDate,
      endDateExclusive: state.range.endDateExclusive,
      weekStart: 'sunday',
    })
    expect(next.granularity).toBe(state.granularity)
    expect(next.preset).toBe(state.preset)
  })
})

describe('reportRange/reducer 纯度', () => {
  const actions: ReportRangeAction[] = [
    { type: 'selectPreset', preset: 'today' },
    { type: 'selectPreset', preset: 'last30Days' },
    { type: 'selectCustomRange', startDate: '2023-11-01', endDateExclusive: '2023-12-01' },
    { type: 'setTimezone', timezone: 'Asia/Shanghai' },
    { type: 'setWeekStart', weekStart: 'sunday' },
    { type: 'setGranularity', granularity: 'week' },
    { type: 'resetGranularity' },
  ]

  for (const action of actions) {
    it(`${action.type} 不改写冻结的入参 state`, () => {
      const frozen = deepFreeze(baseState())
      // 若 reducer 就地改写，严格模式下的冻结对象会直接抛 TypeError。
      expect(() => reportRangeReducer(frozen, action)).not.toThrow()
      expect(frozen).toEqual(baseState())
    })
  }

  it('同一 (state, action) 重复调用产出相等结果（可重放）', () => {
    const state = baseState()
    for (const action of actions) {
      expect(reportRangeReducer(state, action)).toEqual(reportRangeReducer(state, action))
    }
  })
})

describe('reportRange/设置键', () => {
  it('两个 app_settings 键是唯一拼写来源', () => {
    expect(SETTING_KEY_TIMEZONE).toBe('report.timezone')
    expect(SETTING_KEY_WEEK_START).toBe('report.weekStart')
  })

  it('设置查询键与归档族互不重叠（设置写入不该让归档缓存失效）', () => {
    expect([...SETTINGS_QUERY_KEY]).toEqual(['settings'])
    expect(SETTINGS_QUERY_KEY[0]).not.toBe('archive')
  })
})
