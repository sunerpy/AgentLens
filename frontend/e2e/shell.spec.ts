import { expect, test } from '@playwright/test'

import { zh } from '../src/i18n/zh'
import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Shell smoke spec — the template todos 15-18 copy for their own views.
 *
 * Every assertion waits on an explicit locator or an `expect.poll`, never on a fixed
 * timeout, so the spec is deterministic rather than timing-sensitive.
 */

/**
 * Keys are pinned here because they are the testid/route contract; labels are read from
 * `zh.nav` instead of retyped, because a second hand-written copy of the copy is exactly
 * what let the 下钻 → 用量分析 rename ship with a red suite.
 */
const NAV_KEYS = ['overview', 'drilldown', 'detail', 'hosts', 'settings', 'diagnostics'] as const
const NAV = NAV_KEYS.map((key) => ({ key, label: zh.nav[key] }))

test('shell renders every navigation tab and switches views', async ({ page }) => {
  await openShell(page)

  // A seventh dictionary entry must not be able to appear without a case here.
  expect(Object.keys(zh.nav)).toEqual([...NAV_KEYS])

  await expect(page.getByRole('heading', { name: 'AgentLens' })).toBeVisible()
  for (const item of NAV) {
    await expect(page.getByTestId(`nav-${item.key}`)).toHaveText(item.label)
  }

  await expect(page.getByTestId('view-overview')).toBeVisible()
  await expect(page.getByTestId('nav-overview')).toHaveAttribute('aria-selected', 'true')

  for (const item of NAV.slice(1)) {
    await page.getByTestId(`nav-${item.key}`).click()
    await expect(page.getByTestId(`view-${item.key}`)).toBeVisible()
    await expect(page.getByTestId(`nav-${item.key}`)).toHaveAttribute('aria-selected', 'true')
  }

  await page.getByTestId('nav-overview').click()
  await expect(page.getByTestId('view-overview')).toBeVisible()
  await qaScreenshot(page, 'shell.png')
})

test('mock IPC records the settings hydration call', async ({ page }) => {
  await openShell(page)
  await expect(page.getByTestId('view-overview')).toBeVisible()

  const calls = await mockCalls(page, 'get_settings')
  expect(calls.length).toBeGreaterThan(0)
})

test('a structured IpcError renders the error state instead of a blank screen', async ({
  page,
}) => {
  await openShell(page, {
    errors: {
      get_settings: {
        code: 'database',
        message: 'archive database is locked',
        fields: { table: 'app_settings' },
      },
    },
  })

  await expect(page.getByTestId('error-state')).toBeVisible()
  await expect(page.getByTestId('error-code')).toHaveText('database')
  await expect(page.getByTestId('error-message')).toHaveText('archive database is locked')
  await expect(page.getByTestId('view-overview')).toHaveCount(0)
})

test('the page context menu is suppressed everywhere except editable fields', async ({ page }) => {
  await openShell(page)
  await expect(page.getByTestId('view-overview')).toBeVisible()

  /**
   * Records whether the real `contextmenu` event that a right-click produces was cancelled.
   * A capture-phase listener on `window` runs after the document-level guard has had its
   * turn, so `defaultPrevented` reflects the guard's decision.
   */
  const observe = () =>
    page.evaluate(() => {
      const seen: boolean[] = []
      const record = (event: Event) => seen.push(event.defaultPrevented)
      window.addEventListener('contextmenu', record)
      ;(window as unknown as Record<string, unknown>).__ctxSeen = seen
      ;(window as unknown as Record<string, unknown>).__ctxStop = () =>
        window.removeEventListener('contextmenu', record)
    })
  const cancellations = () =>
    page.evaluate(() => (window as unknown as { __ctxSeen: boolean[] }).__ctxSeen)

  await observe()

  // Page chrome and content: Chromium's "Reload"/"Inspect"/"Back" menu must never appear,
  // because a reload discards the whole React tree mid-refresh.
  await page.getByTestId('titlebar-title').click({ button: 'right' })
  await page.getByRole('heading', { name: 'AgentLens' }).click({ button: 'right' })
  await expect.poll(cancellations).toEqual([true, true])

  // An input keeps its native cut/copy/paste block: it is the only pointer-driven way to
  // paste an SSH address or a key path, so suppressing it would be a usability regression.
  await page.getByTestId('nav-hosts').click()
  await expect(page.getByTestId('view-hosts')).toBeVisible()
  const target = page.getByTestId('add-host-target')
  await expect(target).toBeVisible()
  await target.click({ button: 'right' })
  await expect.poll(cancellations).toEqual([true, true, false])
})
