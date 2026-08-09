/**
 * Shared report-range state — the single source of truth for "which window am I looking at".
 *
 * Owner: W8 prep (shell/infrastructure). Consumed by todos 15, 16 and 17 so that the
 * overview, drilldown and detail views stay in lock-step instead of each inventing
 * their own range widget state.
 *
 * The provider component lives in `ReportRangeProvider.tsx`; this module holds only
 * types, constants, the reducer and the hook, so both files stay lint-clean.
 */
import { createContext, useContext } from 'react'

import type { DateRange, Granularity, WeekStart } from '@/generated'
import type { RangePreset } from '@/lib/localDate'
import { rangeForPreset, rangeSpanDays, systemTimezone } from '@/lib/localDate'

/**
 * `app_settings` keys the shell reads and the settings view (todo 19) writes.
 * There is no second storage location for these.
 */
export const SETTING_KEY_TIMEZONE = 'report.timezone'
export const SETTING_KEY_WEEK_START = 'report.weekStart'

/** TanStack Query key for `get_settings`; todo 19 invalidates it after a settings write. */
export const SETTINGS_QUERY_KEY = ['settings'] as const

export interface ReportRangeState {
  /** Which preset button is active; `custom` means explicit dates were chosen. */
  preset: RangePreset
  /** Half-open `[startDate, endDateExclusive)`, plus the week-start the backend needs. */
  range: DateRange
  /** IANA report timezone. All bucketing happens in Rust using this name. */
  timezone: string
  /** Trend granularity; auto-derived from the range span unless pinned by the user. */
  granularity: Granularity
  /** True once the user pinned a granularity explicitly, so presets stop overriding it. */
  granularityPinned: boolean
}

export type ReportRangeAction =
  | { type: 'selectPreset'; preset: Exclude<RangePreset, 'custom'> }
  | { type: 'selectCustomRange'; startDate: string; endDateExclusive: string }
  | { type: 'setTimezone'; timezone: string }
  | { type: 'setWeekStart'; weekStart: WeekStart }
  | { type: 'setGranularity'; granularity: Granularity }
  | { type: 'resetGranularity' }

/**
 * Bucket size that keeps the trend chart readable at each span.
 *
 * The thresholds are chosen so a quarter lands on weeks and a year on months: 365 day-points
 * on one axis is a solid band, not a trend. `Granularity` has no `quarter`/`year` variant, so
 * a year-long window is drawn as 12 month buckets rather than one.
 */
export function defaultGranularity(range: DateRange): Granularity {
  const span = rangeSpanDays(range)
  if (span <= 1) return 'hour'
  if (span <= 31) return 'day'
  if (span <= 92) return 'week'
  return 'month'
}

export function initialReportRangeState(
  timezone: string = systemTimezone(),
  weekStart: WeekStart = 'monday',
  now: Date = new Date(),
): ReportRangeState {
  const range = rangeForPreset('last7Days', timezone, weekStart, now)
  return {
    preset: 'last7Days',
    range,
    timezone,
    granularity: defaultGranularity(range),
    granularityPinned: false,
  }
}

function withDerivedGranularity(state: ReportRangeState, range: DateRange): ReportRangeState {
  return {
    ...state,
    range,
    granularity: state.granularityPinned ? state.granularity : defaultGranularity(range),
  }
}

export function reportRangeReducer(
  state: ReportRangeState,
  action: ReportRangeAction,
): ReportRangeState {
  switch (action.type) {
    case 'selectPreset': {
      const range = rangeForPreset(action.preset, state.timezone, state.range.weekStart)
      return { ...withDerivedGranularity(state, range), preset: action.preset }
    }
    case 'selectCustomRange': {
      const range: DateRange = {
        startDate: action.startDate,
        endDateExclusive: action.endDateExclusive,
        weekStart: state.range.weekStart,
      }
      return { ...withDerivedGranularity(state, range), preset: 'custom' }
    }
    case 'setTimezone': {
      if (action.timezone === state.timezone) return state
      const next = { ...state, timezone: action.timezone }
      if (state.preset === 'custom') return next
      const range = rangeForPreset(state.preset, action.timezone, state.range.weekStart)
      return withDerivedGranularity(next, range)
    }
    case 'setWeekStart': {
      if (action.weekStart === state.range.weekStart) return state
      return { ...state, range: { ...state.range, weekStart: action.weekStart } }
    }
    case 'setGranularity':
      return { ...state, granularity: action.granularity, granularityPinned: true }
    case 'resetGranularity':
      return {
        ...state,
        granularity: defaultGranularity(state.range),
        granularityPinned: false,
      }
  }
}

export interface ReportRangeContextValue extends ReportRangeState {
  dispatch: (action: ReportRangeAction) => void
}

export const ReportRangeContext = createContext<ReportRangeContextValue | null>(null)

/** Read the shared range. Throws when used outside `ReportRangeProvider`. */
export function useReportRange(): ReportRangeContextValue {
  const value = useContext(ReportRangeContext)
  if (value === null) {
    throw new Error('useReportRange must be used inside <ReportRangeProvider>')
  }
  return value
}
