/**
 * Sidebar layout model — the shell's three-state left rail, React-free and DOM-free so the
 * state machine can be asserted directly (same split as `theme/themes.ts`).
 *
 * Ported from `work-kit/src/stores/layoutStore.ts`. Three flags are deliberately independent
 * rather than one enum, because they answer three different questions:
 *
 * | flag        | question                                | persisted? |
 * | ----------- | --------------------------------------- | ---------- |
 * | `collapsed` | show labels, or the icon rail only?     | yes        |
 * | `hidden`    | is the rail on screen at all?           | **no**     |
 * | `pinned`    | does the rail squeeze main, or float?   | yes        |
 * | `width`     | how wide is the expanded rail?          | yes        |
 *
 * `hidden` staying session-only is the load-bearing part of that table, not an oversight.
 * Hiding is a momentary act — "get out of the way, I want the chart full width" — so a
 * relaunch that still had no sidebar would read as *the sidebar is gone*, with the only way
 * back being a 12px strip the user has no reason to suspect exists. `collapsed` is the
 * durable preference and is the one that survives a restart.
 */

/** Whether the rail shows labels. Persisted: this is a durable preference. */
export const SETTING_KEY_SIDEBAR_COLLAPSED = 'ui.sidebar.collapsed'
/** Expanded rail width in CSS px. Persisted. */
export const SETTING_KEY_SIDEBAR_WIDTH = 'ui.sidebar.width'
/** Whether the rail squeezes main content (true) or floats over it (false). Persisted. */
export const SETTING_KEY_SIDEBAR_PINNED = 'ui.sidebar.pinned'

/**
 * Every `app_settings` key this module owns. `hidden` is absent **by design** — see the file
 * header. A test pins this list so a future key cannot be persisted by accident.
 */
export const SIDEBAR_SETTING_KEYS = [
  SETTING_KEY_SIDEBAR_COLLAPSED,
  SETTING_KEY_SIDEBAR_WIDTH,
  SETTING_KEY_SIDEBAR_PINNED,
] as const

/**
 * Collapsed rail width. 64px is inherited from work-kit and is not arbitrary: it holds a
 * 32px icon plate with 16px of gutter either side, so a centred icon lands on the same
 * optical axis as the icon in the expanded state and switching states does not shift it.
 */
export const SIDEBAR_RAIL_WIDTH = 64
export const SIDEBAR_MIN_WIDTH = 200
export const SIDEBAR_MAX_WIDTH = 320
export const SIDEBAR_DEFAULT_WIDTH = 260
/** Keyboard step for the resize separator, in px. */
export const SIDEBAR_WIDTH_STEP = 10

/**
 * Width of the edge strip that brings a hidden rail back on hover.
 *
 * work-kit uses 5px. That is inside the range where Fitts' law starts to bite for a target
 * with no visible affordance — a 5px invisible ribbon is a target the user finds by accident.
 * 12px is still narrower than the 24px (`px-6`) gutter the main column already reserves, so
 * it swallows no clickable content, and it is wide enough to hit on the first try.
 */
export const SIDEBAR_RECALL_WIDTH = 12

export type SidebarState = 'expanded' | 'collapsed' | 'hidden'

export interface SidebarLayout {
  readonly collapsed: boolean
  readonly hidden: boolean
  readonly pinned: boolean
  readonly width: number
}

/** The three persisted fields, i.e. {@link SidebarLayout} minus `hidden`. */
export type PersistedSidebarLayout = Omit<SidebarLayout, 'hidden'>

export const DEFAULT_SIDEBAR_LAYOUT: SidebarLayout = {
  collapsed: false,
  hidden: false,
  pinned: true,
  width: SIDEBAR_DEFAULT_WIDTH,
}

/**
 * `hidden` wins over `collapsed`: a hidden rail has no width, so asking whether it shows
 * labels is not a meaningful question. Ordering these the other way would report a hidden
 * rail as `collapsed` and the recall strip would never render.
 */
export function sidebarState(layout: SidebarLayout): SidebarState {
  if (layout.hidden) return 'hidden'
  return layout.collapsed ? 'collapsed' : 'expanded'
}

/** Clamps to `[200, 320]`; anything non-finite resolves to the default rather than throwing. */
export function clampSidebarWidth(value: number): number {
  if (!Number.isFinite(value)) return SIDEBAR_DEFAULT_WIDTH
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, Math.round(value)))
}

/** Reads a persisted width, falling back to the default for blank or malformed values. */
export function parseSidebarWidth(raw: string | undefined): number {
  const parsed = Number.parseInt((raw ?? '').trim(), 10)
  return Number.isFinite(parsed) ? clampSidebarWidth(parsed) : SIDEBAR_DEFAULT_WIDTH
}

/**
 * Reads a persisted boolean the same way `autoRefreshEnabledFromSettings` does, so the two
 * spellings of "false" in `app_settings` cannot diverge. An absent key takes `fallback`.
 */
export function parseSidebarFlag(raw: string | undefined, fallback: boolean): boolean {
  if (raw === undefined) return fallback
  const value = raw.trim().toLowerCase()
  if (['false', '0', 'off', 'no'].includes(value)) return false
  if (['true', '1', 'on', 'yes'].includes(value)) return true
  return fallback
}

/** Booleans are stored as `'true'` / `'false'`; `app_settings` values are strings. */
export function serializeSidebarLayout(
  patch: Partial<PersistedSidebarLayout>,
): Record<string, string> {
  const values: Record<string, string> = {}
  if (patch.collapsed !== undefined) {
    values[SETTING_KEY_SIDEBAR_COLLAPSED] = String(patch.collapsed)
  }
  if (patch.pinned !== undefined) {
    values[SETTING_KEY_SIDEBAR_PINNED] = String(patch.pinned)
  }
  if (patch.width !== undefined) {
    values[SETTING_KEY_SIDEBAR_WIDTH] = String(clampSidebarWidth(patch.width))
  }
  return values
}

/** Reads the persisted trio out of an `app_settings` snapshot. */
export function readSidebarLayout(
  values: Readonly<Record<string, string | undefined>>,
): PersistedSidebarLayout {
  return {
    collapsed: parseSidebarFlag(
      values[SETTING_KEY_SIDEBAR_COLLAPSED],
      DEFAULT_SIDEBAR_LAYOUT.collapsed,
    ),
    pinned: parseSidebarFlag(values[SETTING_KEY_SIDEBAR_PINNED], DEFAULT_SIDEBAR_LAYOUT.pinned),
    width: parseSidebarWidth(values[SETTING_KEY_SIDEBAR_WIDTH]),
  }
}

/** Rendered rail width in px: `0` hidden, `64` collapsed, else the configured width. */
export function sidebarWidthPx(layout: SidebarLayout): number {
  if (layout.hidden) return 0
  return layout.collapsed ? SIDEBAR_RAIL_WIDTH : clampSidebarWidth(layout.width)
}

/**
 * Width the main column should occupy. Nothing sets this as a style — the shell gets it from
 * flexbox, because an in-flow rail already subtracts its own px from a `flex-1` sibling and a
 * floating rail is `position: fixed` and subtracts none. This stays as the SPEC the layout is
 * measured against in `e2e/sidebar.spec.ts`: an oracle the CSS has to agree with, rather than a
 * second source of the same number that could drift out of step with it.
 */
export function mainWidthCss(layout: SidebarLayout): string {
  if (layout.hidden || !layout.pinned) return '100%'
  return `calc(100% - ${String(sidebarWidthPx(layout))}px)`
}
