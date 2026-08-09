/**
 * Sidebar layout context. Split from the provider so a consumer importing the hook does not
 * drag in `@tanstack/react-query`, mirroring `theme/ThemeContext.ts`.
 */
import { createContext, useContext } from 'react'

import {
  DEFAULT_SIDEBAR_LAYOUT,
  type PersistedSidebarLayout,
  type SidebarLayout,
  type SidebarState,
} from './shellLayout'

export interface ShellLayoutValue {
  readonly layout: SidebarLayout
  readonly state: SidebarState
  /** Rendered rail width in px: `0` hidden, `64` collapsed, else the configured width. */
  readonly widthPx: number
  /** True while a hidden rail is being previewed by the hover recall strip. */
  readonly peeking: boolean
  toggleCollapsed: () => void
  setHidden: (hidden: boolean) => void
  togglePinned: () => void
  /** Live width update during a drag. Local only — nothing is written until {@link commitWidth}. */
  previewWidth: (width: number) => void
  /**
   * Persists a width. Called once on pointer-up, not per pointer-move.
   *
   * The width is passed explicitly rather than read from state: a keyboard step wants to
   * commit the value it just computed, and reading `layout.width` back would persist the
   * pre-step value because the enclosing render has not happened yet.
   */
  commitWidth: (width: number) => void
  setPeeking: (peeking: boolean) => void
  readonly isSaving: boolean
  readonly error: unknown
}

const FALLBACK: ShellLayoutValue = {
  layout: DEFAULT_SIDEBAR_LAYOUT,
  state: 'expanded',
  widthPx: DEFAULT_SIDEBAR_LAYOUT.width,
  peeking: false,
  toggleCollapsed: () => {},
  setHidden: () => {},
  togglePinned: () => {},
  previewWidth: () => {},
  commitWidth: () => {},
  setPeeking: () => {},
  isSaving: false,
  error: null,
}

export const ShellLayoutContext = createContext<ShellLayoutValue>(FALLBACK)

export function useShellLayout(): ShellLayoutValue {
  return useContext(ShellLayoutContext)
}

export type { PersistedSidebarLayout, SidebarLayout, SidebarState }
