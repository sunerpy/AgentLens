import { describe, expect, it } from 'vitest'

import type { DateRange } from '@/generated'

import {
  RANGE_PRESETS,
  formatInstantInZone,
  formatOffsetStampInZone,
  parseOffsetStamp,
  quarterStartOf,
  rangeForPreset,
  rangeSpanDays,
  shiftIsoDate,
  systemTimezone,
  todayInTimezone,
  yearStartOf,
} from './localDate'
import { formatTimestamp as formatDetailTimestamp } from '@/views/detail/formatDetail'
import { formatTimestamp as formatLogTimestamp } from '@/views/diagnostics/logLevels'
import { formatTimestampInZone } from '@/views/hosts/hostsModel'

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
    // 日历对齐而非滚动窗口：2024-01-15 属于 Q1，起点是 01-01 而不是"回看 92 天"。
    ['thisQuarter', '2024-01-01', '2024-01-16', 15],
    ['thisYear', '2024-01-01', '2024-01-16', 15],
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
    expect(RANGE_PRESETS).toEqual([
      'today',
      'last7Days',
      'last30Days',
      'thisQuarter',
      'thisYear',
      'custom',
    ])
  })
})

/**
 * 季度与年是**日历对齐**的周期，所以真正值得钉住的是四个季度的边界月：落在季度首日、
 * 季度末日、跨季度相邻两天上，起点都必须跳到 1 / 4 / 7 / 10 月 1 日，绝不产生 02-01
 * 这种"季度从当月开始"的错误。
 */
describe('localDate/quarterStartOf 四季边界', () => {
  const cases: Array<[string, string]> = [
    ['2024-01-01', '2024-01-01'],
    ['2024-02-15', '2024-01-01'],
    ['2024-03-31', '2024-01-01'],
    ['2024-04-01', '2024-04-01'],
    ['2024-06-30', '2024-04-01'],
    ['2024-07-01', '2024-07-01'],
    ['2024-09-30', '2024-07-01'],
    ['2024-10-01', '2024-10-01'],
    ['2024-12-31', '2024-10-01'],
  ]

  for (const [isoDate, expected] of cases) {
    it(`${isoDate} 归入起于 ${expected} 的季度`, () => {
      expect(quarterStartOf(isoDate)).toBe(expected)
    })
  }

  it('季度末与次季度首日相邻却分属不同季度', () => {
    expect(quarterStartOf('2024-03-31')).toBe('2024-01-01')
    expect(quarterStartOf('2024-04-01')).toBe('2024-04-01')
  })

  it('月份补零到两位', () => {
    expect(quarterStartOf('2024-08-09')).toBe('2024-07-01')
    expect(quarterStartOf('2024-11-09')).toBe('2024-10-01')
  })
})

describe('localDate/yearStartOf 年边界', () => {
  it('年内任意一天都归到 01-01', () => {
    expect(yearStartOf('2024-01-01')).toBe('2024-01-01')
    expect(yearStartOf('2024-12-31')).toBe('2024-01-01')
  })

  it('跨年相邻两天分属不同年', () => {
    expect(yearStartOf('2023-12-31')).toBe('2023-01-01')
    expect(yearStartOf('2024-01-01')).toBe('2024-01-01')
  })
})

describe('localDate/rangeForPreset 季度与年的跨年边界', () => {
  it('12-31 的季度停在 10-01，不跨到次年', () => {
    const range = rangeForPreset(
      'thisQuarter',
      'UTC',
      'monday',
      new Date(Date.UTC(2024, 11, 31, 12)),
    )
    expect(range.startDate).toBe('2024-10-01')
    expect(range.endDateExclusive).toBe('2025-01-01')
    expect(rangeSpanDays(range)).toBe(92)
  })

  it('01-01 的季度与年都只含当天', () => {
    const newYear = new Date(Date.UTC(2024, 0, 1, 12))
    for (const preset of ['thisQuarter', 'thisYear'] as const) {
      const range = rangeForPreset(preset, 'UTC', 'monday', newYear)
      expect(range.startDate).toBe('2024-01-01')
      expect(range.endDateExclusive).toBe('2024-01-02')
      expect(rangeSpanDays(range)).toBe(1)
    }
  })

  it('闰年整年到 12-31 是 366 天', () => {
    const range = rangeForPreset('thisYear', 'UTC', 'monday', new Date(Date.UTC(2024, 11, 31, 12)))
    expect(range.startDate).toBe('2024-01-01')
    expect(rangeSpanDays(range)).toBe(366)
  })

  it('时区把"今天"推进次年时，季度与年一起跟到次年', () => {
    // 2023-12-31T23:30Z 在东八区已是 2024-01-01。
    const instant = new Date(Date.UTC(2023, 11, 31, 23, 30))
    expect(rangeForPreset('thisYear', 'UTC', 'monday', instant).startDate).toBe('2023-01-01')
    expect(rangeForPreset('thisYear', 'Asia/Shanghai', 'monday', instant).startDate).toBe(
      '2024-01-01',
    )
    expect(rangeForPreset('thisQuarter', 'UTC', 'monday', instant).startDate).toBe('2023-10-01')
    expect(rangeForPreset('thisQuarter', 'Asia/Shanghai', 'monday', instant).startDate).toBe(
      '2024-01-01',
    )
  })
})

/**
 * 用户原话：「所有的日期显示应该用同一套逻辑，都应该按照设置的时区进行显示」。
 *
 * 之前每个视图各有一份 `Intl.DateTimeFormat` 包装：主机页一份、明细页一份，日志页干脆只做字符串
 * 切片。三份实现可以在 locale、options 与非法时区的兜底行为上各自漂移，而屏幕上却写着同一个
 * 「报表时区」。下面这组用例把三条渲染路径**钉在同一个引擎上**：同一瞬时在同一时区必须给出
 * 逐字符相同的字符串，换时区必须一起变。
 */
describe('localDate/formatInstantInZone 单一时区渲染引擎', () => {
  /** 2024-01-15T23:30:45.123Z —— UTC 与东八区在这一刻分属不同日历日。 */
  const INSTANT = Date.UTC(2024, 0, 15, 23, 30, 45, 123)

  it('按目标时区渲染 YYYY-MM-DD HH:mm:ss，可跨日界', () => {
    expect(formatInstantInZone(INSTANT, 'UTC')).toBe('2024-01-15 23:30:45')
    expect(formatInstantInZone(INSTANT, 'Asia/Shanghai')).toBe('2024-01-16 07:30:45')
    expect(formatInstantInZone(INSTANT, 'Asia/Tokyo')).toBe('2024-01-16 08:30:45')
    expect(formatInstantInZone(INSTANT, 'America/New_York')).toBe('2024-01-15 18:30:45')
    // 半小时与 45 分钟偏移的时区：只按小时偏移会算错这两个。
    expect(formatInstantInZone(INSTANT, 'Asia/Kathmandu')).toBe('2024-01-16 05:15:45')
    expect(formatInstantInZone(INSTANT, 'Pacific/Chatham')).toBe('2024-01-16 13:15:45')
  })

  it('DST 由 tzdb 决定而不是固定偏移', () => {
    const january = Date.UTC(2024, 0, 15, 12)
    const july = Date.UTC(2024, 6, 15, 12)
    expect(formatInstantInZone(january, 'America/New_York')).toBe('2024-01-15 07:00:00')
    expect(formatInstantInZone(july, 'America/New_York')).toBe('2024-07-15 08:00:00')
    // 中国不实行夏令时，同一 UTC 时刻两季读数相同。
    expect(formatInstantInZone(january, 'Asia/Shanghai')).toBe('2024-01-15 20:00:00')
    expect(formatInstantInZone(july, 'Asia/Shanghai')).toBe('2024-07-15 20:00:00')
  })

  it('没有瞬时可显示时返回 null，绝不返回纪元 0', () => {
    expect(formatInstantInZone(null, 'UTC')).toBeNull()
    expect(formatInstantInZone(undefined, 'UTC')).toBeNull()
    expect(formatInstantInZone(Number.NaN, 'UTC')).toBeNull()
    expect(formatInstantInZone(Number.POSITIVE_INFINITY, 'UTC')).toBeNull()
    // 0 是真实瞬时（纪元），必须渲染出来而不是当成缺失。
    expect(formatInstantInZone(0, 'UTC')).toBe('1970-01-01 00:00:00')
  })

  it('非法时区回落 UTC 而不是抛错', () => {
    expect(formatInstantInZone(INSTANT, 'Not/AZone')).toBe('2024-01-15 23:30:45')
    expect(formatInstantInZone(INSTANT, '')).toBe('2024-01-15 23:30:45')
  })

  it('按时区记忆化的 formatter 不会串味', () => {
    expect(formatInstantInZone(INSTANT, 'Asia/Shanghai')).toBe('2024-01-16 07:30:45')
    expect(formatInstantInZone(INSTANT, 'UTC')).toBe('2024-01-15 23:30:45')
    expect(formatInstantInZone(INSTANT, 'Asia/Shanghai')).toBe('2024-01-16 07:30:45')
  })

  /**
   * 这一条是本轮修复的核心断言：主机「最近成功」、明细「时间」列、日志时间戳三条路径必须落在
   * 同一个引擎上。任何一条重新长出自己的 `Intl` 包装，这里就会红。
   */
  it('主机 / 明细 / 日志三条渲染路径对同一瞬时给出逐字符相同的结果', () => {
    const rfc3339 = '2024-01-15T23:30:45.123Z'
    expect(parseOffsetStamp(rfc3339)).toBe(INSTANT)

    for (const timezone of ['UTC', 'Asia/Shanghai', 'America/New_York', 'Asia/Kathmandu']) {
      const expected = formatInstantInZone(INSTANT, timezone)
      expect(expected).not.toBeNull()
      expect(formatTimestampInZone(INSTANT, timezone)).toBe(expected)
      expect(formatDetailTimestamp(INSTANT, timezone)).toBe(expected)
      expect(formatLogTimestamp(rfc3339, timezone)).toBe(expected)
      expect(formatOffsetStampInZone(rfc3339, timezone)).toBe(expected)
    }
  })

  it('换时区时三条路径一起变，不会只有一处跟着设置走', () => {
    const rfc3339 = '2024-01-15T23:30:45.123Z'
    const utc = [
      formatTimestampInZone(INSTANT, 'UTC'),
      formatDetailTimestamp(INSTANT, 'UTC'),
      formatLogTimestamp(rfc3339, 'UTC'),
    ]
    const shanghai = [
      formatTimestampInZone(INSTANT, 'Asia/Shanghai'),
      formatDetailTimestamp(INSTANT, 'Asia/Shanghai'),
      formatLogTimestamp(rfc3339, 'Asia/Shanghai'),
    ]
    expect(new Set(utc).size).toBe(1)
    expect(new Set(shanghai).size).toBe(1)
    expect(utc[0]).toBe('2024-01-15 23:30:45')
    expect(shanghai[0]).toBe('2024-01-16 07:30:45')
  })

  /** 明细列缺时间时用破折号而不是空单元格；主机列则由调用方决定「从未成功」。 */
  it('明细列的缺失兜底是破折号，主机列是 null', () => {
    expect(formatDetailTimestamp(null, 'UTC')).toBe('—')
    expect(formatDetailTimestamp(undefined, 'UTC')).toBe('—')
    expect(formatTimestampInZone(null, 'UTC')).toBeNull()
  })
})

/**
 * `TimeBucket.label` 与 `DateRange` 的两个端点都是 Rust 用 `chrono_tz` 按报表时区**算好并格式化过**
 * 的字符串。前端再拿它们过一次时区就会转换两次、时间偏移两遍。这组用例钉住的是「这类值不进
 * 时区引擎」——引擎只接受 epoch 毫秒或带偏移的 RFC 3339，预格式化字符串两者都不是，因此
 * `parseOffsetStamp` 必须拒绝它们，`formatOffsetStampInZone` 必须原样返回。
 */
describe('localDate/后端预格式化的标签不被二次转换', () => {
  const PRE_FORMATTED = [
    '2026-01-04', // day 桶
    '2026-01-04T13', // hour 桶
    '2026-W02', // week 桶
    '2026-01', // month 桶
  ]

  it('桶标签不是可解析的瞬时，因此拒绝进入时区引擎', () => {
    for (const label of PRE_FORMATTED) {
      expect(parseOffsetStamp(label)).toBeNull()
    }
  })

  it('误把桶标签喂进渲染函数也只会原样返回，不会移动日期', () => {
    for (const label of PRE_FORMATTED) {
      for (const timezone of ['UTC', 'Asia/Shanghai', 'America/New_York']) {
        expect(formatOffsetStampInZone(label, timezone)).toBe(label)
      }
    }
  })

  it('DateRange 端点同理：无论时区都原样保留日历日', () => {
    const range = rangeForPreset('last7Days', 'Asia/Shanghai', 'monday', INSTANT_2024_01_15_2330Z)
    // 时区已经在 rangeForPreset 里生效过一次（东八区已进入 01-16）。
    expect(range.startDate).toBe('2024-01-10')
    expect(range.endDateExclusive).toBe('2024-01-17')
    for (const endpoint of [range.startDate, range.endDateExclusive]) {
      expect(parseOffsetStamp(endpoint)).toBeNull()
      expect(formatOffsetStampInZone(endpoint, 'America/New_York')).toBe(endpoint)
    }
  })

  /**
   * 缺时区指示符的戳同样被拒绝。这不是洁癖：`Date.parse('2026-01-04T13:00:00')` 会按**运行机器**
   * 的本地时区解释，于是同一份数据在不同机器上渲染出不同时刻——比不转换更糟。
   */
  it('缺时区指示符的戳被拒绝，避免按运行机器时区误解', () => {
    expect(parseOffsetStamp('2026-01-04T13:00:00')).toBeNull()
    expect(parseOffsetStamp('2026-01-04 13:00:00+08:00')).toBeNull()
    expect(parseOffsetStamp('2026-01-04T13:00:00+0800')).toBeNull()
    expect(parseOffsetStamp('')).toBeNull()
  })

  it('合法带偏移戳被接受，Z 与 +00:00 等价', () => {
    expect(parseOffsetStamp('2024-01-15T23:30:45Z')).toBe(Date.UTC(2024, 0, 15, 23, 30, 45))
    expect(parseOffsetStamp('2024-01-15T23:30:45+00:00')).toBe(Date.UTC(2024, 0, 15, 23, 30, 45))
    expect(parseOffsetStamp('2024-01-16T07:30:45+08:00')).toBe(Date.UTC(2024, 0, 15, 23, 30, 45))
    expect(parseOffsetStamp('2024-01-15T18:30:45.123-05:00')).toBe(
      Date.UTC(2024, 0, 15, 23, 30, 45, 123),
    )
  })
})
