import { describe, expect, it } from 'vitest'

import {
  buildMonthGrid,
  firstDayOfMonth,
  isWithinInclusive,
  monthOf,
  shiftMonth,
} from './monthGrid'

/**
 * 自定义区间日历的月网格。它建在 `@/lib/localDate` 的 `shiftIsoDate`（UTC 正午锚点、
 * 免受 DST 影响的纯日历位移）之上，所以这里没有引入任何日期库；网格是纯展示层，
 * 真正的分桶边界与周起始仍由 Rust 决定。
 *
 * 网格是周一起首的固定 7 列布局，因此"前导空格数"和"总格数是 7 的倍数"是两条必须
 * 钉死的不变量：算错前导列会让整个月错位一格，用户点到的是隔壁那天。
 */
describe('monthGrid/monthOf 与 firstDayOfMonth', () => {
  it('取月份前缀', () => {
    expect(monthOf('2024-02-29')).toBe('2024-02')
    expect(monthOf('2023-12-31')).toBe('2023-12')
  })

  it('月首日就是 -01', () => {
    expect(firstDayOfMonth('2024-02')).toBe('2024-02-01')
    expect(firstDayOfMonth('2100-02')).toBe('2100-02-01')
  })

  it('两者互为逆向（月首日的月份等于原月份）', () => {
    for (const month of ['2024-01', '2024-02', '2024-12', '2100-02']) {
      expect(monthOf(firstDayOfMonth(month))).toBe(month)
    }
  })
})

describe('monthGrid/shiftMonth', () => {
  it('跨年回卷（两个方向）', () => {
    expect(shiftMonth('2024-01', -1)).toBe('2023-12')
    expect(shiftMonth('2024-12', 1)).toBe('2025-01')
  })

  it('整年与多年位移', () => {
    expect(shiftMonth('2024-06', 12)).toBe('2025-06')
    expect(shiftMonth('2024-01', -13)).toBe('2022-12')
    expect(shiftMonth('2024-11', 25)).toBe('2026-12')
  })

  it('位移 0 是恒等变换', () => {
    expect(shiftMonth('2024-06', 0)).toBe('2024-06')
  })

  it('正负位移互为逆运算', () => {
    for (const month of ['2024-01', '2024-12', '2023-07']) {
      for (const delta of [1, 6, 13, 24]) {
        expect(shiftMonth(shiftMonth(month, delta), -delta)).toBe(month)
      }
    }
  })

  it('年份始终补足 4 位（0 年附近也不退化成 1 位）', () => {
    expect(shiftMonth('0001-01', -1)).toBe('0000-12')
  })
})

describe('monthGrid/buildMonthGrid', () => {
  it('格数恒为 7 的倍数（7 列布局的前提）', () => {
    for (const month of ['2024-01', '2024-02', '2023-02', '2024-04', '2024-12', '2100-02']) {
      expect(buildMonthGrid(month).days.length % 7).toBe(0)
    }
  })

  it('闰年 2 月排出 29 天，前导 3 格空（2024-02-01 是周四）', () => {
    const { month, days } = buildMonthGrid('2024-02')
    expect(month).toBe('2024-02')
    expect(days).toHaveLength(35)
    expect(days.slice(0, 3)).toEqual([null, null, null])
    expect(days[3]).toBe('2024-02-01')
    expect(days[31]).toBe('2024-02-29')
    expect(days.slice(32)).toEqual([null, null, null])
  })

  it('平年 2 月只排 28 天，没有 02-29 这一格', () => {
    const { days } = buildMonthGrid('2023-02')
    expect(days.filter((day) => day !== null)).toHaveLength(28)
    expect(days).not.toContain('2023-02-29')
    expect(days[2]).toBe('2023-02-01')
  })

  it('世纪平年 2 月恰好铺满 4 周，无任何填充格（2100-02-01 是周一）', () => {
    const { days } = buildMonthGrid('2100-02')
    expect(days).toHaveLength(28)
    expect(days[0]).toBe('2100-02-01')
    expect(days[27]).toBe('2100-02-28')
    expect(days.every((day) => day !== null)).toBe(true)
  })

  it('400 整除的世纪年仍是闰年（2000-02 有 29 天）', () => {
    const { days } = buildMonthGrid('2000-02')
    expect(days.filter((day) => day !== null)).toHaveLength(29)
    expect(days).toContain('2000-02-29')
  })

  it('月首恰为周一时无前导空格', () => {
    const { days } = buildMonthGrid('2024-04')
    expect(days[0]).toBe('2024-04-01')
    expect(days.filter((day) => day !== null)).toHaveLength(30)
  })

  it('月首恰为周日时前导 6 格空，且需要 6 行', () => {
    const { days } = buildMonthGrid('2024-12')
    expect(days).toHaveLength(42)
    expect(days.slice(0, 6)).toEqual([null, null, null, null, null, null])
    expect(days[6]).toBe('2024-12-01')
    expect(days[36]).toBe('2024-12-31')
  })

  it('所有非空格都属于本月、连续、且无重复', () => {
    for (const month of ['2024-01', '2024-02', '2023-02', '2024-12', '2100-02']) {
      const filled = buildMonthGrid(month).days.filter((day): day is string => day !== null)
      expect(new Set(filled).size).toBe(filled.length)
      expect(filled.every((day) => monthOf(day) === month)).toBe(true)
      expect(filled[0]).toBe(`${month}-01`)
      // 逐日递增，中间不跳格。
      expect(filled).toEqual([...filled].sort())
    }
  })

  it('空格只出现在首尾，中间不夹空（否则日历会断开）', () => {
    for (const month of ['2024-02', '2024-12', '2023-02']) {
      const { days } = buildMonthGrid(month)
      const firstFilled = days.findIndex((day) => day !== null)
      const lastFilled = days.length - 1 - [...days].reverse().findIndex((day) => day !== null)
      expect(days.slice(firstFilled, lastFilled + 1).every((day) => day !== null)).toBe(true)
    }
  })
})

describe('monthGrid/isWithinInclusive', () => {
  it('闭区间：两个端点都算命中', () => {
    expect(isWithinInclusive('2024-01-01', '2024-01-01', '2024-01-07')).toBe(true)
    expect(isWithinInclusive('2024-01-07', '2024-01-01', '2024-01-07')).toBe(true)
    expect(isWithinInclusive('2024-01-04', '2024-01-01', '2024-01-07')).toBe(true)
  })

  it('端点外一天即落空', () => {
    expect(isWithinInclusive('2023-12-31', '2024-01-01', '2024-01-07')).toBe(false)
    expect(isWithinInclusive('2024-01-08', '2024-01-01', '2024-01-07')).toBe(false)
  })

  it('单日区间只命中那一天', () => {
    expect(isWithinInclusive('2024-01-01', '2024-01-01', '2024-01-01')).toBe(true)
    expect(isWithinInclusive('2024-01-02', '2024-01-01', '2024-01-01')).toBe(false)
  })

  it('倒置区间恒为 false，不会静默交换端点', () => {
    expect(isWithinInclusive('2024-01-04', '2024-01-07', '2024-01-01')).toBe(false)
  })

  it('跨年跨月比较靠 ISO 字典序，无需解析（补零是前提）', () => {
    expect(isWithinInclusive('2024-01-01', '2023-12-25', '2024-01-05')).toBe(true)
    expect(isWithinInclusive('2024-09-09', '2024-09-08', '2024-09-10')).toBe(true)
  })
})
