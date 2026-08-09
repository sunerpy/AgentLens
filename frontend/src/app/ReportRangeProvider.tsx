/**
 * Provides the shared report range to every view, hydrating the report timezone and
 * week start from `app_settings` before the views run their first query.
 *
 * Owner: W8 prep (shell/infrastructure). Todos 15-17 consume it via `useReportRange()`;
 * todo 19 writes the two setting keys through `setSettings`.
 *
 * Children are intentionally held back until `get_settings` settles: querying a trend
 * with the wrong timezone would produce buckets that silently shift once settings load.
 * A settings failure renders the shared error panel with a retry, never a blank window.
 */
import { useMemo, useReducer, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'

import {
  ReportRangeContext,
  SETTINGS_QUERY_KEY,
  SETTING_KEY_TIMEZONE,
  SETTING_KEY_WEEK_START,
  initialReportRangeState,
  reportRangeReducer,
} from '@/app/reportRange'
import { ErrorState, LoadingState } from '@/components/app-state'
import type { WeekStart } from '@/generated'
import { getSettings } from '@/lib/ipc'
import { systemTimezone } from '@/lib/localDate'

function parseWeekStart(value: string | undefined): WeekStart {
  return value === 'sunday' ? 'sunday' : 'monday'
}

/** Mounted only once the resolved timezone / week start are known, so the reducer seeds once. */
function ResolvedReportRangeProvider({
  timezone,
  weekStart,
  children,
}: {
  timezone: string
  weekStart: WeekStart
  children: ReactNode
}) {
  const [state, dispatch] = useReducer(
    reportRangeReducer,
    initialReportRangeState(timezone, weekStart),
  )
  const value = useMemo(() => ({ ...state, dispatch }), [state])
  return <ReportRangeContext.Provider value={value}>{children}</ReportRangeContext.Provider>
}

export function ReportRangeProvider({ children }: { children: ReactNode }) {
  const settings = useQuery({ queryKey: SETTINGS_QUERY_KEY, queryFn: getSettings })

  if (settings.isPending) {
    return <LoadingState />
  }
  if (settings.isError) {
    return (
      <div className="p-8">
        <ErrorState error={settings.error} onRetry={() => void settings.refetch()} />
      </div>
    )
  }

  const values = settings.data.values
  return (
    <ResolvedReportRangeProvider
      timezone={values[SETTING_KEY_TIMEZONE] || systemTimezone()}
      weekStart={parseWeekStart(values[SETTING_KEY_WEEK_START])}
    >
      {children}
    </ResolvedReportRangeProvider>
  )
}
