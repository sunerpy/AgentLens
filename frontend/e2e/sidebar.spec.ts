import path from 'node:path'

import { expect, test, type Locator, type Page } from '@playwright/test'

import {
  SETTING_KEY_SIDEBAR_COLLAPSED,
  SETTING_KEY_SIDEBAR_PINNED,
  SETTING_KEY_SIDEBAR_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_RAIL_WIDTH,
  SIDEBAR_RECALL_WIDTH,
  mainWidthCss,
} from '../src/app/layout/shellLayout'
import { THEME_KEYS, THEME_MODE, type ThemeKey } from '../src/app/theme/themes'
import { VIEW_KEYS } from '../src/app/views'
import { zh } from '../src/i18n/zh'
import { QA_DIR, mockCalls, openShell } from './harness'

/**
 * Left navigation rail. Component-level: real Chromium, mocked IPC.
 *
 * Persistence is asserted in two halves for the same reason `theme.spec.ts` does it — the mock
 * dataset is rebuilt on every page load, so it cannot itself model a restart:
 *  - the WRITE half checks which `set_settings` payloads each control produces (and, for hiding,
 *    that it produces none);
 *  - the READ half seeds `app_settings` and checks the shell boots on it, which is the state a
 *    real restart would find in the archive.
 * A reload *can* prove the negative directly, so `hidden` gets that stronger check too.
 */

/**
 * Screenshots go where the caller asks. Default is the gitignored in-repo QA directory, because
 * an absolute path baked into a committed spec would fail on any other machine; the acceptance
 * run overrides it to collect evidence outside the working tree.
 */
const SHOT_DIR = process.env.AGENTLENS_SHOT_DIR ?? path.join(QA_DIR, 'sidebar')

async function shot(page: Page, name: string): Promise<void> {
  await page.screenshot({
    path: path.join(SHOT_DIR, name),
    fullPage: false,
    animations: 'disabled',
  })
}

type SidebarSettings = Partial<
  Record<
    | typeof SETTING_KEY_SIDEBAR_COLLAPSED
    | typeof SETTING_KEY_SIDEBAR_PINNED
    | typeof SETTING_KEY_SIDEBAR_WIDTH,
    string
  >
>

async function openSidebar(
  page: Page,
  options: { theme?: ThemeKey; sidebar?: SidebarSettings } = {},
): Promise<void> {
  await openShell(page, {
    dataset: {
      settings: {
        values: {
          'report.timezone': 'UTC',
          'report.weekStart': 'monday',
          ...(options.theme === undefined ? {} : { 'ui.theme': options.theme }),
          ...options.sidebar,
        },
      },
    },
  })
  await expect(page.getByTestId('view-overview')).toBeVisible()
}

function rail(page: Page): Locator {
  return page.getByTestId('app-sidebar')
}

async function railWidth(page: Page): Promise<number> {
  const box = await rail(page).boundingBox()
  return Math.round(box?.width ?? -1)
}

/** Every `ui.sidebar.*` key that has ever been written, in call order. */
async function sidebarWrites(page: Page): Promise<string[]> {
  const calls = await mockCalls(page, 'set_settings')
  return calls.flatMap((call) => {
    const settings = call.args.settings as { values: Record<string, string> } | undefined
    return Object.keys(settings?.values ?? {}).filter((key) => key.startsWith('ui.sidebar.'))
  })
}

test('the rail renders every navigation item and switches views', async ({ page }) => {
  await openSidebar(page)

  const list = rail(page).getByRole('tablist')
  await expect(list).toHaveAttribute('aria-orientation', 'vertical')
  await expect(list).toHaveAttribute('aria-label', zh.sidebar.label)

  for (const view of VIEW_KEYS) {
    await expect(page.getByTestId(`nav-${view}`)).toHaveText(zh.nav[view])
  }

  for (const view of VIEW_KEYS.slice(1)) {
    await page.getByTestId(`nav-${view}`).click()
    await expect(page.getByTestId(`view-${view}`)).toBeVisible()
    await expect(page.getByTestId(`nav-${view}`)).toHaveAttribute('aria-selected', 'true')
  }

  await page.getByTestId('nav-overview').click()
  await expect(page.getByTestId('view-overview')).toBeVisible()
})

test('expanded → collapsed → hidden → recalled', async ({ page }) => {
  await openSidebar(page)

  expect(await railWidth(page)).toBe(SIDEBAR_DEFAULT_WIDTH)
  await expect(rail(page)).toHaveAttribute('data-state', 'expanded')
  await shot(page, 'state-expanded.png')

  await page.getByTestId('sidebar-toggle-collapsed').click()
  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')
  expect(await railWidth(page)).toBe(SIDEBAR_RAIL_WIDTH)
  // Labels are gone, so the accessible name has to come from somewhere else.
  for (const view of VIEW_KEYS) {
    const item = page.getByTestId(`nav-${view}`)
    await expect(item).toHaveText('')
    await expect(item).toHaveAttribute('aria-label', zh.nav[view])
    await expect(item).toHaveAttribute('title', zh.nav[view])
  }
  await shot(page, 'state-collapsed.png')

  await page.getByTestId('sidebar-toggle-hidden').click()
  await expect(rail(page)).toHaveCount(0)
  const recall = page.getByTestId('sidebar-recall')
  await expect(recall).toBeVisible()
  expect(Math.round((await recall.boundingBox())?.width ?? -1)).toBe(SIDEBAR_RECALL_WIDTH)
  // The hairline inside the strip is what makes the hidden state recoverable without guessing.
  await expect(page.getByTestId('sidebar-recall-hint')).toBeVisible()
  await shot(page, 'state-hidden.png')

  // Recalled from the preview's own toggle. The strip cannot be *mouse*-clicked: hovering it is
  // what opens the preview, and the preview then covers it. Keyboard focus + Enter still works,
  // and `AppSidebar.test.tsx` covers that path.
  await recall.hover()
  await expect(rail(page)).toHaveAttribute('data-floating', 'true')
  await shot(page, 'state-hidden-peek.png')
  const restore = page.getByTestId('sidebar-toggle-hidden')
  await expect(restore).toHaveAttribute('aria-label', zh.sidebar.show)
  await restore.click()

  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')
  await expect(page.getByTestId('sidebar-recall')).toHaveCount(0)
})

test('hovering the edge strip previews a hidden rail without un-hiding it', async ({ page }) => {
  await openSidebar(page)

  await page.getByTestId('sidebar-toggle-hidden').click()
  await expect(rail(page)).toHaveCount(0)

  await page.getByTestId('sidebar-recall').hover()
  await expect(rail(page)).toBeVisible()
  await expect(rail(page)).toHaveAttribute('data-state', 'hidden')
  await expect(rail(page)).toHaveAttribute('data-floating', 'true')
  // At its real width, not the 0 a hidden rail measures — that rendered a 1px border sliver
  // which `toBeVisible` happily accepted.
  expect(await railWidth(page)).toBe(SIDEBAR_DEFAULT_WIDTH)
  // The preview floats, so it must not have pushed the header aside.
  const header = page.getByRole('heading', { name: 'AgentLens' })
  await expect(header).toBeVisible()

  // A coordinate, not `heading.hover()`: the floating rail overlays the left of the content, so
  // the heading's own box sits underneath it and hovering it would land on the rail again.
  await page.mouse.move(1000, 400)
  await expect(rail(page)).toHaveCount(0)
  await expect(page.getByTestId('sidebar-recall')).toBeVisible()
})

test('the main column takes exactly the width the layout model specifies', async ({ page }) => {
  await openSidebar(page)

  const measure = () =>
    page.evaluate(() => {
      const main = document.querySelector('main')
      const shell = main?.parentElement?.parentElement
      return {
        main: Math.round(main?.parentElement?.getBoundingClientRect().width ?? -1),
        shell: Math.round(shell?.getBoundingClientRect().width ?? -1),
      }
    })

  const expectAgrees = async (layout: Parameters<typeof mainWidthCss>[0]) => {
    const { main, shell } = await measure()
    const spec = mainWidthCss(layout)
    const expected =
      spec === '100%' ? shell : shell - Number(/(\d+)px/.exec(spec)?.[1] ?? Number.NaN)
    expect(main, `main should be ${spec} of the shell`).toBe(expected)
  }

  const base = { collapsed: false, hidden: false, pinned: true, width: SIDEBAR_DEFAULT_WIDTH }
  await expectAgrees(base)

  await page.getByTestId('sidebar-toggle-collapsed').click()
  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')
  await expectAgrees({ ...base, collapsed: true })

  await page.getByTestId('sidebar-toggle-pinned').click()
  await expect(rail(page)).toHaveAttribute('data-pinned', 'false')
  await expectAgrees({ ...base, collapsed: true, pinned: false })

  await page.getByTestId('sidebar-toggle-hidden').click()
  await expect(rail(page)).toHaveCount(0)
  await expectAgrees({ ...base, collapsed: true, pinned: false, hidden: true })
})

test('dragging the separator resizes the rail and writes once, on release', async ({ page }) => {
  await openSidebar(page)

  const handle = page.getByTestId('sidebar-resize')
  const box = await handle.boundingBox()
  expect(box).not.toBeNull()
  if (box === null) return

  const y = box.y + box.height / 2
  await page.mouse.move(box.x + box.width / 2, y)
  await page.mouse.down()
  await page.mouse.move(box.x + box.width / 2 + 30, y, { steps: 6 })
  // Mid-drag: the rail has already moved, but nothing has been persisted yet.
  await expect.poll(() => railWidth(page)).toBe(SIDEBAR_DEFAULT_WIDTH + 30)
  expect(await sidebarWrites(page)).toEqual([])

  await page.mouse.up()
  await expect.poll(() => sidebarWrites(page)).toEqual([SETTING_KEY_SIDEBAR_WIDTH])

  // Past the clamp the rail stops growing rather than following the pointer.
  await page.mouse.down()
  await page.mouse.move(box.x + 600, y, { steps: 8 })
  await page.mouse.up()
  await expect.poll(() => railWidth(page)).toBe(SIDEBAR_MAX_WIDTH)
})

test('the separator resizes from the keyboard too', async ({ page }) => {
  await openSidebar(page)

  const handle = page.getByTestId('sidebar-resize')
  await expect(handle).toHaveAttribute('aria-valuenow', String(SIDEBAR_DEFAULT_WIDTH))
  await handle.focus()
  await page.keyboard.press('ArrowRight')

  await expect(handle).toHaveAttribute('aria-valuenow', String(SIDEBAR_DEFAULT_WIDTH + 10))
  expect(await railWidth(page)).toBe(SIDEBAR_DEFAULT_WIDTH + 10)
})

test('collapsing, pinning and resizing persist; hiding writes nothing', async ({ page }) => {
  await openSidebar(page)

  await page.getByTestId('sidebar-toggle-collapsed').click()
  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')
  await expect.poll(() => sidebarWrites(page)).toEqual([SETTING_KEY_SIDEBAR_COLLAPSED])

  await page.getByTestId('sidebar-toggle-pinned').click()
  await expect(rail(page)).toHaveAttribute('data-pinned', 'false')
  await expect
    .poll(() => sidebarWrites(page))
    .toEqual([SETTING_KEY_SIDEBAR_COLLAPSED, SETTING_KEY_SIDEBAR_PINNED])

  await page.getByTestId('sidebar-toggle-hidden').click()
  await expect(rail(page)).toHaveCount(0)
  await page.getByTestId('sidebar-recall').hover()
  await expect(rail(page)).toHaveAttribute('data-floating', 'true')

  // Still two writes: hiding and previewing are session-only acts.
  expect(await sidebarWrites(page)).toEqual([
    SETTING_KEY_SIDEBAR_COLLAPSED,
    SETTING_KEY_SIDEBAR_PINNED,
  ])
})

test('a persisted collapsed rail comes back collapsed at the next launch', async ({ page }) => {
  await openSidebar(page, {
    sidebar: {
      [SETTING_KEY_SIDEBAR_COLLAPSED]: 'true',
      [SETTING_KEY_SIDEBAR_PINNED]: 'false',
      [SETTING_KEY_SIDEBAR_WIDTH]: '300',
    },
  })

  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')
  await expect(rail(page)).toHaveAttribute('data-pinned', 'false')
  // Reading the layout back must not itself trigger a write.
  expect(await sidebarWrites(page)).toEqual([])

  await page.getByTestId('sidebar-toggle-collapsed').click()
  await expect(rail(page)).toHaveAttribute('data-state', 'expanded')
  // The persisted 300 is what an expanded rail reopens at, not the 260 default.
  expect(await railWidth(page)).toBe(300)
})

test('a hidden rail is back after a reload, because hiding is never persisted', async ({
  page,
}) => {
  await openSidebar(page, { sidebar: { [SETTING_KEY_SIDEBAR_COLLAPSED]: 'true' } })
  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')

  await page.getByTestId('sidebar-toggle-hidden').click()
  await expect(rail(page)).toHaveCount(0)

  await page.reload()
  await expect(page.getByTestId('view-overview')).toBeVisible()

  // The durable preference survived; the momentary one did not.
  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')
  await expect(page.getByTestId('sidebar-recall')).toHaveCount(0)
})

test('the rail is reachable and operable from the keyboard alone', async ({ page }) => {
  await openSidebar(page)

  // Tab order, not a roving tabindex: a rail is chrome the user tabs *into*, so every item has
  // to be reachable that way rather than only via arrow keys after landing on the first one.
  const tabbable = await page.evaluate(() =>
    [...document.querySelectorAll('[data-testid^="nav-"][role="tab"]')].map(
      (node) => (node as HTMLElement).tabIndex,
    ),
  )
  expect(tabbable).toHaveLength(VIEW_KEYS.length)
  for (const index of tabbable) expect(index).toBeGreaterThanOrEqual(0)

  await page.getByTestId('nav-overview').focus()
  await page.keyboard.press('ArrowDown')
  await expect(page.getByTestId(`nav-${VIEW_KEYS[1]}`)).toBeFocused()
  await expect(page.getByTestId(`view-${VIEW_KEYS[1]}`)).toBeVisible()

  await page.keyboard.press('End')
  const last = VIEW_KEYS[VIEW_KEYS.length - 1]
  await expect(page.getByTestId(`nav-${last}`)).toBeFocused()
  await expect(page.getByTestId(`view-${last}`)).toBeVisible()

  await page.keyboard.press('Home')
  await expect(page.getByTestId('nav-overview')).toBeFocused()

  // Enter must activate, so the rail works for anyone who never reaches for a mouse.
  await page.getByTestId('nav-hosts').focus()
  await page.keyboard.press('Enter')
  await expect(page.getByTestId('view-hosts')).toBeVisible()

  await page.getByTestId('sidebar-toggle-collapsed').focus()
  await page.keyboard.press('Enter')
  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')
})

test('the collapsed rail marks the selected item without relying on text', async ({ page }) => {
  await openSidebar(page)

  await page.getByTestId('nav-detail').click()
  await page.getByTestId('sidebar-toggle-collapsed').click()
  await expect(rail(page)).toHaveAttribute('data-state', 'collapsed')

  await expect(page.getByTestId('nav-detail')).toHaveText('')
  await expect(page.getByTestId('nav-marker-detail')).toBeAttached()
  await expect(page.getByTestId('nav-marker-overview')).toHaveCount(0)
})

test('the self-drawn titlebar keeps its full-width drag region above the rail', async ({
  page,
}) => {
  await openSidebar(page)

  const titlebar = page.getByTestId('titlebar')
  await expect(titlebar).toHaveAttribute('data-tauri-drag-region', 'deep')

  const bar = await titlebar.boundingBox()
  const shell = await page.evaluate(() => Math.round(document.body.clientWidth))
  expect(Math.round(bar?.width ?? -1), 'the rail must not cut the drag region').toBe(shell)

  // The rail starts below the titlebar, so it can neither overlap the drag region nor the
  // macOS traffic-light inset the bar reserves at its leading edge.
  const railBox = await rail(page).boundingBox()
  expect(railBox?.y ?? -1).toBeGreaterThanOrEqual((bar?.y ?? 0) + (bar?.height ?? 0))
})

type Rgb = readonly [number, number, number]

/** WCAG 2.x relative luminance of an sRGB triple. */
function luminance([red, green, blue]: Rgb): number {
  const [r, g, b] = [red, green, blue].map((raw) => {
    const value = raw / 255
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(left: Rgb, right: Rgb): number {
  const a = luminance(left)
  const b = luminance(right)
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)
}

/**
 * The rail is built from `--card` / `--border` / `--muted`, not the shadcn `--sidebar-*` preset,
 * precisely so this test can exist: those eight preset tokens were only ever declared under
 * `:root` and `.dark`, so `ocean` and `violet` would have inherited the LIGHT values and painted
 * a white rail inside a dark theme. Nothing rendered them, so nobody had ever seen it.
 */
test('every theme paints a legible rail, including the two dark tinted palettes', async ({
  page,
}) => {
  const surfaces = new Map<ThemeKey, string>()

  for (const theme of THEME_KEYS) {
    await openSidebar(page, { theme })
    await expect(page.getByTestId('app-sidebar')).toBeVisible()

    /*
      Transitions off before measuring, and this is load-bearing rather than tidiness. The boot
      cache makes each iteration open on the PREVIOUS theme and then flip, and the nav items carry
      `transition-colors`, so `getComputedStyle` mid-flip hands back an interpolated `oklab(...)`.
      That is how the first run of this test "failed": it compared a settled dark rail against a
      label still two thirds of the way from the light palette, and reported 3.50:1 for a pairing
      that actually measures 6.5:1.
    */
    await page.addStyleTag({
      content: '*, *::before, *::after { transition: none !important; animation: none !important }',
    })

    const paint = await page.evaluate(() => {
      /*
        Resolved through a canvas rather than parsed: this palette is authored in `oklch`, and
        `getComputedStyle` hands back `oklch(1 0 0)` verbatim, so a regex reading `rgb()` would
        see nothing. Painting one pixel makes the browser do its own oklch → sRGB conversion,
        which is also the conversion the user actually sees.
      */
      const canvas = document.createElement('canvas')
      canvas.width = 1
      canvas.height = 1
      const context = canvas.getContext('2d', { willReadFrequently: true })
      const toRgb = (colour: string): [number, number, number] => {
        if (context === null) return [-1, -1, -1]
        context.clearRect(0, 0, 1, 1)
        context.fillStyle = colour
        context.fillRect(0, 0, 1, 1)
        const [r, g, b] = context.getImageData(0, 0, 1, 1).data
        return [r, g, b]
      }
      const aside = document.querySelector('[data-testid="app-sidebar"]')
      const idle = document.querySelector('[data-testid="nav-hosts"]')
      const active = document.querySelector('[data-testid="nav-overview"]')
      const read = (node: Element | null) => (node === null ? null : getComputedStyle(node))
      const asideStyle = read(aside)
      return {
        dark: document.documentElement.classList.contains('dark'),
        rail: toRgb(asideStyle?.backgroundColor ?? 'transparent'),
        border: toRgb(asideStyle?.borderInlineEndColor ?? 'transparent'),
        idle: toRgb(read(idle)?.color ?? 'transparent'),
        activeFill: toRgb(read(active)?.backgroundColor ?? 'transparent'),
        activeText: toRgb(read(active)?.color ?? 'transparent'),
        canvas: toRgb(getComputedStyle(document.body).backgroundColor),
      }
    })

    expect(paint.dark, `${theme} dark class`).toBe(THEME_MODE[theme] === 'dark')

    // A dark theme must not paint a light rail — the exact defect the preset tokens would have
    // shipped. The rail follows `--card`, so its luminance tracks the palette's mode.
    const railLuminance = luminance(paint.rail)
    if (THEME_MODE[theme] === 'dark') {
      expect(railLuminance, `${theme} rail is too light for a dark theme`).toBeLessThan(0.2)
    } else {
      expect(railLuminance, `${theme} rail is too dark for a light theme`).toBeGreaterThan(0.6)
    }

    // 4.5:1 is the WCAG 1.4.3 AA floor for normal-size text, which these 14px labels are.
    expect(
      contrast(paint.rail, paint.idle),
      `${theme} unselected label is unreadable on the rail`,
    ).toBeGreaterThan(4.5)
    expect(
      contrast(paint.activeFill, paint.activeText),
      `${theme} selected label is unreadable on its own fill`,
    ).toBeGreaterThan(4.5)
    expect(paint.border, `${theme} rail has no edge against the content`).not.toEqual(paint.rail)
    expect(paint.rail, `${theme} rail does not step away from the canvas`).not.toEqual(paint.canvas)

    const railKey = paint.rail.join(',')
    for (const [other, colour] of surfaces) {
      expect(railKey, `${theme} paints the same rail as ${other}`).not.toBe(colour)
    }
    surfaces.set(theme, railKey)

    await shot(page, `theme-${theme}.png`)
  }

  expect(surfaces.size).toBe(THEME_KEYS.length)
})
