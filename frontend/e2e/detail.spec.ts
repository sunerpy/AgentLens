import { expect, test, type Page } from '@playwright/test'

import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Detail view spec — server-side paging is the property under test.
 *
 * The mock slices the seeded 137-row dataset by the `limit`/`offset` it actually receives, so
 * asserting BOTH the outgoing call args and the rendered row identities proves the page turn went
 * through IPC. A client-side slice would keep `offset: 0` and fail the args assertion.
 *
 * Every wait is an explicit locator or `expect.poll`; there is no `waitForTimeout` anywhere.
 */
const PAGE_SIZE = 50
const TOTAL_ROWS = 137

async function lastQueryArgs(page: Page): Promise<Record<string, unknown>> {
  const args = await page.evaluate(() => {
    const controller = (
      window as unknown as {
        __AGENTLENS_MOCK_IPC__: { lastArgs: (command: string) => Record<string, unknown> }
      }
    ).__AGENTLENS_MOCK_IPC__
    return controller.lastArgs('query_messages')
  })
  expect(args).toBeTruthy()
  return args
}

async function forceEmptyPage(page: Page): Promise<void> {
  await page.evaluate(() => {
    const controller = (
      window as unknown as {
        __AGENTLENS_MOCK_IPC__: { setResponse: (command: string, value: unknown) => void }
      }
    ).__AGENTLENS_MOCK_IPC__
    controller.setResponse('query_messages', { rows: [], totalCount: 0, limit: 50, offset: 0 })
  })
}

async function openDetail(page: Page): Promise<void> {
  await openShell(page)
  await page.getByTestId('nav-detail').click()
  await expect(page.getByTestId('view-detail')).toBeVisible()
}

test('first paint requests one server page of 50 rows', async ({ page }) => {
  await openDetail(page)
  await expect(page.getByTestId('detail-table')).toBeVisible()

  const args = await lastQueryArgs(page)
  expect(args.limit).toBe(PAGE_SIZE)
  expect(args.offset).toBe(0)

  await expect(page.getByTestId('detail-row')).toHaveCount(PAGE_SIZE)
  await expect(page.getByTestId('detail-total-count')).toHaveAttribute(
    'data-total-count',
    String(TOTAL_ROWS),
  )
})

test('total row count comes from total_count, not from the page length', async ({ page }) => {
  await openDetail(page)
  await expect(page.getByTestId('detail-row')).toHaveCount(PAGE_SIZE)

  await expect(page.getByTestId('detail-total-count')).toContainText(String(TOTAL_ROWS))
  await expect(page.getByTestId('detail-page-range')).toContainText('1')
  await expect(page.getByTestId('detail-page-range')).toContainText('50')
})

test('next page issues a new offset=50 call and renders different rows', async ({ page }) => {
  await openDetail(page)
  await expect(page.getByTestId('detail-row').first()).toHaveAttribute(
    'data-message-id',
    'msg_mock_0000',
  )
  const before = (await mockCalls(page, 'query_messages')).length

  await page.getByTestId('detail-next-page').click()

  await expect
    .poll(async () => (await lastQueryArgs(page)).offset)
    .toBe(PAGE_SIZE)
  expect((await mockCalls(page, 'query_messages')).length).toBeGreaterThan(before)

  // Row identities from the SECOND slice: the mock only returns these when it really got offset 50.
  await expect(page.locator('[data-testid="detail-row"][data-message-id="msg_mock_0050"]')).toBeVisible()
  await expect(page.locator('[data-testid="detail-row"][data-message-id="msg_mock_0099"]')).toBeVisible()
  await expect(page.locator('[data-testid="detail-row"][data-message-id="msg_mock_0000"]')).toHaveCount(0)
  await expect(page.getByTestId('detail-row')).toHaveCount(PAGE_SIZE)

  await expect(page.getByTestId('detail-page-range')).toContainText('51')
  await expect(page.getByTestId('detail-prev-page')).toBeEnabled()

  await qaScreenshot(page, 'detail-pagination.png')
})

test('the last page is bounded by total_count', async ({ page }) => {
  await openDetail(page)
  await page.getByTestId('detail-next-page').click()
  await expect(page.locator('[data-testid="detail-row"][data-message-id="msg_mock_0050"]')).toBeVisible()
  await page.getByTestId('detail-next-page').click()

  await expect
    .poll(async () => (await lastQueryArgs(page)).offset)
    .toBe(2 * PAGE_SIZE)
  await expect(page.getByTestId('detail-row')).toHaveCount(TOTAL_ROWS - 2 * PAGE_SIZE)
  await expect(page.getByTestId('detail-next-page')).toBeDisabled()
  await expect(page.getByTestId('detail-total-count')).toContainText(String(TOTAL_ROWS))
})

test('changing a filter resets the offset to 0', async ({ page }) => {
  await openDetail(page)
  await page.getByTestId('detail-next-page').click()
  await expect.poll(async () => (await lastQueryArgs(page)).offset).toBe(PAGE_SIZE)

  await page.getByTestId('detail-filter-host').selectOption('local-host-000001')

  await expect
    .poll(async () => {
      const args = await lastQueryArgs(page)
      return { offset: args.offset, hostId: (args.filters as { hostId: string | null }).hostId }
    })
    .toEqual({ offset: 0, hostId: 'local-host-000001' })
  await expect(page.locator('[data-testid="detail-row"][data-message-id="msg_mock_0000"]')).toBeVisible()
})

test('a filter matching nothing renders the empty state with total_count = 0', async ({ page }) => {
  await openDetail(page)
  await expect(page.getByTestId('detail-table')).toBeVisible()

  await forceEmptyPage(page)
  await page.getByTestId('detail-filter-model').selectOption('gpt-5-codex')

  await expect(page.getByTestId('empty-state')).toBeVisible()
  await expect(page.getByTestId('detail-table')).toHaveCount(0)
  await expect(page.getByTestId('detail-total-count')).toHaveAttribute('data-total-count', '0')
  await expect(page.getByTestId('detail-next-page')).toBeDisabled()
  await expect(page.getByTestId('detail-prev-page')).toBeDisabled()
})

test('cost source and incomplete badges are rendered per row', async ({ page }) => {
  await openDetail(page)
  await expect(page.getByTestId('detail-row')).toHaveCount(PAGE_SIZE)

  // Seeded rows cycle actual → estimated → unavailable, so all three badges exist on page 1.
  await expect(page.getByTestId('detail-cost-source')).toHaveCount(PAGE_SIZE)
  await expect(page.getByTestId('detail-cost-source').filter({ hasText: '实际' }).first()).toBeVisible()
  await expect(page.getByTestId('detail-cost-source').filter({ hasText: '估算' }).first()).toBeVisible()
  await expect(
    page.getByTestId('detail-cost-source').filter({ hasText: '成本不可用' }).first(),
  ).toBeVisible()

  // `index % 11 === 0` → rows 0, 11, 22, 33, 44 on the first page.
  await expect(page.getByTestId('detail-incomplete')).toHaveCount(5)
})

test('an IPC failure renders the shared error state instead of a blank table', async ({ page }) => {
  await openShell(page, {
    errors: {
      query_messages: {
        code: 'database',
        message: 'archive database is locked',
        fields: { table: 'usage_record' },
      },
    },
  })
  await page.getByTestId('nav-detail').click()

  await expect(page.getByTestId('error-state')).toBeVisible()
  await expect(page.getByTestId('error-code')).toHaveText('database')
  await expect(page.getByTestId('error-message')).toHaveText('archive database is locked')
  await expect(page.getByTestId('detail-table')).toHaveCount(0)
})

test('malformed rows render without breaking the layout', async ({ page }) => {
  const longModelId = `model-${'x'.repeat(180)}`
  await openShell(page, {
    responses: {
      query_messages: {
        rows: [
          {
            hostId: 'local-host-000001',
            source: 'opencode',
            messageId: 'msg_malformed_0001',
            sessionId: 'ses_malformed',
            timeCreatedUtc: 1_767_225_600_000,
            agentRaw: '',
            agentKey: 'unknown',
            providerId: 'kiro-auth',
            modelId: longModelId,
            variant: null,
            tokens: null,
            cost: null,
            isIncomplete: true,
            projectDir: '',
          },
        ],
        totalCount: 1,
        limit: 50,
        offset: 0,
      },
    },
  })
  await page.getByTestId('nav-detail').click()

  const row = page.locator('[data-testid="detail-row"][data-message-id="msg_malformed_0001"]')
  await expect(row).toBeVisible()
  await expect(row.getByTestId('detail-incomplete')).toBeVisible()
  await expect(row.getByTestId('detail-cost-source')).toHaveText('成本不可用')

  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBe(0)
})
