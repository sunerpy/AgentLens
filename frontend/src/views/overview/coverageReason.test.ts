import { describe, expect, it } from 'vitest'

import type { CoverageNote } from '@/generated'
import {
  coverageExplanationFor,
  coverageReasonFor,
  coverageReasonIndex,
  coverageReasonPairs,
  hasCoverageGap,
  isBucketInProgress,
  type BucketWindow,
} from '@/views/overview/coverageReason'

function note(label: string, shortfalls: CoverageNote['shortfalls']): CoverageNote {
  return { label, shortfalls }
}

const HOUR_MS = 3_600_000
const DAY_MS = 86_400_000

/** 桶边界一律由后端按报表时区算好后送来，这里只给绝对时刻，不做任何日历推导。 */
function window(label: string, startUtcMs: number, endUtcMs: number): BucketWindow {
  return { label, startUtcMs, endUtcMs }
}

describe('coverageReasonIndex', () => {
  it('分离两种成因：部分覆盖与完全缺失', () => {
    const index = coverageReasonIndex([
      note('2026-01-02', [
        { hostId: 'local', source: 'opencode', partial: true },
        { hostId: 'build-box', source: 'codex', partial: false },
      ]),
    ])

    const reason = coverageReasonFor(index, '2026-01-02')
    expect(reason).not.toBeNull()
    expect(reason?.partial).toEqual([{ hostId: 'local', source: 'opencode', partial: true }])
    expect(reason?.missing).toEqual([{ hostId: 'build-box', source: 'codex', partial: false }])
  })

  it('按桶 label 建索引，未列出的桶返回 null', () => {
    const index = coverageReasonIndex([
      note('2026-01-02', [{ hostId: 'local', source: 'opencode', partial: true }]),
    ])

    expect(coverageReasonFor(index, '2026-01-01')).toBeNull()
    expect(coverageReasonFor(index, '2026-01-02')).not.toBeNull()
  })

  // 空 shortfalls 的 note 若被保留，UI 会渲染出一个「为什么不是完整覆盖」标题下什么都没有的空行。
  it('丢弃没有任何 shortfall 的 note，而不是留下空原因', () => {
    const index = coverageReasonIndex([note('2026-01-02', [])])

    expect(index.size).toBe(0)
    expect(coverageReasonFor(index, '2026-01-02')).toBeNull()
  })

  it('多个桶各自独立索引', () => {
    const index = coverageReasonIndex([
      note('2026-01-01', [{ hostId: 'a', source: 'opencode', partial: false }]),
      note('2026-01-03', [{ hostId: 'b', source: 'codex', partial: true }]),
    ])

    expect([...index.keys()]).toEqual(['2026-01-01', '2026-01-03'])
    expect(coverageReasonFor(index, '2026-01-01')?.missing).toHaveLength(1)
    expect(coverageReasonFor(index, '2026-01-03')?.partial).toHaveLength(1)
  })
})

describe('coverageReasonPairs', () => {
  // 部分覆盖排在前面：那是「采集断过 / 刚开始采集」，用户改配置就能修；完全缺失多半是没启用该源。
  it('部分覆盖的对排在完全缺失之前', () => {
    const index = coverageReasonIndex([
      note('2026-01-02', [
        { hostId: 'gone', source: 'hermes', partial: false },
        { hostId: 'half', source: 'opencode', partial: true },
      ]),
    ])
    const reason = coverageReasonFor(index, '2026-01-02')
    expect(reason).not.toBeNull()
    if (reason === null) return

    expect(coverageReasonPairs(reason).map((pair) => pair.hostId)).toEqual(['half', 'gone'])
  })
})

describe('isBucketInProgress', () => {
  const day = window('2026-08-10', Date.UTC(2026, 7, 10), Date.UTC(2026, 7, 11))

  it('当前时刻落在桶内即为进行中', () => {
    expect(isBucketInProgress(day, Date.UTC(2026, 7, 10, 5, 25))).toBe(true)
  })

  // 半开区间 [start, end)：右端等于当前时刻的桶已经结束，它的不完整就是真缺口。
  it('右端恰好等于当前时刻算已结束，右端大于当前时刻才算进行中', () => {
    expect(isBucketInProgress(day, day.endUtcMs)).toBe(false)
    expect(isBucketInProgress(day, day.endUtcMs - 1)).toBe(true)
  })

  it('左端恰好等于当前时刻算进行中', () => {
    expect(isBucketInProgress(day, day.startUtcMs)).toBe(true)
    expect(isBucketInProgress(day, day.startUtcMs - 1)).toBe(false)
  })

  // 判定只比较两个绝对时刻，因此与粒度无关：进行中的桶就是当前小时 / 本周 / 本月。
  it('小时 / 周 / 月粒度同样成立', () => {
    const now = Date.UTC(2026, 7, 10, 5, 25)
    const hour = window(
      '2026-08-10T05:00+00:00',
      Date.UTC(2026, 7, 10, 5),
      Date.UTC(2026, 7, 10, 6),
    )
    const week = window('2026-W33', Date.UTC(2026, 7, 10), Date.UTC(2026, 7, 10) + 7 * DAY_MS)
    const month = window('2026-08', Date.UTC(2026, 7, 1), Date.UTC(2026, 8, 1))

    expect(isBucketInProgress(hour, now)).toBe(true)
    expect(isBucketInProgress(week, now)).toBe(true)
    expect(isBucketInProgress(month, now)).toBe(true)
    // 各自的上一个桶都已结束。
    expect(
      isBucketInProgress(
        { ...hour, startUtcMs: hour.startUtcMs - HOUR_MS, endUtcMs: hour.startUtcMs },
        now,
      ),
    ).toBe(false)
    expect(
      isBucketInProgress(
        { ...week, startUtcMs: week.startUtcMs - 7 * DAY_MS, endUtcMs: week.startUtcMs },
        now,
      ),
    ).toBe(false)
    expect(
      isBucketInProgress(
        { ...month, startUtcMs: Date.UTC(2026, 6, 1), endUtcMs: month.startUtcMs },
        now,
      ),
    ).toBe(false)
  })

  // 未来的桶不是「进行中」：它压根没有相交的采集区间，后端给的是 None，说的是另一件事。
  it('完全在未来的桶不算进行中', () => {
    expect(isBucketInProgress(day, day.startUtcMs - DAY_MS)).toBe(false)
  })
})

/**
 * 进行中的桶为什么不该报成缺口：采集区间是 `[since, now]`，Full 要求区间压住整个桶，
 * 所以当前时刻所在的桶永远是 Partial —— 每天、每个源、每次刷新都如此。
 */
describe('coverageExplanationFor', () => {
  const TODAY = window('2026-08-10', Date.UTC(2026, 7, 10), Date.UTC(2026, 7, 11))
  const YESTERDAY = window('2026-08-09', Date.UTC(2026, 7, 9), Date.UTC(2026, 7, 10))
  const NOW = Date.UTC(2026, 7, 10, 5, 25)

  it('进行中的桶：只覆盖一部分的对被吸收，缺口清单无话可说', () => {
    const index = coverageReasonIndex([
      note('2026-08-10', [
        { hostId: 'local', source: 'opencode', partial: true },
        { hostId: 'build-box', source: 'codex', partial: true },
      ]),
    ])

    const explanation = coverageExplanationFor(index, TODAY, NOW)
    expect(explanation.inProgress).toBe(true)
    expect(explanation.pairs).toEqual([])
    expect(explanation.unknown).toBe(false)
    expect(hasCoverageGap(explanation)).toBe(false)
  })

  it('已结束的历史桶仍不完整：逐对列出，缺口清单照报', () => {
    const index = coverageReasonIndex([
      note('2026-08-09', [
        { hostId: 'local', source: 'opencode', partial: true },
        { hostId: 'build-box', source: 'codex', partial: false },
      ]),
    ])

    const explanation = coverageExplanationFor(index, YESTERDAY, NOW)
    expect(explanation.inProgress).toBe(false)
    expect(explanation.pairs.map((pair) => pair.hostId)).toEqual(['local', 'build-box'])
    expect(hasCoverageGap(explanation)).toBe(true)
  })

  // 「这个桶里完全没有它的采集区间」不是「桶还没结束」能解释的：那台机器这一整段都没采到。
  it('进行中的桶里完全没有采集区间的对仍要报', () => {
    const index = coverageReasonIndex([
      note('2026-08-10', [
        { hostId: 'local', source: 'opencode', partial: true },
        { hostId: 'build-box', source: 'codex', partial: false },
      ]),
    ])

    const explanation = coverageExplanationFor(index, TODAY, NOW)
    expect(explanation.inProgress).toBe(true)
    expect(explanation.pairs).toEqual([{ hostId: 'build-box', source: 'codex', partial: false }])
    expect(hasCoverageGap(explanation)).toBe(true)
  })

  it('桶右端恰好等于当前时刻：按已结束处理，六个对照旧列出', () => {
    const index = coverageReasonIndex([
      note('2026-08-09', [{ hostId: 'local', source: 'opencode', partial: true }]),
    ])

    const ended = coverageExplanationFor(index, YESTERDAY, YESTERDAY.endUtcMs)
    expect(ended.inProgress).toBe(false)
    expect(ended.pairs).toHaveLength(1)

    const running = coverageExplanationFor(index, YESTERDAY, YESTERDAY.endUtcMs - 1)
    expect(running.inProgress).toBe(true)
    expect(running.pairs).toEqual([])
  })

  it('非「天」粒度的进行中桶同样被吸收', () => {
    const hour = window(
      '2026-08-10T05:00+00:00',
      Date.UTC(2026, 7, 10, 5),
      Date.UTC(2026, 7, 10, 6),
    )
    const month = window('2026-08', Date.UTC(2026, 7, 1), Date.UTC(2026, 8, 1))
    const index = coverageReasonIndex([
      note(hour.label, [{ hostId: 'local', source: 'opencode', partial: true }]),
      note(month.label, [{ hostId: 'local', source: 'opencode', partial: true }]),
    ])

    expect(hasCoverageGap(coverageExplanationFor(index, hour, NOW))).toBe(false)
    expect(hasCoverageGap(coverageExplanationFor(index, month, NOW))).toBe(false)
  })

  it('后端没给 note 时：已结束的桶算诊断不出，进行中的桶不算', () => {
    const empty = coverageReasonIndex([])

    const ended = coverageExplanationFor(empty, YESTERDAY, NOW)
    expect(ended.unknown).toBe(true)
    expect(hasCoverageGap(ended)).toBe(true)

    const running = coverageExplanationFor(empty, TODAY, NOW)
    expect(running.unknown).toBe(false)
    expect(running.inProgress).toBe(true)
    expect(hasCoverageGap(running)).toBe(false)
  })
})
