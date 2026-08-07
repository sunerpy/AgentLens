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

describe('formatTimestamp', () => {
  it('trims an RFC 3339 stamp to a readable local wall clock', () => {
    expect(formatTimestamp('2026-08-07T09:58:05.442+08:00')).toBe('2026-08-07 09:58:05')
  })

  it('passes through anything that is not shaped like a stamp', () => {
    expect(formatTimestamp('')).toBe('')
    expect(formatTimestamp('not-a-timestamp')).toBe('not-a-timestamp')
    // Right length, wrong separator: mangling it would be worse than showing it as-is.
    expect(formatTimestamp('2026-08-07 09:58:05.442')).toBe('2026-08-07 09:58:05.442')
  })
})

describe('entriesToText', () => {
  it('renders one line per entry with the level upper-cased', () => {
    expect(entriesToText([entry('error', 'archive unavailable')])).toBe(
      '2026-08-07 09:58:05 ERROR agentlens_tauri_lib::tray archive unavailable',
    )
  })

  it('joins multiple entries with newlines and yields empty text for none', () => {
    expect(entriesToText(ALL).split('\n')).toHaveLength(5)
    expect(entriesToText([])).toBe('')
  })
})
