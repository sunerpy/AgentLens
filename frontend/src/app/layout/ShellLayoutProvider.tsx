/**
 * Keeps the sidebar's durable state in step with `app_settings`.
 *
 * Same shape as `ThemeProvider`, and for the same reason: layout is presentation, so gating
 * the shell on `get_settings` would trade a one-frame width flash for a visible loading
 * screen. Until settings arrive the rail renders at {@link DEFAULT_SIDEBAR_LAYOUT}.
 *
 * `hidden` and `peeking` live in `useState` and are never written. See the table in
 * `shellLayout.ts` for why that asymmetry is deliberate.
 */
import { useCallback, useMemo, useState, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { SETTINGS_QUERY_KEY } from '@/app/reportRange'
import { getSettings, setSettings } from '@/lib/ipc'

import { ShellLayoutContext, type ShellLayoutValue } from './ShellLayoutContext'
import {
  DEFAULT_SIDEBAR_LAYOUT,
  clampSidebarWidth,
  readSidebarLayout,
  serializeSidebarLayout,
  sidebarState,
  sidebarWidthPx,
  type PersistedSidebarLayout,
  type SidebarLayout,
} from './shellLayout'

export function ShellLayoutProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient()
  const settings = useQuery({ queryKey: SETTINGS_QUERY_KEY, queryFn: getSettings })

  /** Session-only, by design. */
  const [hidden, setHiddenState] = useState(false)
  const [peeking, setPeekingState] = useState(false)
  /** Optimistic overlay on the persisted trio; cleared once a write comes back applied. */
  const [pending, setPending] = useState<Partial<PersistedSidebarLayout>>({})

  const persisted: PersistedSidebarLayout =
    settings.data === undefined ? DEFAULT_SIDEBAR_LAYOUT : readSidebarLayout(settings.data.values)

  const save = useMutation({
    mutationFn: (patch: Partial<PersistedSidebarLayout>) =>
      setSettings({ values: serializeSidebarLayout(patch) }),
    onSuccess: (result) => {
      queryClient.setQueryData(SETTINGS_QUERY_KEY, result)
      setPending({})
    },
  })
  const mutate = save.mutate

  const layout: SidebarLayout = useMemo(
    () => ({
      collapsed: pending.collapsed ?? persisted.collapsed,
      pinned: pending.pinned ?? persisted.pinned,
      width: clampSidebarWidth(pending.width ?? persisted.width),
      hidden,
    }),
    [hidden, pending.collapsed, pending.pinned, pending.width, persisted],
  )

  /** Optimistic first so the rail moves on the same frame as the click, then the write. */
  const persist = useCallback(
    (patch: Partial<PersistedSidebarLayout>) => {
      setPending((previous) => ({ ...previous, ...patch }))
      mutate(patch)
    },
    [mutate],
  )

  const collapsed = layout.collapsed
  const pinned = layout.pinned

  const toggleCollapsed = useCallback(() => {
    persist({ collapsed: !collapsed })
  }, [collapsed, persist])

  const togglePinned = useCallback(() => {
    persist({ pinned: !pinned })
  }, [persist, pinned])

  /**
   * Un-hiding also clears the hover preview: leaving `peeking` set would keep the rail
   * pinned to `position: fixed` after it rejoined the flow, i.e. a rail that overlaps the
   * content it just made room for.
   */
  const setHidden = useCallback((next: boolean) => {
    setHiddenState(next)
    if (!next) setPeekingState(false)
  }, [])

  const setPeeking = useCallback((next: boolean) => {
    setPeekingState(next)
  }, [])

  /** Drag: local only. One write per drag, on release — not one per pointer-move. */
  const previewWidth = useCallback((next: number) => {
    setPending((previous) => ({ ...previous, width: clampSidebarWidth(next) }))
  }, [])

  const commitWidth = useCallback(
    (next: number) => {
      const clamped = clampSidebarWidth(next)
      setPending((previous) => ({ ...previous, width: clamped }))
      mutate({ width: clamped })
    },
    [mutate],
  )

  const value: ShellLayoutValue = useMemo(
    () => ({
      layout,
      state: sidebarState(layout),
      widthPx: sidebarWidthPx(layout),
      peeking,
      toggleCollapsed,
      setHidden,
      togglePinned,
      previewWidth,
      commitWidth,
      setPeeking,
      isSaving: save.isPending,
      error: save.error,
    }),
    [
      commitWidth,
      layout,
      peeking,
      previewWidth,
      save.error,
      save.isPending,
      setHidden,
      setPeeking,
      toggleCollapsed,
      togglePinned,
    ],
  )

  return <ShellLayoutContext.Provider value={value}>{children}</ShellLayoutContext.Provider>
}
