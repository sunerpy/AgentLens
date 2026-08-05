/**
 * Presentation-level calendar-date string helpers.
 *
 * Owner: W8 prep (shell/infrastructure). Shared by todos 15-17.
 *
 * Deliberately dependency-free (no date-fns / dayjs / moment — hard plan constraint).
 * These helpers only ever produce the `YYYY-MM-DD` local-date strings that `DateRange`
 * carries across IPC. **All real calendar math — bucket boundaries, DST folds/gaps,
 * week starts, timezone conversion of timestamps — is done in Rust** (`agentlens_core::query`).
 * Nothing here interprets an epoch timestamp for display.
 */
import type { DateRange, WeekStart } from '@/generated'

/** The range presets the UI offers. `custom` means "the user picked explicit dates". */
export const RANGE_PRESETS = ['today', 'last7Days', 'last30Days', 'custom'] as const

export type RangePreset = (typeof RANGE_PRESETS)[number]

/** The system IANA timezone, or `UTC` when the runtime refuses to report one. */
export function systemTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

/**
 * The current local calendar date in `timezone`, formatted `YYYY-MM-DD`.
 *
 * Uses `en-CA`, whose short date format is already ISO-ordered, so no manual part
 * reassembly (and no off-by-one from `toISOString`) is involved.
 */
export function todayInTimezone(timezone: string, now: Date = new Date()): string {
  try {
    return new Intl.DateTimeFormat('en-CA', {
      timeZone: timezone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
    }).format(now)
  } catch {
    return new Intl.DateTimeFormat('en-CA', {
      timeZone: 'UTC',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
    }).format(now)
  }
}

/**
 * Shift a `YYYY-MM-DD` string by whole days.
 *
 * Anchored at UTC noon so the arithmetic can never be perturbed by a DST transition:
 * the result is a pure calendar-date shift, not an instant shift.
 */
export function shiftIsoDate(isoDate: string, days: number): string {
  const [year, month, day] = isoDate.split('-').map(Number)
  const anchor = Date.UTC(year, month - 1, day, 12)
  const shifted = new Date(anchor + days * 86_400_000)
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${shifted.getUTCFullYear()}-${pad(shifted.getUTCMonth() + 1)}-${pad(shifted.getUTCDate())}`
}

/** Number of whole days a preset spans; `custom` has no intrinsic span. */
const PRESET_SPAN_DAYS: Record<Exclude<RangePreset, 'custom'>, number> = {
  today: 1,
  last7Days: 7,
  last30Days: 30,
}

/**
 * Build the half-open `[startDate, endDateExclusive)` range for a preset.
 *
 * `custom` has no canonical span, so it falls back to `today`; callers that select
 * `custom` are expected to supply explicit dates.
 */
export function rangeForPreset(
  preset: RangePreset,
  timezone: string,
  weekStart: WeekStart,
  now: Date = new Date(),
): DateRange {
  const today = todayInTimezone(timezone, now)
  const span = preset === 'custom' ? PRESET_SPAN_DAYS.today : PRESET_SPAN_DAYS[preset]
  return {
    startDate: shiftIsoDate(today, -(span - 1)),
    endDateExclusive: shiftIsoDate(today, 1),
    weekStart,
  }
}

/** Whole-day span of a half-open range; used to pick a default granularity. */
export function rangeSpanDays(range: DateRange): number {
  const toUtcNoon = (isoDate: string) => {
    const [year, month, day] = isoDate.split('-').map(Number)
    return Date.UTC(year, month - 1, day, 12)
  }
  const span = (toUtcNoon(range.endDateExclusive) - toUtcNoon(range.startDate)) / 86_400_000
  return Math.max(0, Math.round(span))
}
