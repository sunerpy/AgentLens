/**
 * Theme registry — the single source of truth for which palettes exist and which of them are
 * dark. It is deliberately React-free and DOM-free so the mapping can be asserted directly.
 *
 * `mode` is not cosmetic metadata: `applyTheme` uses it to decide whether the `dark` class goes
 * on <html>, and `index.css` relies on that class for `@custom-variant dark`. A theme whose
 * `mode` disagrees with its `index.css` block would render half-light, half-dark, so the two
 * files must be changed together.
 */
export const SETTING_KEY_THEME = 'ui.theme'

export const THEME_KEYS = ['light', 'dark', 'forest', 'ocean', 'amber', 'violet'] as const

export type ThemeKey = (typeof THEME_KEYS)[number]

export type ThemeMode = 'light' | 'dark'

export const DEFAULT_THEME: ThemeKey = 'light'

export const THEME_MODE: Record<ThemeKey, ThemeMode> = {
  light: 'light',
  dark: 'dark',
  forest: 'light',
  ocean: 'dark',
  amber: 'dark',
  violet: 'dark',
}

/**
 * Swatch colours for the theme pickers, as literal OKLCH rather than `var(--…)` references:
 * a picker shows every theme at once, so each preview must render its own palette while the
 * document is still on another one. Each triple is `[background, card, accent]` and mirrors the
 * matching block in `index.css`.
 */
export const THEME_SWATCH: Record<ThemeKey, readonly [string, string, string]> = {
  light: ['oklch(1 0 0)', 'oklch(0.922 0 0)', 'oklch(0.205 0 0)'],
  dark: ['oklch(0.145 0 0)', 'oklch(0.269 0 0)', 'oklch(0.922 0 0)'],
  forest: ['oklch(0.99 0.006 152)', 'oklch(0.9 0.014 152)', 'oklch(0.44 0.09 157)'],
  ocean: ['oklch(0.17 0.024 250)', 'oklch(0.29 0.03 252)', 'oklch(0.74 0.13 225)'],
  amber: ['oklch(0.185 0.014 62)', 'oklch(0.3 0.022 64)', 'oklch(0.79 0.14 76)'],
  violet: ['oklch(0.165 0.026 300)', 'oklch(0.29 0.034 302)', 'oklch(0.74 0.15 302)'],
}

export function isThemeKey(value: unknown): value is ThemeKey {
  return typeof value === 'string' && (THEME_KEYS as readonly string[]).includes(value)
}

/** Any unrecognised or missing value resolves to {@link DEFAULT_THEME} rather than throwing. */
export function parseTheme(value: unknown): ThemeKey {
  return isThemeKey(value) ? value : DEFAULT_THEME
}

export function themeMode(theme: ThemeKey): ThemeMode {
  return THEME_MODE[theme]
}
