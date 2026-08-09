/**
 * Applies the colour theme to <html> and keeps it in step with `app_settings`.
 *
 * Deliberately does NOT gate its children on `get_settings`: the theme is presentation, so
 * holding the whole shell back for it would trade a one-frame palette flash for a visible
 * loading screen. The boot cache in `applyTheme.ts` removes that flash on every launch after
 * the first, and `ReportRangeProvider` already blocks the *data* views on the same query, so
 * this provider adds no second IPC round trip.
 */
import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { SETTINGS_QUERY_KEY } from '@/app/reportRange'
import { getSettings, setSettings } from '@/lib/ipc'

import { applyTheme, currentTheme, reconcileTheme } from './applyTheme'
import { ThemeContext } from './ThemeContext'
import { SETTING_KEY_THEME, parseTheme, type ThemeKey } from './themes'

export function ThemeProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient()
  const settings = useQuery({ queryKey: SETTINGS_QUERY_KEY, queryFn: getSettings })

  /** Whatever the pre-render boot hint put on <html>; the fallback until settings arrive. */
  const [bootTheme] = useState(() => currentTheme(document.documentElement))
  const [optimistic, setOptimistic] = useState<ThemeKey | null>(null)

  const persisted =
    settings.data === undefined ? null : parseTheme(settings.data.values[SETTING_KEY_THEME])

  const save = useMutation({
    mutationFn: (next: ThemeKey) => setSettings({ values: { [SETTING_KEY_THEME]: next } }),
    onSuccess: (result) => {
      queryClient.setQueryData(SETTINGS_QUERY_KEY, result)
      setOptimistic(null)
    },
  })

  const mutate = save.mutate
  const setTheme = useCallback(
    (next: ThemeKey) => {
      setOptimistic(next)
      mutate(next)
    },
    [mutate],
  )

  const theme = optimistic ?? persisted ?? bootTheme

  useEffect(() => {
    const root = document.documentElement
    // Only the persisted value refreshes the boot hint: a pick whose write failed must not
    // survive a restart, or the next launch would show a theme the archive does not hold.
    if (optimistic === null && persisted !== null) {
      reconcileTheme(root, persisted)
      return
    }
    applyTheme(root, theme)
  }, [optimistic, persisted, theme])

  return (
    <ThemeContext.Provider value={{ theme, setTheme, isSaving: save.isPending, error: save.error }}>
      {children}
    </ThemeContext.Provider>
  )
}
