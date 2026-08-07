import { expect, test } from '@playwright/test'

import { zh } from '../src/i18n/zh'
import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Settings view spec (todo 19).
 *
 * Every assertion waits on an explicit locator, never on a fixed timeout.
 */
const ARCHIVE_PATH = '/tmp/agentlens-qa/agentlens/archive.db'

const SETTINGS_DATASET = {
  settings: {
    values: {
      'report.timezone': 'UTC',
      'report.weekStart': 'monday',
      'refresh.localIntervalMs': '300000',
      'refresh.remoteIntervalMs': '900000',
      'archive.path': ARCHIVE_PATH,
    },
  },
} as const

async function openSettings(page: Parameters<typeof openShell>[0], config = {}) {
  await openShell(page, { dataset: SETTINGS_DATASET, ...config })
  await page.getByTestId('nav-settings').click()
  await expect(page.getByTestId('view-settings')).toBeVisible()
}

test('the timezone picker is a constrained IANA dropdown that rejects free text', async ({
  page,
}) => {
  await openSettings(page)

  const timezone = page.getByTestId('settings-timezone')
  await expect(timezone).toBeVisible()
  await expect(timezone).toHaveValue('UTC')
  expect(await timezone.evaluate((node) => node.tagName)).toBe('SELECT')

  // A `<select>` has no text input surface at all, so an arbitrary IANA-looking string
  // cannot be entered — this is the plan's failure scenario.
  await expect(timezone.fill('Not/AZone')).rejects.toThrow()
  await expect(timezone.selectOption('Not/AZone', { timeout: 2000 })).rejects.toThrow()

  // Forcing the value from the DOM cannot smuggle the string in either: an assignment with no
  // matching option leaves `selectedIndex = -1`, so the value becomes empty rather than invalid.
  const forced = await timezone.evaluate((node) => {
    const select = node as HTMLSelectElement
    select.value = 'Not/AZone'
    return { value: select.value, selectedIndex: select.selectedIndex }
  })
  expect(forced).toEqual({ value: '', selectedIndex: -1 })

  const options = await timezone.evaluate((node) =>
    [...(node as HTMLSelectElement).options].map((option) => option.value),
  )
  expect(options.length).toBeGreaterThan(10)
  expect(options).toContain('UTC')
  expect(options).toContain('Asia/Shanghai')
  expect(options).not.toContain('Not/AZone')

  await expect(page.getByTestId('settings-week-start')).toHaveValue('monday')
})

test('a local interval below the 300s floor is clamped and explained', async ({ page }) => {
  await openSettings(page)

  const local = page.getByTestId('settings-local-interval')
  const hint = page.getByTestId('settings-local-interval-clamped')

  await expect(hint).toHaveCount(0)
  await local.fill('60')
  await local.blur()
  await expect(local).toHaveValue('300')
  await expect(hint).toBeVisible()
  await expect(hint).toHaveText('低于下限，已自动调整为 300 秒')

  for (const malformed of ['0', '-1', '']) {
    await local.fill(malformed)
    await local.blur()
    await expect(local).toHaveValue('300')
    await expect(hint).toBeVisible()
  }

  await local.fill('600')
  await local.blur()
  await expect(local).toHaveValue('600')
  await expect(hint).toHaveCount(0)
})

test('saving writes the four owned keys through set_settings and clears the dirty flag', async ({
  page,
}) => {
  await openSettings(page)

  await page.getByTestId('settings-timezone').selectOption('Asia/Shanghai')
  await page.getByTestId('settings-week-start').selectOption('sunday')
  await page.getByTestId('settings-local-interval').fill('600')
  await page.getByTestId('settings-local-interval').blur()
  await page.getByTestId('settings-remote-interval').fill('1200')
  await page.getByTestId('settings-remote-interval').blur()

  await expect(page.getByTestId('settings-dirty')).toBeVisible()
  await page.getByTestId('settings-save').click()

  await expect(page.getByTestId('settings-saved')).toBeVisible()
  await expect(page.getByTestId('settings-dirty')).toHaveCount(0)
  await expect(page.getByTestId('settings-save')).toBeDisabled()

  const calls = await mockCalls(page, 'set_settings')
  expect(calls).toHaveLength(1)
  expect(calls[0].args).toEqual({
    settings: {
      values: {
        'report.timezone': 'Asia/Shanghai',
        'report.weekStart': 'sunday',
        'refresh.localIntervalMs': '600000',
        'refresh.remoteIntervalMs': '1200000',
      },
    },
  })
})

test('a second save wins and the persisted value survives leaving the view', async ({ page }) => {
  await openSettings(page)

  await page.getByTestId('settings-timezone').selectOption('Asia/Tokyo')
  await page.getByTestId('settings-save').click()
  await expect(page.getByTestId('settings-saved')).toBeVisible()

  await page.getByTestId('settings-timezone').selectOption('Europe/Berlin')
  await page.getByTestId('settings-save').click()
  await expect(page.getByTestId('settings-saved')).toBeVisible()

  const calls = await mockCalls(page, 'set_settings')
  expect(calls).toHaveLength(2)
  expect((calls[1].args as Record<string, { values: Record<string, string> }>).settings.values)
    .toMatchObject({ 'report.timezone': 'Europe/Berlin' })

  // Remounting the view reads the persisted snapshot, not the local draft.
  await page.getByTestId('nav-overview').click()
  await expect(page.getByTestId('view-overview')).toBeVisible()
  await page.getByTestId('nav-settings').click()
  await expect(page.getByTestId('settings-timezone')).toHaveValue('Europe/Berlin')
})

test('price rows can be added, edited and deleted through prices_set', async ({ page }) => {
  await openSettings(page)

  await expect(page.getByTestId('price-row-0')).toBeVisible()
  expect(await page.getByTestId('price-provider-0').evaluate((node) => node.tagName)).toBe('SELECT')
  await expect(page.getByTestId('price-provider-0')).toHaveValue('__custom__')
  await expect(page.getByTestId('price-provider-custom-0')).toHaveValue('kiro-auth')
  await expect(page.getByTestId('price-row-2')).toHaveCount(0)

  await page.getByTestId('price-add').click()
  await page.getByTestId('price-provider-2').selectOption('anthropic')
  await page.getByTestId('price-model-2').selectOption('claude-sonnet-4-5-20250929')
  await page.getByTestId('price-inputPerMtok-2').fill('2.5')
  await page.getByTestId('price-outputPerMtok-2').fill('12')
  await page.getByTestId('price-cacheReadPerMtok-2').fill('0.25')
  await page.getByTestId('price-cacheWritePerMtok-2').fill('3.125')
  await page.getByTestId('price-inputPerMtok-0').fill('4')

  await page.getByTestId('price-save').click()
  await expect(page.getByTestId('price-saved')).toBeVisible()

  let calls = await mockCalls(page, 'prices_set')
  expect(calls).toHaveLength(1)
  let table = (calls[0].args as { prices: { schemaVersion: number; entries: unknown[] } }).prices
  expect(table.schemaVersion).toBe(1)
  expect(table.entries).toHaveLength(3)
  expect(table.entries[0]).toMatchObject({ providerId: 'kiro-auth', inputPerMtok: 4 })
  expect(table.entries[2]).toMatchObject({
    providerId: 'anthropic',
    modelId: 'claude-sonnet-4-5-20250929',
    inputPerMtok: 2.5,
    outputPerMtok: 12,
    cacheReadPerMtok: 0.25,
    cacheWritePerMtok: 3.125,
  })

  await page.getByTestId('price-delete-1').click()
  await expect(page.getByTestId('price-row-2')).toHaveCount(0)
  await page.getByTestId('price-save').click()

  await expect
    .poll(async () => (await mockCalls(page, 'prices_set')).length)
    .toBe(2)
  calls = await mockCalls(page, 'prices_set')
  table = (calls[1].args as { prices: { schemaVersion: number; entries: unknown[] } }).prices
  expect(table.entries).toHaveLength(2)
  expect(table.entries.map((entry) => (entry as { providerId: string }).providerId)).toEqual([
    'kiro-auth',
    'anthropic',
  ])
})

test('malformed price rows block the save with a readable reason', async ({ page }) => {
  await openSettings(page)

  await page.getByTestId('price-add').click()
  await expect(page.getByTestId('price-issue-blank')).toBeVisible()
  await expect(page.getByTestId('price-save')).toBeDisabled()

  await page.getByTestId('price-provider-2').selectOption('anthropic')
  await page.getByTestId('price-model-2').selectOption('claude-sonnet-4-5-20250929')
  await expect(page.getByTestId('price-issue-blank')).toHaveCount(0)

  await page.getByTestId('price-inputPerMtok-2').fill('-1')
  await expect(page.getByTestId('price-issue-number')).toBeVisible()
  await expect(page.getByTestId('price-save')).toBeDisabled()

  await page.getByTestId('price-inputPerMtok-2').fill('1')
  await page.getByTestId('price-provider-2').selectOption('__custom__')
  await page.getByTestId('price-provider-custom-2').fill('kiro-auth')
  await page.getByTestId('price-model-custom-2').fill('claude-opus-5-max')
  await expect(page.getByTestId('price-issue-duplicate')).toBeVisible()
  await expect(page.getByTestId('price-save')).toBeDisabled()

  expect(await mockCalls(page, 'prices_set')).toHaveLength(0)
})

test('provider and model dropdowns are linked and selecting a catalog model fills prices', async ({
  page,
}) => {
  await openSettings(page)

  await expect(page.getByTestId('price-catalog-version')).toContainText('2026-08-07.1')
  await page.getByTestId('price-add').click()

  const provider = page.getByTestId('price-provider-2')
  await expect(provider.locator('option')).toHaveText([
    '请选择 provider',
    'Amazon Bedrock',
    'Anthropic',
    'Google',
    'OpenAI',
    '手动输入…',
  ])
  await provider.selectOption('amazon-bedrock')

  const model = page.getByTestId('price-model-2')
  const modelOptions = await model.locator('option').allTextContents()
  expect(modelOptions).toContain('anthropic.claude-sonnet-4-5-20250929-v1:0')
  expect(modelOptions).not.toContain('gpt-5')

  await model.selectOption('anthropic.claude-sonnet-4-5-20250929-v1:0')
  await expect(page.getByTestId('price-inputPerMtok-2')).toHaveValue('3')
  await expect(page.getByTestId('price-outputPerMtok-2')).toHaveValue('15')
  await expect(page.getByTestId('price-cacheReadPerMtok-2')).toHaveValue('0.3')
  await expect(page.getByTestId('price-cacheWritePerMtok-2')).toHaveValue('3.75')
})

test('catalog gaps stay honest and can be copied into a manual override row', async ({ page }) => {
  await openSettings(page)

  const approximate = page.getByTestId('price-observed-approximate-0')
  await expect(approximate).toContainText('近似匹配')
  await expect(approximate).toContainText(
    'aws / us.anthropic.claude-sonnet-4-5-20250929-v1:0',
  )

  const unknown = page.getByTestId('price-observed-unknown-0')
  await expect(unknown).toContainText('价格未知')
  await expect(unknown).toContainText('private-provider / private-model-v7')
  await page.getByTestId('price-observed-add-unknown-0').click()

  await expect(page.getByTestId('price-provider-2')).toHaveValue('__custom__')
  await expect(page.getByTestId('price-provider-custom-2')).toHaveValue('private-provider')
  await expect(page.getByTestId('price-model-custom-2')).toHaveValue('private-model-v7')
  await page.getByTestId('price-inputPerMtok-2').fill('0.8')
  await page.getByTestId('price-outputPerMtok-2').fill('3.2')
  await page.getByTestId('price-save').click()

  const calls = await mockCalls(page, 'prices_set')
  expect(calls).toHaveLength(1)
  const table = (calls[0].args as { prices: { entries: unknown[] } }).prices
  expect(table.entries[2]).toMatchObject({
    providerId: 'private-provider',
    modelId: 'private-model-v7',
    inputPerMtok: 0.8,
    outputPerMtok: 3.2,
  })
})

test('a prices IPC failure renders the shared error state instead of an empty table', async ({
  page,
}) => {
  await openSettings(page, {
    errors: {
      prices_get: {
        code: 'pricing',
        message: 'prices.json 解析失败：missing field `input_per_mtok`',
        fields: { path: 'prices.json' },
      },
    },
  })

  const card = page.getByTestId('settings-prices')
  await expect(card.getByTestId('error-state')).toBeVisible()
  await expect(card.getByTestId('error-code')).toHaveText('pricing')
  await expect(card.getByTestId('price-table')).toHaveCount(0)
  await expect(page.getByTestId('settings-report')).toBeVisible()
})

test('a failing set_settings surfaces the structured error', async ({ page }) => {
  await openSettings(page, {
    errors: {
      set_settings: {
        code: 'database',
        message: 'archive database is locked',
        fields: { table: 'app_settings' },
      },
    },
  })

  await page.getByTestId('settings-timezone').selectOption('Asia/Tokyo')
  await page.getByTestId('settings-save').click()

  await expect(page.getByTestId('error-code').first()).toHaveText('database')
  await expect(page.getByTestId('error-message').first()).toHaveText('archive database is locked')
})

test('the archive location offers a reveal action that degrades to a notice, never a crash', async ({
  page,
  context,
}) => {
  const crashes: string[] = []
  page.on('pageerror', (error) => crashes.push(error.message))

  await context.grantPermissions(['clipboard-write'])
  await openSettings(page)

  await expect(page.getByTestId('settings-archive-path')).toHaveText(ARCHIVE_PATH)

  // The mock installs `__TAURI_INTERNALS__` but registers no opener command, so this run
  // exercises the "shell present, reveal refused" branch — the same shape a Linux box with
  // no `org.freedesktop.FileManager1` on the bus produces. It must degrade to a notice and
  // leave the view intact; an unhandled rejection here is the regression being prevented.
  // (The no-bridge branch has no reachable settings view to assert against and is covered by
  // `src/lib/revealPath.test.ts`.)
  await expect(page.getByTestId('settings-archive-open-notice')).toHaveCount(0)
  await page.getByTestId('settings-archive-open').click()
  await expect(page.getByTestId('settings-archive-open-notice')).toHaveText(
    zh.settings.archive.openFailed,
  )
  await expect(page.getByTestId('settings-archive-path')).toHaveText(ARCHIVE_PATH)

  // The wire contract: one reveal for exactly the displayed path, as an array.
  const reveals = await mockCalls(page, 'plugin:opener|reveal_item_in_dir')
  expect(reveals).toHaveLength(1)
  expect(reveals[0].args).toEqual({ paths: [ARCHIVE_PATH] })

  await page.getByTestId('settings-archive-copy').click()
  await expect(page.getByTestId('settings-archive-copied')).toBeVisible()
  expect(crashes).toEqual([])

  await qaScreenshot(page, 'settings.png')
})
