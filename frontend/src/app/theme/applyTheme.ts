/**
 * DOM application of a theme, plus the boot cache.
 *
 * Why a `localStorage` cache exists at all: `app_settings` is the source of truth, but it is
 * read over async IPC, so the first paint would land on the default light palette and then snap
 * to a dark one. The cache is a *paint hint* only — never authoritative. On every launch
 * {@link reconcileTheme} overwrites it with whatever `app_settings` says, so a stale or absent
 * cache self-heals, and a machine with no cache simply shows one frame of the default.
 */
import { DEFAULT_THEME, parseTheme, themeMode, type ThemeKey } from './themes'

export const THEME_CACHE_KEY = 'agentlens.ui.theme'

export const THEME_ATTRIBUTE = 'data-theme'

export const DARK_CLASS = 'dark'

export function applyTheme(root: HTMLElement, theme: ThemeKey): void {
  root.setAttribute(THEME_ATTRIBUTE, theme)
  root.classList.toggle(DARK_CLASS, themeMode(theme) === 'dark')
}

/** Reads the currently applied theme back off the DOM; used by tests and by the picker. */
export function currentTheme(root: HTMLElement): ThemeKey {
  return parseTheme(root.getAttribute(THEME_ATTRIBUTE))
}

/**
 * Storage access is wrapped because a WebView with site data disabled throws on both read and
 * write; a missing paint hint must never take the app down.
 */
export function readCachedTheme(storage: Pick<Storage, 'getItem'> | null): ThemeKey {
  try {
    return parseTheme(storage?.getItem(THEME_CACHE_KEY) ?? null)
  } catch {
    return DEFAULT_THEME
  }
}

export function writeCachedTheme(storage: Pick<Storage, 'setItem'> | null, theme: ThemeKey): void {
  try {
    storage?.setItem(THEME_CACHE_KEY, theme)
  } catch {
    /* A read-only storage costs a one-frame flash on the next launch, nothing more. */
  }
}

export type ThemeStorage = Pick<Storage, 'getItem' | 'setItem'>

/**
 * Merely *touching* `localStorage` throws in a webview with site data disabled, so the lookup
 * itself is guarded rather than only the read and the write.
 */
export function defaultThemeStorage(): ThemeStorage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

/** Apply the cached hint before React mounts. Safe to call outside a browser. */
export function applyCachedTheme(storage: ThemeStorage | null = defaultThemeStorage()): ThemeKey {
  if (typeof document === 'undefined') return DEFAULT_THEME
  const theme = readCachedTheme(storage)
  applyTheme(document.documentElement, theme)
  return theme
}

/** Apply the authoritative theme and refresh the paint hint so the next launch matches. */
export function reconcileTheme(
  root: HTMLElement,
  theme: ThemeKey,
  storage: ThemeStorage | null = defaultThemeStorage(),
): void {
  applyTheme(root, theme)
  writeCachedTheme(storage, theme)
}
