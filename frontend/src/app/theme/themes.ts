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

/**
 * `amber` is light, not dark. It was replanted as a warm paper palette because its accent hue
 * was the same gold `--chart-1` reserves for the partial-coverage hatch, and because the set
 * otherwise ran 2 light / 4 dark. The matching `[data-theme='amber']` block in `index.css`
 * carries the same flip; changing one without the other renders the shell half-light, half-dark.
 */
export const THEME_MODE: Record<ThemeKey, ThemeMode> = {
  light: 'light',
  dark: 'dark',
  forest: 'light',
  ocean: 'dark',
  amber: 'light',
  violet: 'dark',
}

/**
 * Swatch colours for the theme pickers, as literal OKLCH rather than `var(--…)` references:
 * a picker shows every theme at once, so each preview must render its own palette while the
 * document is still on another one. Each triple is `[background, border, accent]` and mirrors the
 * matching block in `index.css`. The middle stop is `--border` rather than `--card`: the swatch
 * paints it as a 1px inset ring, and a card fill is by design only a hair off the background, so
 * using it drew a ring that was not there.
 */
export const THEME_SWATCH: Record<ThemeKey, readonly [string, string, string]> = {
  light: ['oklch(0.972 0.002 264)', 'oklch(0.885 0.004 264)', 'oklch(0.22 0.006 264)'],
  dark: ['oklch(0.16 0.003 264)', 'oklch(0.305 0.007 264)', 'oklch(0.93 0.003 264)'],
  forest: ['oklch(0.968 0.007 156)', 'oklch(0.878 0.016 156)', 'oklch(0.42 0.095 158)'],
  ocean: ['oklch(0.175 0.016 252)', 'oklch(0.325 0.03 250)', 'oklch(0.72 0.13 232)'],
  amber: ['oklch(0.976 0.009 78)', 'oklch(0.884 0.02 76)', 'oklch(0.5 0.13 44)'],
  violet: ['oklch(0.17 0.018 300)', 'oklch(0.325 0.032 302)', 'oklch(0.73 0.15 302)'],
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
