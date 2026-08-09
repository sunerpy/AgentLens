/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Month-grid helpers for the custom-range calendar. Deliberately built on
 * `@/lib/localDate`'s `shiftIsoDate` (a DST-immune UTC-noon-anchored calendar-date shift)
 * so no date library is introduced. The single `getUTCDay()` call resolves the weekday
 * column of a month's first day, which a grid cannot be laid out without; it is presentation
 * only — every bucket boundary, week start and DST fold is still decided in Rust.
 */
import { shiftIsoDate } from '@/lib/localDate'

export interface MonthGrid {
  month: string
  days: (string | null)[]
}

export function monthOf(isoDate: string): string {
  return isoDate.slice(0, 7)
}

export function firstDayOfMonth(month: string): string {
  return `${month}-01`
}

export function shiftMonth(month: string, delta: number): string {
  const [year, monthIndex] = month.split('-').map(Number)
  const zeroBased = year * 12 + (monthIndex - 1) + delta
  const nextYear = Math.floor(zeroBased / 12)
  const nextMonth = zeroBased - nextYear * 12 + 1
  return `${String(nextYear).padStart(4, '0')}-${String(nextMonth).padStart(2, '0')}`
}

/** Monday-first column index (0..6) of a `YYYY-MM-DD` date. */
function mondayFirstWeekday(isoDate: string): number {
  const [year, month, day] = isoDate.split('-').map(Number)
  const sundayFirst = new Date(Date.UTC(year, month - 1, day, 12)).getUTCDay()
  return (sundayFirst + 6) % 7
}

export function buildMonthGrid(month: string): MonthGrid {
  const first = firstDayOfMonth(month)
  const days: (string | null)[] = new Array<string | null>(mondayFirstWeekday(first)).fill(null)
  let cursor = first
  while (monthOf(cursor) === month) {
    days.push(cursor)
    cursor = shiftIsoDate(cursor, 1)
  }
  while (days.length % 7 !== 0) {
    days.push(null)
  }
  return { month, days }
}

export function isWithinInclusive(isoDate: string, start: string, endInclusive: string): boolean {
  return isoDate >= start && isoDate <= endInclusive
}
