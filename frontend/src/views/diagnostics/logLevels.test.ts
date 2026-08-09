import { describe, expect, it } from 'vitest'

import type { LogEntry, LogLevel } from '@/generated'

import { entriesToText, filterEntries, formatTimestamp, LOG_LEVELS } from './logLevels'

function entry(level: LogLevel, message = 'm'): LogEntry {
  return {
    timestamp: '2026-08-07T09:58:05.442+08:00',
    level,
    target: 'agentlens_tauri_lib::tray',
    message,
  }
}

const ALL = LOG_LEVELS.map((level) => entry(level, level))

describe('LOG_LEVELS', () => {
  it('is ordered from most to least severe', () => {
    expect(LOG_LEVELS).toEqual(['error', 'warn', 'info', 'debug', 'trace'])
  })
})

describe('filterEntries', () => {
  it('returns every entry for "all" without aliasing the input array', () => {
    const filtered = filterEntries(ALL, 'all')
    expect(filtered).toHaveLength(5)
    expect(filtered).not.toBe(ALL)
  })

  /**
   * The load-bearing case: selecting `warn` must still show errors. Filtering to exactly the
   * chosen level would hide the errors a user picked `warn` in order to find.
   */
  it('keeps entries at or above the chosen severity', () => {
    expect(filterEntries(ALL, 'error').map((item) => item.level)).toEqual(['error'])
    expect(filterEntries(ALL, 'warn').map((item) => item.level)).toEqual(['error', 'warn'])
    expect(filterEntries(ALL, 'info').map((item) => item.level)).toEqual(['error', 'warn', 'info'])
    expect(filterEntries(ALL, 'trace')).toHaveLength(5)
  })

  it('preserves the incoming order rather than re-sorting', () => {
    const reversed = [...ALL].reverse()
    expect(filterEntries(reversed, 'info').map((item) => item.level)).toEqual([
      'info',
      'warn',
      'error',
    ])
  })

  it('returns an empty list when nothing matches', () => {
    expect(filterEntries([entry('debug')], 'error')).toEqual([])
    expect(filterEntries([], 'all')).toEqual([])
  })
})

/**
 * 日志时间戳由 Rust 用**运行机器**的本地偏移写入（`chrono::Local`），所以它是一个明确的瞬时
 * 但带着用户没有选过的第三个时区。这里钉住的是「按报表时区渲染」这条统一口径：同一条记录在
 * 不同报表时区下必须给出不同且正确的墙上时间，否则日志页就会是全应用唯一不听报表时区的钟。
 */
describe('formatTimestamp', () => {
  it('把带偏移的 RFC 3339 戳按报表时区渲染成可读墙上时间', () => {
    // 种子戳本身是 +08:00，所以东八区读数与原文一致——这是回归基线。
    expect(formatTimestamp('2026-08-07T09:58:05.442+08:00', 'Asia/Shanghai')).toBe(
      '2026-08-07 09:58:05',
    )
  })

  it('同一条记录在不同报表时区下给出不同且正确的时间', () => {
    const stamp = '2026-08-07T09:58:05.442+08:00'
    expect(formatTimestamp(stamp, 'UTC')).toBe('2026-08-07 01:58:05')
    expect(formatTimestamp(stamp, 'Asia/Tokyo')).toBe('2026-08-07 10:58:05')
    expect(formatTimestamp(stamp, 'America/New_York')).toBe('2026-08-06 21:58:05')
  })

  it('跨日界也按目标时区落到正确的那一天', () => {
    // 2026-08-07T00:30+08:00 在 UTC 还是 08-06。丢掉偏移只做切片会读成 08-07。
    expect(formatTimestamp('2026-08-07T00:30:00.000+08:00', 'UTC')).toBe('2026-08-06 16:30:00')
  })

  it('Z 后缀与显式 +00:00 等价', () => {
    expect(formatTimestamp('2026-08-07T01:58:05Z', 'Asia/Shanghai')).toBe('2026-08-07 09:58:05')
    expect(formatTimestamp('2026-08-07T01:58:05+00:00', 'Asia/Shanghai')).toBe(
      '2026-08-07 09:58:05',
    )
  })

  it('非法时区回落 UTC 而不是抛错或清空整列', () => {
    expect(formatTimestamp('2026-08-07T09:58:05.442+08:00', 'Not/AZone')).toBe(
      '2026-08-07 01:58:05',
    )
  })

  it('passes through anything that is not shaped like a stamp', () => {
    expect(formatTimestamp('', 'UTC')).toBe('')
    expect(formatTimestamp('not-a-timestamp', 'UTC')).toBe('not-a-timestamp')
    // Right length, wrong separator: mangling it would be worse than showing it as-is.
    expect(formatTimestamp('2026-08-07 09:58:05.442', 'UTC')).toBe('2026-08-07 09:58:05.442')
  })

  /**
   * 缺偏移的戳**不做时区转换**：`Date.parse` 会把它当成运行机器的本地时间，于是同一份日志
   * 在不同机器上会显示不同时刻。按原文切片虽然不完美，但至少不撒谎。
   */
  it('缺时区指示符的戳只做切片，不按报表时区推断', () => {
    expect(formatTimestamp('2026-08-07T09:58:05.442', 'UTC')).toBe('2026-08-07 09:58:05')
    expect(formatTimestamp('2026-08-07T09:58:05.442', 'Asia/Tokyo')).toBe('2026-08-07 09:58:05')
  })
})

describe('entriesToText', () => {
  it('renders one line per entry with the level upper-cased', () => {
    expect(entriesToText([entry('error', 'archive unavailable')], 'Asia/Shanghai')).toBe(
      '2026-08-07 09:58:05 ERROR agentlens_tauri_lib::tray archive unavailable',
    )
  })

  it('joins multiple entries with newlines and yields empty text for none', () => {
    expect(entriesToText(ALL, 'Asia/Shanghai').split('\n')).toHaveLength(5)
    expect(entriesToText([], 'Asia/Shanghai')).toBe('')
  })

  /** 剪贴板必须与屏幕同口径，否则两个人对着同一份日志会对不上时间。 */
  it('剪贴板文本与列表用同一个报表时区', () => {
    expect(entriesToText([entry('error', 'boom')], 'UTC')).toContain('2026-08-07 01:58:05')
  })
})
