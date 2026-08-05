import { expect, test } from '@playwright/test'

import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Shell smoke spec — the template todos 15-18 copy for their own views.
 *
 * Every assertion waits on an explicit locator or an `expect.poll`, never on a fixed
 * timeout, so the spec is deterministic rather than timing-sensitive.
 */
const NAV = [
  { key: 'overview', label: '总览' },
  { key: 'drilldown', label: '下钻' },
  { key: 'detail', label: '明细' },
  { key: 'hosts', label: '主机' },
  { key: 'settings', label: '设置' },
] as const

test('shell renders all five navigation tabs and switches views', async ({ page }) => {
  await openShell(page)

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
