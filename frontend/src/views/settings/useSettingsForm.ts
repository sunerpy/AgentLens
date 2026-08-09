/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Settings form state: a draft layered over the persisted `app_settings` snapshot.
 *
 * The draft is a sparse patch rather than a copy of the loaded settings, so there is no
 * query-to-state synchronisation effect and no window in which the form shows stale values.
 * Saving writes only the keys this view owns; the Rust command layer upsert-merges them,
 * so keys owned by other views are never erased.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useState } from 'react'

import {
  SETTINGS_QUERY_KEY,
  SETTING_KEY_TIMEZONE,
  SETTING_KEY_WEEK_START,
  useReportRange,
} from '@/app/reportRange'
import type { AppSettings, WeekStart } from '@/generated'
import { getSettings, setSettings } from '@/lib/ipc'
import { systemTimezone } from '@/lib/localDate'

import {
  DEFAULT_LOCAL_INTERVAL_SECONDS,
  DEFAULT_REMOTE_INTERVAL_SECONDS,
  SETTING_KEY_ARCHIVE_PATH,
  SETTING_KEY_AUTO_REFRESH_ENABLED,
  SETTING_KEY_LOCAL_INTERVAL_MS,
  SETTING_KEY_REMOTE_INTERVAL_MS,
  autoRefreshEnabledFromSettings,
  intervalSecondsFromSettings,
  parseIntervalSeconds,
  type IntervalIssue,
} from './settingsKeys'

export interface SettingsFormValues {
  timezone: string
  weekStart: WeekStart
  autoRefreshEnabled: boolean
  localIntervalSeconds: string
  remoteIntervalSeconds: string
}

type SettingsDraft = Partial<SettingsFormValues>

export interface IntervalIssues {
  local: IntervalIssue | null
  remote: IntervalIssue | null
}

function persistedValues(settings: AppSettings): SettingsFormValues {
  const values = settings.values
  return {
    timezone: values[SETTING_KEY_TIMEZONE] || systemTimezone(),
    weekStart: values[SETTING_KEY_WEEK_START] === 'sunday' ? 'sunday' : 'monday',
    autoRefreshEnabled: autoRefreshEnabledFromSettings(values),
    localIntervalSeconds: String(
      intervalSecondsFromSettings(
        values,
        SETTING_KEY_LOCAL_INTERVAL_MS,
        DEFAULT_LOCAL_INTERVAL_SECONDS,
      ),
    ),
    remoteIntervalSeconds: String(
      intervalSecondsFromSettings(
        values,
        SETTING_KEY_REMOTE_INTERVAL_MS,
        DEFAULT_REMOTE_INTERVAL_SECONDS,
      ),
    ),
  }
}

export function useSettingsForm() {
  const queryClient = useQueryClient()
  const { dispatch } = useReportRange()
  const settings = useQuery({ queryKey: SETTINGS_QUERY_KEY, queryFn: getSettings })
  const [draft, setDraft] = useState<SettingsDraft>({})
  const [saved, setSaved] = useState(false)

  const persisted =
    settings.data === undefined
      ? undefined
      : {
          ...persistedValues(settings.data),
          archivePath: settings.data.values[SETTING_KEY_ARCHIVE_PATH] ?? '',
        }
  const values: SettingsFormValues | undefined =
    persisted === undefined ? undefined : { ...persisted, ...draft }

  /**
   * Derived from the current field text rather than latched on blur: the floor is a property
   * of the value, so a bad value must keep the save disabled for as long as it is on screen.
   */
  const local = values === undefined ? null : parseIntervalSeconds(values.localIntervalSeconds)
  const remote = values === undefined ? null : parseIntervalSeconds(values.remoteIntervalSeconds)
  const issues: IntervalIssues = { local: local?.issue ?? null, remote: remote?.issue ?? null }
  const hasIssue = issues.local !== null || issues.remote !== null

  const save = useMutation({
    /**
     * `localSeconds` / `remoteSeconds` are the already-validated numbers, so this never
     * converts a refused value: the backend rejects anything under the floor, and re-parsing
     * here would risk sending one anyway.
     */
    mutationFn: ({
      payload,
      localSeconds,
      remoteSeconds,
    }: {
      payload: SettingsFormValues
      localSeconds: number
      remoteSeconds: number
    }) =>
      setSettings({
        values: {
          [SETTING_KEY_TIMEZONE]: payload.timezone,
          [SETTING_KEY_WEEK_START]: payload.weekStart,
          [SETTING_KEY_AUTO_REFRESH_ENABLED]: String(payload.autoRefreshEnabled),
          [SETTING_KEY_LOCAL_INTERVAL_MS]: String(localSeconds * 1000),
          [SETTING_KEY_REMOTE_INTERVAL_MS]: String(remoteSeconds * 1000),
        },
      }),
    onSuccess: (result, { payload }) => {
      queryClient.setQueryData(SETTINGS_QUERY_KEY, result)
      dispatch({ type: 'setTimezone', timezone: payload.timezone })
      dispatch({ type: 'setWeekStart', weekStart: payload.weekStart })
      setDraft({})
      setSaved(true)
      void queryClient.invalidateQueries({ queryKey: SETTINGS_QUERY_KEY })
    },
  })

  const update = useCallback((patch: SettingsDraft) => {
    setSaved(false)
    setDraft((current) => ({ ...current, ...patch }))
  }, [])

  const dirty =
    values !== undefined &&
    persisted !== undefined &&
    (Object.keys(draft) as (keyof SettingsFormValues)[]).some(
      (key) => draft[key] !== undefined && draft[key] !== persisted[key],
    )

  return {
    values,
    archivePath: persisted?.archivePath ?? '',
    issues,
    hasIssue,
    dirty,
    saved: saved && !dirty,
    isPending: settings.isPending,
    error: settings.error ?? save.error ?? null,
    isSaving: save.isPending,
    refetch: () => void settings.refetch(),
    update,
    submit: () => {
      if (values === undefined) return
      if (local?.seconds == null || remote?.seconds == null) return
      save.mutate({ payload: values, localSeconds: local.seconds, remoteSeconds: remote.seconds })
    },
  }
}
