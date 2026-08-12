import { expect, test } from '@playwright/test'

import { zh } from '../src/i18n/zh'
import { mockCalls, openShell, qaLocatorScreenshot, qaScreenshot } from './harness'

/**
 * Settings view spec (todo 19).
 *
 * Every assertion waits on an explicit locator, never on a fixed timeout.
 */
const ARCHIVE_PATH = '/tmp/agentlens-qa/agentlens/archive.db'

/**
 * `refresh.localIntervalMs` 是 600000 而不是旧的 300000：下限已随后端提到 10 分钟，用一个
 * 低于下限的种子值会让这个卡片一开局就处于「保存被阻断」状态，那不是这些用例要测的东西。
 */
const SETTINGS_DATASET = {
  settings: {
    values: {
      'report.timezone': 'UTC',
      'report.weekStart': 'monday',
      'refresh.localIntervalMs': '600000',
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

/**
 * 需求 4：后端 `MIN_AUTO_REFRESH_INTERVAL_MS` **拒绝**低于 600000 毫秒的写入而不是钳制，
 * 因此界面也必须报错并阻断保存，绝不静默改写成 600 —— 用户填了 60 却被悄悄改成 600，
 * 他会一直以为应用在每分钟采集。这条用例同时证明：前端先拦下来，用户不会吃一个 IPC 错误。
 */
test('an interval below the 600s floor is refused with an explanation, never silently corrected', async ({
  page,
}) => {
  await openSettings(page)

  const local = page.getByTestId('settings-local-interval')
  const issue = page.getByTestId('settings-local-interval-issue')
  const save = page.getByTestId('settings-save')

  await expect(issue).toHaveCount(0)

  await local.fill('60')
  await expect(issue).toBeVisible()
  await expect(issue).toHaveText(zh.settings.refresh.belowFloor)
  // 值保持用户输入的样子，没有被改写。
  await expect(local).toHaveValue('60')
  await expect(local).toHaveAttribute('aria-invalid', 'true')
  await expect(save).toBeDisabled()

  // 旧的 300 秒下限已不再合法。
  await local.fill('300')
  await expect(issue).toHaveText(zh.settings.refresh.belowFloor)
  await expect(save).toBeDisabled()

  // 差一秒也拒绝。
  await local.fill('599')
  await expect(issue).toHaveText(zh.settings.refresh.belowFloor)
  await expect(save).toBeDisabled()

  for (const malformed of ['0', '-1', '']) {
    await local.fill(malformed)
    await expect(issue).toHaveText(zh.settings.refresh.malformed)
    await expect(save).toBeDisabled()
  }

  // 恰好等于下限即被接受。
  await local.fill('600')
  await expect(issue).toHaveCount(0)
  await expect(local).not.toHaveAttribute('aria-invalid', 'true')

  // 全程没有把被拒的值送出去过。
  expect(await mockCalls(page, 'set_settings')).toHaveLength(0)
})

test('saving writes the owned keys through set_settings and clears the dirty flag', async ({
  page,
}) => {
  await openSettings(page)

  await page.getByTestId('settings-timezone').selectOption('Asia/Shanghai')
  await page.getByTestId('settings-week-start').selectOption('sunday')
  await page.getByTestId('settings-local-interval').fill('600')
  await page.getByTestId('settings-remote-interval').fill('1200')

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
        'refresh.autoRefreshEnabled': 'true',
        'refresh.localIntervalMs': '600000',
        'refresh.remoteIntervalMs': '1200000',
        'update.autoInstallEnabled': 'true',
      },
    },
  })
})

/**
 * 需求 4：自动刷新开关。缺省开启（与 Rust `resolve_auto_refresh_enabled` 的「键缺失即开启」
 * 一致）；关闭后间隔输入框**禁用而不隐藏** —— 隐藏会让已配置的值从视野里消失，把一个开关
 * 显得像破坏性操作。
 */
test('the auto-refresh toggle defaults on, persists, and disables the interval fields when off', async ({
  page,
}) => {
  await openSettings(page)

  const toggle = page.getByTestId('settings-auto-refresh')
  const local = page.getByTestId('settings-local-interval')
  const remote = page.getByTestId('settings-remote-interval')

  // 种子数据里没有这个键，因此必须解读为开启。
  await expect(toggle).toBeChecked()
  await expect(page.getByTestId('settings-auto-refresh-state')).toHaveText(
    zh.settings.refresh.autoRefreshOn,
  )
  await expect(local).toBeEnabled()

  await toggle.uncheck()
  await expect(page.getByTestId('settings-auto-refresh-state')).toHaveText(
    zh.settings.refresh.autoRefreshOff,
  )
  await expect(local).toBeDisabled()
  await expect(remote).toBeDisabled()
  // 禁用不等于清空：已配置的值仍然看得见。
  await expect(local).toHaveValue('600')

  await page.getByTestId('settings-save').click()
  await expect(page.getByTestId('settings-saved')).toBeVisible()

  const calls = await mockCalls(page, 'set_settings')
  expect(calls).toHaveLength(1)
  expect(
    (calls[0].args as Record<string, { values: Record<string, string> }>).settings.values,
  ).toMatchObject({ 'refresh.autoRefreshEnabled': 'false' })

  // 离开再回来读的是持久化后的快照，开关仍然是关。
  await page.getByTestId('nav-overview').click()
  await expect(page.getByTestId('view-overview')).toBeVisible()
  await page.getByTestId('nav-settings').click()
  await expect(page.getByTestId('settings-auto-refresh')).not.toBeChecked()
})

test('automatic updates default on and a newer signed release can be installed on Windows', async ({
  page,
}) => {
  await openSettings(page, {
    responses: {
      updater_check: {
        currentVersion: '0.0.4',
        version: '0.0.5',
        date: '2026-08-12T03:00:00Z',
        body: 'signed updater release',
        autoInstallSupported: true,
      },
    },
  })

  const toggle = page.getByTestId('settings-auto-update')
  await expect(toggle).toBeChecked()
  await page.getByTestId('settings-update-check').click()
  await expect(page.getByTestId('settings-update-version')).toContainText('0.0.5')
  await expect(page.getByTestId('settings-update-install')).toBeVisible()

  await page.getByTestId('settings-update-install').click()
  await expect(page.getByTestId('settings-update-progress')).toContainText('100%')
  await qaLocatorScreenshot(page.getByTestId('settings-update'), 'settings-updater-installed.png')
  expect(await mockCalls(page, 'updater_install')).toHaveLength(1)
})

test('turning automatic updates off still checks the version but only shows advice', async ({
  page,
}) => {
  await openSettings(page, {
    responses: {
      updater_check: {
        currentVersion: '0.0.4',
        version: '0.0.5',
        date: '2026-08-12T03:00:00Z',
        body: 'signed updater release',
        autoInstallSupported: true,
      },
    },
  })

  await page.getByTestId('settings-auto-update').uncheck()
  await page.getByTestId('settings-save').click()
  await page.getByTestId('settings-update-check').click()

  await expect(page.getByTestId('settings-update-version')).toContainText('0.0.5')
  await expect(page.getByTestId('settings-update-advice')).toBeVisible()
  await expect(page.getByTestId('settings-update-install')).toHaveCount(0)
  await qaLocatorScreenshot(page.getByTestId('settings-update'), 'settings-updater-advice.png')
  expect(await mockCalls(page, 'updater_check')).toHaveLength(1)
  expect(await mockCalls(page, 'updater_install')).toHaveLength(0)
})

/** 关闭状态下也不能让一个非法间隔溜过去：开关与下限校验是两件独立的事。 */
test('a refused interval still blocks the save while auto-refresh is off', async ({ page }) => {
  await openSettings(page)

  await page.getByTestId('settings-local-interval').fill('60')
  await page.getByTestId('settings-auto-refresh').uncheck()

  await expect(page.getByTestId('settings-local-interval-issue')).toHaveText(
    zh.settings.refresh.belowFloor,
  )
  await expect(page.getByTestId('settings-save')).toBeDisabled()
  expect(await mockCalls(page, 'set_settings')).toHaveLength(0)
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
  expect(
    (calls[1].args as Record<string, { values: Record<string, string> }>).settings.values,
  ).toMatchObject({ 'report.timezone': 'Europe/Berlin' })

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
  await expect(page.getByTestId('price-provider-0')).toHaveValue('kiro-auth')
  await expect(page.getByTestId('price-model-0')).toBeEnabled()
  await expect(page.getByTestId('price-model-0')).toHaveValue('claude-opus-5-max')
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

  await expect.poll(async () => (await mockCalls(page, 'prices_set')).length).toBe(2)
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
  await page.getByTestId('price-model-2').selectOption('claude-opus-5-max')
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
    'kiro-auth',
    'aws',
    'private-provider',
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

  const inferred = page.getByTestId('price-observed-inferred-0')
  await expect(inferred).toContainText(zh.settings.prices.inferred)
  await expect(inferred).toContainText('kiro-auth / claude-opus-5-high')
  await expect(page.getByTestId('price-row-inferred-0')).toHaveText(zh.settings.prices.inferred)

  const approximate = page.getByTestId('price-observed-approximate-0')
  await expect(approximate).toContainText('近似匹配')
  await expect(approximate).toContainText('aws / us.anthropic.claude-sonnet-4-5-20250929-v1:0')

  const unknown = page.getByTestId('price-observed-unknown-0')
  await expect(unknown).toContainText('价格未知')
  await expect(unknown).toContainText('private-provider / private-model-v7')
  // 就地展开：输入框出现在被点的那一行下方，而不是直接往下方价格表追加一行。
  await page.getByTestId('price-observed-add-unknown-0').click()
  const inline = page.getByTestId('price-observed-inline-unknown-0')
  await expect(inline).toBeVisible()
  await inline.getByTestId('price-observed-inline-unknown-0-inputPerMtok').fill('0.8')
  await inline.getByTestId('price-observed-inline-unknown-0-outputPerMtok').fill('3.2')
  await qaScreenshot(page, 'settings-inline-override.png')
  await inline.getByTestId('price-observed-inline-unknown-0-save').click()
  await expect(inline).toBeHidden()

  await expect(page.getByTestId('price-provider-2')).toHaveValue('private-provider')
  await expect(page.getByTestId('price-model-2')).toBeEnabled()
  await expect(page.getByTestId('price-model-2')).toHaveValue('private-model-v7')
  await expect(page.getByTestId('price-inputPerMtok-2')).toHaveValue('0.8')
  await expect(page.getByTestId('price-outputPerMtok-2')).toHaveValue('3.2')
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

/**
 * 观测模型条数随归档增长没有上限，真实数据里单个网关下就有 5+ 个模型。这批种子造出 26 条
 * 跨三种匹配状态的模型，用来验证分页、筛选、搜索三者可叠加，且条件变化后页码回到第一页。
 */
const MANY_OBSERVED_DATASET = {
  ...SETTINGS_DATASET,
  priceCatalog: {
    schemaVersion: 1,
    catalogVersion: 'qa-observed',
    updatedAt: '2026-08-08',
    currency: 'USD',
    entries: [],
    observedModels: [
      ...Array.from({ length: 12 }, (_unused, index) => ({
        providerId: 'kiro-auth',
        modelId: `Claude-Opus-Cross-${String(index).padStart(2, '0')}`,
        usageCount: 10_000 - index,
        matchKind: 'crossProvider' as const,
        matchedPrice: {
          providerId: 'anthropic',
          modelId: 'claude-opus-5',
          inputPerMtok: 5,
          outputPerMtok: 25,
          cacheReadPerMtok: 0.5,
          cacheWritePerMtok: 6.25,
          extra: {},
        },
      })),
      ...Array.from({ length: 9 }, (_unused, index) => ({
        providerId: 'aws',
        modelId: `bedrock-normalized-${String(index).padStart(2, '0')}`,
        usageCount: 900 - index,
        matchKind: 'normalized' as const,
        matchedPrice: null,
      })),
      ...Array.from({ length: 5 }, (_unused, index) => ({
        providerId: 'private-provider',
        modelId: `private-model-${String(index).padStart(2, '0')}`,
        usageCount: 50 - index,
        matchKind: 'unknown' as const,
        matchedPrice: null,
      })),
    ],
  },
} as const

function observedRowCount(page: Parameters<typeof openShell>[0]): Promise<number> {
  return page.evaluate(
    () =>
      document.querySelectorAll(
        '[data-testid^="price-observed-inferred-"], [data-testid^="price-observed-approximate-"], [data-testid^="price-observed-unknown-"]',
      ).length,
  )
}

test('归档中的模型匹配分页展示，并支持筛选与搜索叠加', async ({ page }) => {
  await openSettings(page, { dataset: MANY_OBSERVED_DATASET })

  await expect(page.getByTestId('price-observed-total')).toHaveText(
    zh.settings.prices.observedTotal(26, 26),
  )
  await expect(page.getByTestId('price-observed-page')).toHaveText(
    zh.settings.prices.observedPage(1, 3),
  )
  expect(await observedRowCount(page)).toBe(10)
  await expect(page.getByTestId('price-observed-prev')).toBeDisabled()
  await qaScreenshot(page, 'settings-observed-paged.png')

  await page.getByTestId('price-observed-next').click()
  await expect(page.getByTestId('price-observed-page')).toHaveText(
    zh.settings.prices.observedPage(2, 3),
  )
  await page.getByTestId('price-observed-next').click()
  await expect(page.getByTestId('price-observed-page')).toHaveText(
    zh.settings.prices.observedPage(3, 3),
  )
  expect(await observedRowCount(page)).toBe(6)
  await expect(page.getByTestId('price-observed-next')).toBeDisabled()

  // 分页最常见的缺陷：筛选后页码停在旧位置，用户看到空页并以为没有数据。
  await page.getByTestId('price-observed-filter').selectOption('unknown')
  await expect(page.getByTestId('price-observed-page')).toHaveText(
    zh.settings.prices.observedPage(1, 1),
  )
  await expect(page.getByTestId('price-observed-total')).toHaveText(
    zh.settings.prices.observedTotal(5, 26),
  )
  expect(await observedRowCount(page)).toBe(5)
  await expect(page.getByTestId('price-observed-empty')).toBeHidden()

  // 搜索大小写不敏感：模型真实大小写是 Claude-Opus-Cross-XX。
  await page.getByTestId('price-observed-filter').selectOption('all')
  await page.getByTestId('price-observed-search').fill('claude-opus-cross-1')
  await expect(page.getByTestId('price-observed-total')).toHaveText(
    zh.settings.prices.observedTotal(2, 26),
  )
  expect(await observedRowCount(page)).toBe(2)
  await qaScreenshot(page, 'settings-observed-search.png')

  // 筛选 + 搜索叠加成互斥条件 → 空态，而不是一片空白。
  await page.getByTestId('price-observed-filter').selectOption('unknown')
  await expect(page.getByTestId('price-observed-empty')).toContainText(
    zh.settings.prices.observedNoMatch,
  )
  expect(await observedRowCount(page)).toBe(0)
  await qaScreenshot(page, 'settings-observed-empty.png')

  await page.getByTestId('price-observed-clear-search').click()
  await expect(page.getByTestId('price-observed-total')).toHaveText(
    zh.settings.prices.observedTotal(5, 26),
  )
})

/** 筛选与搜索都不能让「补充覆盖价」失效：那是这个列表存在的理由。 */
test('筛选并搜索之后仍能把某一条观测模型补充为覆盖价', async ({ page }) => {
  await openSettings(page, { dataset: MANY_OBSERVED_DATASET })

  await page.getByTestId('price-observed-filter').selectOption('unknown')
  await page.getByTestId('price-observed-search').fill('PRIVATE-MODEL-03')
  expect(await observedRowCount(page)).toBe(1)

  await page.getByTestId('price-observed-add-unknown-3').click()
  await page
    .getByTestId('price-observed-inline-unknown-3')
    .getByTestId('price-observed-inline-unknown-3-save')
    .click()

  // 种子里已有两条覆盖价，所以新行是第三行。
  await expect(page.getByTestId('price-provider-2')).toHaveValue('private-provider')
  await expect(page.getByTestId('price-model-2')).toHaveValue('private-model-03')
})

/**
 * 就地展开的核心约束：同一时刻只展开一个。两个未保存草稿同时存在时，用户无法判断保存动作
 * 作用在哪一个上。
 */
test('覆盖价输入就地展开，且同时只展开一个', async ({ page }) => {
  await openSettings(page, { dataset: MANY_OBSERVED_DATASET })

  const first = page.getByTestId('price-observed-inline-inferred-0')
  const second = page.getByTestId('price-observed-inline-inferred-1')

  await page.getByTestId('price-observed-add-inferred-0').click()
  await expect(first).toBeVisible()
  await expect(page.getByTestId('price-observed-inferred-0')).toHaveAttribute(
    'data-expanded',
    'true',
  )
  await expect(second).toHaveCount(0)

  // 展开另一行必须收起前一行。
  await page.getByTestId('price-observed-add-inferred-1').click()
  await expect(first).toHaveCount(0)
  await expect(second).toBeVisible()

  // Esc 收起，键盘可达。
  await second.getByTestId('price-observed-inline-inferred-1-inputPerMtok').press('Escape')
  await expect(second).toHaveCount(0)
})

/** Enter 保存：整条流程不碰鼠标也能走完。 */
test('就地展开支持 Enter 保存，并把改过的费率带进价格表', async ({ page }) => {
  await openSettings(page, { dataset: MANY_OBSERVED_DATASET })

  await page.getByTestId('price-observed-filter').selectOption('unknown')
  await page.getByTestId('price-observed-add-unknown-0').click()

  const inline = page.getByTestId('price-observed-inline-unknown-0')
  const input = inline.getByTestId('price-observed-inline-unknown-0-inputPerMtok')
  await input.fill('7.5')
  await input.press('Enter')

  await expect(inline).toHaveCount(0)
  await expect(page.getByTestId('price-provider-2')).toHaveValue('private-provider')
  await expect(page.getByTestId('price-model-2')).toHaveValue('private-model-00')
  await expect(page.getByTestId('price-inputPerMtok-2')).toHaveValue('7.5')
})

/** 非法费率必须用既有的那套提示拦住，而不是让它进到价格表里再被后端拒绝。 */
test('就地展开复用既有的费率校验与错误提示', async ({ page }) => {
  await openSettings(page, { dataset: MANY_OBSERVED_DATASET })

  await page.getByTestId('price-observed-filter').selectOption('unknown')
  await page.getByTestId('price-observed-add-unknown-0').click()

  const inline = page.getByTestId('price-observed-inline-unknown-0')
  await inline.getByTestId('price-observed-inline-unknown-0-inputPerMtok').fill('-1')

  await expect(inline.getByTestId('price-observed-inline-unknown-0-save')).toBeDisabled()
  await expect(inline.getByTestId('price-issue-number')).toHaveText(
    zh.settings.prices.invalidNumber,
  )
  await expect(inline).toBeVisible()
})

/**
 * 覆盖价的四个费率可以从已有定价填充。
 *
 * 手敲四个数字是这条路径上最大的摩擦：权威单价就在内置目录里，用户自己定过的价就在覆盖价表里。
 * 这条用例走真实浏览器验证「挑一条 → 填进去 → 再改 → 保存的是改后的值」，并确认出处可见。
 */
test('inline rates can be filled from an existing price and still edited before saving', async ({
  page,
}) => {
  await openSettings(page)

  await page.getByTestId('price-observed-add-unknown-0').click()
  const panel = page.getByTestId('price-observed-inline-unknown-0')
  await expect(panel).toBeVisible()
  const id = 'price-observed-inline-unknown-0'

  await panel.getByTestId(`${id}-fill-toggle`).click()
  const picker = panel.getByTestId(`${id}-fill-panel`)
  await expect(picker).toBeVisible()

  // 目录 5 条 + 已保存的覆盖价 2 条 = 7 条，每页 5 条。
  await expect(panel.getByTestId(`${id}-fill-total`)).toHaveText(zh.settings.prices.fillTotal(7, 7))
  await panel.getByTestId(`${id}-fill-search`).fill('OPUS-5')
  await expect(panel.getByTestId(`${id}-fill-total`)).toHaveText(zh.settings.prices.fillTotal(2, 7))
  await expect(panel.getByTestId(`${id}-fill-option-catalog-1`)).toContainText(
    'anthropic / claude-opus-5',
  )
  await expect(panel.getByTestId(`${id}-fill-option-override-0`)).toContainText(
    'kiro-auth / claude-opus-5-max',
  )
  await qaScreenshot(page, 'settings-price-fill-picker.png')

  await panel.getByTestId(`${id}-fill-apply-catalog-1`).click()
  await expect(panel.getByTestId(`${id}-inputPerMtok`)).toHaveValue('5')
  await expect(panel.getByTestId(`${id}-outputPerMtok`)).toHaveValue('25')
  await expect(panel.getByTestId(`${id}-cacheReadPerMtok`)).toHaveValue('0.5')
  await expect(panel.getByTestId(`${id}-cacheWritePerMtok`)).toHaveValue('6.25')
  await expect(panel.getByTestId(`${id}-fill-origin`)).toHaveText(
    zh.settings.prices.fillOrigin(zh.settings.prices.fillKindCatalog, 'anthropic', 'claude-opus-5'),
  )

  await panel.getByTestId(`${id}-inputPerMtok`).fill('4.5')
  await expect(panel.getByTestId(`${id}-fill-adjusted`)).toHaveText(zh.settings.prices.fillAdjusted)
  await qaScreenshot(page, 'settings-price-fill-applied.png')

  await panel.getByTestId(`${id}-save`).click()
  await expect(panel).toBeHidden()
  await expect(page.getByTestId('price-inputPerMtok-2')).toHaveValue('4.5')
  await expect(page.getByTestId('price-outputPerMtok-2')).toHaveValue('25')

  await page.getByTestId('price-save').click()
  const calls = await mockCalls(page, 'prices_set')
  expect(calls).toHaveLength(1)
  const table = (calls[0].args as { prices: { entries: unknown[] } }).prices
  expect(table.entries[2]).toMatchObject({
    providerId: 'private-provider',
    modelId: 'private-model-v7',
    inputPerMtok: 4.5,
    outputPerMtok: 25,
    cacheReadPerMtok: 0.5,
    cacheWritePerMtok: 6.25,
  })
})
