import { describe, expect, it } from 'vitest'

import type { CoverageNote } from '@/generated'
import {
  coverageReasonFor,
  coverageReasonIndex,
  coverageReasonPairs,
} from '@/views/overview/coverageReason'

function note(label: string, shortfalls: CoverageNote['shortfalls']): CoverageNote {
  return { label, shortfalls }
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
