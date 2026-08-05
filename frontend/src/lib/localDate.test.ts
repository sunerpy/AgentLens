import { describe, expect, it } from 'vitest'

import type { DateRange } from '@/generated'

import {
  RANGE_PRESETS,
  rangeForPreset,
  rangeSpanDays,
  shiftIsoDate,
  systemTimezone,
  todayInTimezone,
} from './localDate'

/**
 * `@/lib/localDate` 是**手写的日历算术**：计划硬性禁止 date-fns / dayjs / moment，真正的
 * 分桶边界、DST 折叠与周起始全在 Rust 侧（`agentlens_core::query`），这里只负责
 * `DateRange` 经 IPC 传递所需的 `YYYY-MM-DD` 字符串。手写日期算术正是 off-by-one 的
 * 藏身处，因此本套用例全部断言**字面量字符串**，不用 toBeTruthy 之类的弱断言。
 *
 * 时间注入：所有涉及"今天"的断言都传入固定 `Date`，没有任何断言依赖真实
 * `new Date()`，因此跨机器、跨时区、跨执行次数完全确定。
 */

/** 固定瞬时：2024-01-15T23:30Z。UTC 与东八区在这一刻处于不同日历日。 */
const INSTANT_2024_01_15_2330Z = new Date(Date.UTC(2024, 0, 15, 23, 30))

describe('localDate/shiftIsoDate 月长边界', () => {
  it('跨 31 天月末进入下月', () => {
    expect(shiftIsoDate('2024-01-31', 1)).toBe('2024-02-01')
    expect(shiftIsoDate('2024-07-31', 1)).toBe('2024-08-01')
  })

  it('跨 30 天月末进入下月', () => {
    expect(shiftIsoDate('2024-04-30', 1)).toBe('2024-05-01')
    expect(shiftIsoDate('2024-06-30', 1)).toBe('2024-07-01')
  })

  it('平年 2 月停在 28 天', () => {
    expect(shiftIsoDate('2023-02-28', 1)).toBe('2023-03-01')
    expect(shiftIsoDate('2023-03-01', -1)).toBe('2023-02-28')
  })

  it('闰年 2 月多出 29 日', () => {
    expect(shiftIsoDate('2024-02-28', 1)).toBe('2024-02-29')
    expect(shiftIsoDate('2024-02-29', 1)).toBe('2024-03-01')
    expect(shiftIsoDate('2024-03-01', -1)).toBe('2024-02-29')
  })

  it('世纪年按 400 规则判闰：2000 是闰年，2100 不是', () => {
    expect(shiftIsoDate('2000-02-28', 1)).toBe('2000-02-29')
    expect(shiftIsoDate('2100-02-28', 1)).toBe('2100-03-01')
    expect(shiftIsoDate('2100-03-01', -1)).toBe('2100-02-28')
  })
})

describe('localDate/shiftIsoDate 年月回卷', () => {
  it('向前跨年', () => {
    expect(shiftIsoDate('2023-12-31', 1)).toBe('2024-01-01')
  })

  it('向后跨年', () => {
    expect(shiftIsoDate('2024-01-01', -1)).toBe('2023-12-31')
  })

  it('位移 0 是恒等变换', () => {
    expect(shiftIsoDate('2024-05-15', 0)).toBe('2024-05-15')
  })

  it('闰年整年 366 天、平年 365 天', () => {
    expect(shiftIsoDate('2024-01-01', 366)).toBe('2025-01-01')
    expect(shiftIsoDate('2023-01-01', 365)).toBe('2024-01-01')
  })

  it('个位月日补零到两位', () => {
    expect(shiftIsoDate('2024-09-08', 1)).toBe('2024-09-09')
    expect(shiftIsoDate('2024-10-01', -1)).toBe('2024-09-30')
  })

  it('正负位移互为逆运算（往返回到原点）', () => {
    for (const iso of ['2024-02-29', '2023-12-31', '2100-02-28', '2024-06-15']) {
      for (const days of [1, 7, 30, 365]) {
        expect(shiftIsoDate(shiftIsoDate(iso, days), -days)).toBe(iso)
      }
    }
  })

  it('UTC 正午锚点让位移免受 DST 转换扰动', () => {
    // 美国 2024 春季进位在 03-10、秋季回拨在 11-03；纯日历位移不得吞掉或重复任何一天。
    expect(shiftIsoDate('2024-03-09', 1)).toBe('2024-03-10')
    expect(shiftIsoDate('2024-03-10', 1)).toBe('2024-03-11')
    expect(shiftIsoDate('2024-11-02', 1)).toBe('2024-11-03')
    expect(shiftIsoDate('2024-11-03', 1)).toBe('2024-11-04')
  })
})

describe('localDate/shiftIsoDate 畸形输入', () => {
  /**
   * 这里钉住的是**实测行为**而非期望行为：`Date.UTC` 会把越界字段规范化而不是报错。
   * 调用方只从 `todayInTimezone` 与 `DateRange` 取值，两者都已是合法四位年份的
   * `YYYY-MM-DD`，所以规范化不会在生产路径上出现——但一旦有人接入用户手输的日期，
   * 下面这些结果就是他会拿到的东西。
   */
  it('越界月日被规范化而不是抛错', () => {
    expect(shiftIsoDate('2024-13-01', 0)).toBe('2025-01-01')
    expect(shiftIsoDate('2024-01-32', 0)).toBe('2024-02-01')
    expect(shiftIsoDate('2024-02-30', 0)).toBe('2024-03-01')
    expect(shiftIsoDate('2024-00-01', 0)).toBe('2023-12-01')
    expect(shiftIsoDate('2024-01-00', 0)).toBe('2023-12-31')
  })

  it('两位数年份被 Date.UTC 映射到 1900+：0000 与 0099 都不是本义', () => {
    // JS 的历史遗留语义。这是必须坚持四位年份输入的具体理由。
    expect(shiftIsoDate('0000-01-01', 0)).toBe('1900-01-01')
    expect(shiftIsoDate('0099-01-01', 0)).toBe('1999-01-01')
  })
})

describe('localDate/todayInTimezone', () => {
  it('按目标时区解析日历日，可跨日界', () => {
    expect(todayInTimezone('UTC', INSTANT_2024_01_15_2330Z)).toBe('2024-01-15')
    expect(todayInTimezone('America/New_York', INSTANT_2024_01_15_2330Z)).toBe('2024-01-15')
    // 东八区已进入次日，Kiritimati（+14）同理。
    expect(todayInTimezone('Asia/Shanghai', INSTANT_2024_01_15_2330Z)).toBe('2024-01-16')
    expect(todayInTimezone('Pacific/Kiritimati', INSTANT_2024_01_15_2330Z)).toBe('2024-01-16')
  })

  it('非法时区回落 UTC 而不是抛错', () => {
    expect(todayInTimezone('Not/AZone', INSTANT_2024_01_15_2330Z)).toBe('2024-01-15')
  })

  it('输出严格是 ISO 序 YYYY-MM-DD（en-CA 短日期形态）', () => {
    expect(todayInTimezone('UTC', new Date(Date.UTC(2024, 8, 5, 12)))).toBe('2024-09-05')
  })
})

describe('localDate/rangeForPreset 半开区间', () => {
  const cases: Array<[(typeof RANGE_PRESETS)[number], string, string, number]> = [
    ['today', '2024-01-15', '2024-01-16', 1],
    ['last7Days', '2024-01-09', '2024-01-16', 7],
    ['last30Days', '2023-12-17', '2024-01-16', 30],
    // custom 无固有跨度，回落 today 的跨度；选 custom 的调用方需自行给显式日期。
    ['custom', '2024-01-15', '2024-01-16', 1],
  ]

  for (const [preset, startDate, endDateExclusive, span] of cases) {
    it(`${preset} 产出 [${startDate}, ${endDateExclusive})，跨度 ${span} 天`, () => {
      const range = rangeForPreset(preset, 'UTC', 'monday', INSTANT_2024_01_15_2330Z)
      expect(range.startDate).toBe(startDate)
      expect(range.endDateExclusive).toBe(endDateExclusive)
      // 半开约定：结束端点是"今天的次日"，因此跨度恰好等于预设天数。
      expect(rangeSpanDays(range)).toBe(span)
    })
  }

  it('endDateExclusive 严格晚于最后一个被包含的日子', () => {
    const range = rangeForPreset('last7Days', 'UTC', 'monday', INSTANT_2024_01_15_2330Z)
    const lastIncluded = shiftIsoDate(range.endDateExclusive, -1)
    expect(lastIncluded).toBe('2024-01-15')
    expect(range.endDateExclusive > lastIncluded).toBe(true)
  })

  it('weekStart 原样透传给后端', () => {
    expect(rangeForPreset('today', 'UTC', 'sunday', INSTANT_2024_01_15_2330Z).weekStart).toBe(
      'sunday',
    )
    expect(rangeForPreset('today', 'UTC', 'monday', INSTANT_2024_01_15_2330Z).weekStart).toBe(
      'monday',
    )
  })

  it('时区决定"今天"是哪一天，从而决定整个区间', () => {
    const utc = rangeForPreset('today', 'UTC', 'monday', INSTANT_2024_01_15_2330Z)
    const shanghai = rangeForPreset('today', 'Asia/Shanghai', 'monday', INSTANT_2024_01_15_2330Z)
    expect(utc.startDate).toBe('2024-01-15')
    expect(shanghai.startDate).toBe('2024-01-16')
  })

  it('跨闰年 2 月回看：起点落在 02-24 而不是 02-23', () => {
    const range = rangeForPreset(
      'last7Days',
      'UTC',
      'monday',
      new Date(Date.UTC(2024, 2, 1, 12)), // 2024-03-01，2024 是闰年
    )
    expect(range.startDate).toBe('2024-02-24')
    expect(range.endDateExclusive).toBe('2024-03-02')
  })

  it('跨平年 2 月回看：起点落在 02-23', () => {
    const range = rangeForPreset(
      'last7Days',
      'UTC',
      'monday',
      new Date(Date.UTC(2023, 2, 1, 12)), // 2023-03-01，平年
    )
    expect(range.startDate).toBe('2023-02-23')
    expect(range.endDateExclusive).toBe('2023-03-02')
  })

  it('跨年回看 30 天', () => {
    const range = rangeForPreset(
      'last30Days',
      'UTC',
      'monday',
      new Date(Date.UTC(2024, 0, 2, 12)), // 2024-01-02
    )
    expect(range.startDate).toBe('2023-12-04')
    expect(range.endDateExclusive).toBe('2024-01-03')
  })
})

describe('localDate/rangeSpanDays', () => {
  const range = (startDate: string, endDateExclusive: string): DateRange => ({
    startDate,
    endDateExclusive,
    weekStart: 'monday',
  })

  it('空区间是 0 天', () => {
    expect(rangeSpanDays(range('2024-01-15', '2024-01-15'))).toBe(0)
  })

  it('倒置区间被夹到 0，不产出负数', () => {
    expect(rangeSpanDays(range('2024-01-16', '2024-01-15'))).toBe(0)
    expect(rangeSpanDays(range('2025-01-01', '2024-01-01'))).toBe(0)
  })

  it('按实际月长计数：闰年 2 月 29 天，平年 28 天', () => {
    expect(rangeSpanDays(range('2024-02-01', '2024-03-01'))).toBe(29)
    expect(rangeSpanDays(range('2023-02-01', '2023-03-01'))).toBe(28)
    expect(rangeSpanDays(range('2100-02-01', '2100-03-01'))).toBe(28)
  })

  it('按实际年长计数', () => {
    expect(rangeSpanDays(range('2024-01-01', '2025-01-01'))).toBe(366)
    expect(rangeSpanDays(range('2023-01-01', '2024-01-01'))).toBe(365)
  })

  it('跨 DST 月份仍是整数天（UTC 正午锚点的直接后果）', () => {
    // 若改成按本地瞬时相减，3 月在北半球会算出 30.958 天并被 round 成 31——恰好
    // 相同；但 11 月会算出 31.04 天。两者都必须是 31 与 30。
    expect(rangeSpanDays(range('2024-03-01', '2024-04-01'))).toBe(31)
    expect(rangeSpanDays(range('2024-11-01', '2024-12-01'))).toBe(30)
  })
})

describe('localDate/systemTimezone', () => {
  it('返回非空字符串，且可直接喂给 todayInTimezone', () => {
    const timezone = systemTimezone()
    expect(typeof timezone).toBe('string')
    expect(timezone.length).toBeGreaterThan(0)
    // 不断言具体时区（随机器而变），只断言它是 todayInTimezone 的合法输入。
    expect(todayInTimezone(timezone, INSTANT_2024_01_15_2330Z)).toMatch(/^\d{4}-\d{2}-\d{2}$/)
  })
})

describe('localDate/RANGE_PRESETS', () => {
  it('预设集合与 UI 按钮一一对应，顺序稳定', () => {
    expect(RANGE_PRESETS).toEqual(['today', 'last7Days', 'last30Days', 'custom'])
  })
})
