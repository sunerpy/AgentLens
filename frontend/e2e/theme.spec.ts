import { expect, test, type Page } from '@playwright/test'

import { THEME_KEYS, THEME_MODE, type ThemeKey } from '../src/app/theme/themes'
import { zh } from '../src/i18n/zh'
import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Colour-theme spec. Component-level: real Chromium, mocked IPC.
 *
 * The persistence round trip is asserted in two halves, because `installMockIpc` rebuilds the
 * seeded dataset on every page load and therefore cannot itself model a restart:
 *  - the WRITE half checks the `set_settings` payload actually carries `ui.theme`;
 *  - the READ half seeds `app_settings` with a theme and checks the shell boots on it, which is
 *    exactly the state a real restart would find in the archive.
 * `src/app/theme/theme.test.ts` covers the DOM + boot-cache round trip in isolation.
 */
const SETTING_KEY_THEME = 'ui.theme'

async function documentTheme(page: Page): Promise<{ theme: string | null; dark: boolean }> {
  return page.evaluate(() => ({
    theme: document.documentElement.getAttribute('data-theme'),
    dark: document.documentElement.classList.contains('dark'),
  }))
}

async function openTheme(page: Page, theme?: ThemeKey): Promise<void> {
  await openShell(
    page,
    theme === undefined
      ? {}
      : {
          dataset: {
            settings: {
              values: {
                'report.timezone': 'UTC',
                'report.weekStart': 'monday',
                [SETTING_KEY_THEME]: theme,
              },
            },
          },
        },
  )
  await expect(page.getByTestId('view-overview')).toBeVisible()
}

test('the shell boots on the neutral light theme when nothing is persisted', async ({ page }) => {
  await openTheme(page)

  expect(await documentTheme(page)).toEqual({ theme: 'light', dark: false })
  await expect(page.getByTestId('theme-menu-current')).toHaveText(zh.theme.names.light)
})

test('picking a theme applies it immediately and persists ui.theme', async ({ page }) => {
  await openTheme(page)

  await page.getByTestId('theme-menu-trigger').click()
  await expect(page.getByTestId('theme-menu')).toBeVisible()
  await page.getByTestId('theme-option-ocean').click()

  // The menu closes on pick, so the choice is judged against the page, not against a popover.
  await expect(page.getByTestId('theme-menu')).toHaveCount(0)
  await expect.poll(async () => (await documentTheme(page)).theme).toBe('ocean')
  expect(await documentTheme(page)).toEqual({ theme: 'ocean', dark: true })
  await expect(page.getByTestId('theme-menu-current')).toHaveText(zh.theme.names.ocean)

  await expect
    .poll(async () => {
      const last = (await mockCalls(page, 'set_settings')).at(-1)?.args.settings as
        | { values: Record<string, string> }
        | undefined
      return last?.values[SETTING_KEY_THEME]
    })
    .toBe('ocean')
})

test('a persisted theme is applied at the next launch', async ({ page }) => {
  // `amber` is a LIGHT palette. The literal `false` is deliberate rather than read from
  // `THEME_MODE`, which would make the assertion tautological: the mode is the thing under test.
  await openTheme(page, 'amber')

  expect(await documentTheme(page)).toEqual({ theme: 'amber', dark: false })
  await expect(page.getByTestId('theme-menu-current')).toHaveText(zh.theme.names.amber)
  // No write is issued just by reading a persisted theme back.
  expect(await mockCalls(page, 'set_settings')).toHaveLength(0)
})

test('an unrecognised persisted theme falls back to the default instead of an unstyled shell', async ({
  page,
}) => {
  await openShell(page, {
    dataset: {
      settings: {
        values: {
          'report.timezone': 'UTC',
          'report.weekStart': 'monday',
          [SETTING_KEY_THEME]: 'chartreuse',
        },
      },
    },
  })

  await expect(page.getByTestId('view-overview')).toBeVisible()
  expect(await documentTheme(page)).toEqual({ theme: 'light', dark: false })
})

test('every registered theme paints a distinct surface and keeps text legible', async ({
  page,
}) => {
  const seen = new Map<ThemeKey, string>()

  for (const theme of THEME_KEYS) {
    await openTheme(page, theme)
    expect(await documentTheme(page)).toEqual({
      theme,
      dark: THEME_MODE[theme] === 'dark',
    })

    const paint = await page.evaluate(() => {
      const style = getComputedStyle(document.body)
      const card = document.querySelector('[data-slot="card"]')
      return {
        background: style.backgroundColor,
        foreground: style.color,
        card: card === null ? '' : getComputedStyle(card).backgroundColor,
      }
    })

    // A theme whose token block failed to load would inherit the previous palette; a distinct
    // background per theme is the cheapest machine-checkable proof that it did load.
    expect(paint.background).not.toBe('')
    expect(paint.background).not.toBe(paint.foreground)
    expect(paint.card).not.toBe('')
    for (const [other, background] of seen) {
      expect(paint.background, `${theme} must not paint the same surface as ${other}`).not.toBe(
        background,
      )
    }
    seen.set(theme, paint.background)

    await qaScreenshot(page, `theme-${theme}.png`)
  }

  expect(seen.size).toBe(THEME_KEYS.length)
})

/** `oklch(L C H)` as declared in `index.css`; a custom property is not resolved by the engine. */
function parseOklch(value: string): { l: number; c: number; h: number } {
  const match = /oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)/.exec(value)
  expect(match, `not an oklch triple: ${value}`).not.toBeNull()
  if (match === null) return { l: 0, c: 0, h: 0 }
  return { l: Number(match[1]), c: Number(match[2]), h: Number(match[3]) }
}

function hueGap(left: number, right: number): number {
  const raw = Math.abs(left - right) % 360
  return raw > 180 ? 360 - raw : raw
}

/**
 * Chart palettes are the one part of a theme a screenshot diff cannot police: a line drawn at the
 * background's own lightness is invisible rather than wrong-coloured, and two neighbouring hues
 * read as one line rather than as an error. Both were live defects — the stock shadcn preset
 * shipped an identical `--chart-*` ramp for light and dark, which only surfaced once a theme
 * applied the `dark` class.
 */
test('every theme keeps its chart palette readable against its own surface', async ({ page }) => {
  for (const theme of THEME_KEYS) {
    await openTheme(page, theme)

    const tokens = await page.evaluate(() => {
      const style = getComputedStyle(document.documentElement)
      const read = (name: string) => style.getPropertyValue(name).trim()
      return {
        background: read('--background'),
        series: [1, 2, 3, 4, 5, 6].map((index) => read(`--series-${index}`)),
        other: read('--series-7'),
        // 2 / 4 / 5 only: those three are stroked as the ungrouped lines. `--chart-1` is the
        // partial-coverage *wash*, deliberately close to the surface, and is checked separately.
        lines: [2, 4, 5].map((index) => read(`--chart-${index}`)),
        hatch: read('--chart-1'),
      }
    })

    const background = parseOklch(tokens.background)
    const series = tokens.series.map(parseOklch)

    for (const [index, entry] of series.entries()) {
      expect(
        Math.abs(entry.l - background.l),
        `${theme} --series-${index + 1} has no lightness contrast against --background`,
      ).toBeGreaterThan(0.2)
    }
    // Ordered 2 → 4 → 5, each strictly further from the surface than the last, which is what
    // pins the ramp's *direction*: a contrast-only floor is satisfied by a ramp running the wrong
    // way, and that is exactly the shape the shadcn preset shipped for the dark palettes.
    const lines = tokens.lines.map(parseOklch)
    let previous = 0
    for (const [index, entry] of lines.entries()) {
      const contrast = Math.abs(entry.l - background.l)
      expect(
        contrast,
        `${theme} --chart-${[2, 4, 5][index]} has too little lightness contrast against --background`,
      ).toBeGreaterThan(0.35)
      expect(
        contrast,
        `${theme} --chart-${[2, 4, 5][index]} does not sit further from --background than the previous series`,
      ).toBeGreaterThan(previous)
      previous = contrast
    }
    // The primary line carries the metric the view exists for, so it gets the widest margin.
    expect(previous, `${theme} --chart-5 is not the most legible line`).toBeGreaterThan(0.55)

    // The hatch is a wash *behind* the data, so it is checked for presence, not for legibility.
    expect(
      Math.abs(parseOklch(tokens.hatch).l - background.l),
      `${theme} --chart-1 is indistinguishable from --background`,
    ).toBeGreaterThan(0.05)

    // Adjacent, because groups are assigned `--series-1` upward and a two-line chart uses 1 and 2.
    for (let index = 1; index < series.length; index += 1) {
      expect(
        hueGap(series[index - 1].h, series[index].h),
        `${theme} --series-${index} and --series-${index + 1} are too close in hue`,
      ).toBeGreaterThanOrEqual(90)
    }
    // The commonest case is three groups, so those three must be mutually separated too.
    expect(hueGap(series[0].h, series[2].h), `${theme} triad 1/3 too close`).toBeGreaterThanOrEqual(
      90,
    )

    // 其他 must not look like a named group even before its dashed stroke is considered.
    expect(parseOklch(tokens.other).c, `${theme} --series-7 is too saturated`).toBeLessThan(0.05)
  }
})

test('the settings card and the header menu drive the same selection', async ({ page }) => {
  await openTheme(page)

  await page.getByTestId('nav-settings').click()
  await expect(page.getByTestId('settings-appearance')).toBeVisible()

  const forest = page.getByTestId('settings-appearance').getByTestId('theme-option-forest')
  await expect(forest).toHaveAttribute('aria-checked', 'false')
  await forest.click()

  await expect(forest).toHaveAttribute('aria-checked', 'true')
  expect(await documentTheme(page)).toEqual({ theme: 'forest', dark: false })
  await expect(page.getByTestId('theme-menu-current')).toHaveText(zh.theme.names.forest)

  await page.getByTestId('theme-menu-trigger').click()
  await expect(page.getByTestId('theme-menu').getByTestId('theme-option-forest')).toHaveAttribute(
    'aria-checked',
    'true',
  )
})
