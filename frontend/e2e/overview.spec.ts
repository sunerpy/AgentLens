import { expect, test, type Locator, type Page } from '@playwright/test'

import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Overview view (todo 15). Component-level QA against `vite dev` with mocked IPC.
 *
 * Every wait is on an explicit locator or `expect.poll`; there is no `waitForTimeout`, so
 * the spec is deterministic rather than timing-sensitive.
 *
 * Literals below are the seeded dataset in `src/lib/mockIpc.ts` (report timezone UTC):
 * the summary is the exact aggregate of the covered buckets, and the seven trend buckets
 * 2026-01-01..07 include one `none` bucket, one `partial` bucket, one covered-but-zero
 * bucket, one mixed-cost bucket and one estimate-only bucket.
 */
const GAP_BUCKET = '2026-01-01'
const PARTIAL_BUCKET = '2026-01-02'
const ZERO_BUCKET = '2026-01-03'
const MIXED_COST_BUCKET = '2026-01-04'
const BUCKET_COUNT = 7
const GAP_INDEX = 0
const MIXED_COST_INDEX = 3

async function openOverview(page: Page): Promise<void> {
  await openShell(page)
  await expect(page.getByTestId('view-overview')).toBeVisible()
  await expect(page.getByTestId('overview-summary')).toBeVisible()
  await expect(page.getByTestId('overview-trend')).toBeVisible()
}

/**
 * Hover the horizontal centre of category `index`.
 *
 * The x position is derived from the rendered plot area (`.recharts-cartesian-grid`) rather
 * than from a hard-coded pixel, and `barCategoryGap={0}` makes every category band exactly
 * `plotWidth / BUCKET_COUNT` wide, so the centre is computed, not guessed.
 */
async function hoverBucket(page: Page, index: number): Promise<void> {
  const grid = page.locator('.recharts-cartesian-grid').first()
  await expect(grid).toBeVisible()
  // `mouse.move` takes viewport coordinates and does not scroll; the chart sits below the
  // fold on a 720px-tall viewport, so it must be scrolled in before the box is measured.
  await grid.scrollIntoViewIfNeeded()
  const box = await grid.boundingBox()
  expect(box).not.toBeNull()
  if (box === null) return
  const band = box.width / BUCKET_COUNT
  await page.mouse.move(box.x + band * (index + 0.5), box.y + box.height / 2)
}

function dots(page: Page): Locator {
  return page.locator('[data-testid="trend-dot-tokens"]')
}

test('summary cards render the seeded aggregate literals', async ({ page }) => {
  await openOverview(page)

  // The four token metrics render compactly (real archives reach 11 digits, which overlaps
  // the neighbouring metric at this cell width) and carry the exact grouped figure in `title`,
  // so precision stays reachable rather than lost.
  await expect(page.getByTestId('summary-token-input')).toHaveText('386.2K')
  await expect(page.getByTestId('summary-token-input')).toHaveAttribute('title', '386,150')
  await expect(page.getByTestId('summary-token-output')).toHaveText('29.6K')
  await expect(page.getByTestId('summary-token-output')).toHaveAttribute('title', '29,550')
  await expect(page.getByTestId('summary-token-reasoning')).toHaveText('2.2K')
  await expect(page.getByTestId('summary-token-reasoning')).toHaveAttribute('title', '2,150')
  // Display grouping: 缓存 = tokCacheRead 231,200 + tokCacheWrite 11,750.
  await expect(page.getByTestId('summary-token-cache')).toHaveText('243K')
  await expect(page.getByTestId('summary-token-cache')).toHaveAttribute('title', '242,950')
  // The footnote keeps its atomic values at full precision, unaffected by the display format.
  await expect(page.getByTestId('summary-token-cache-read')).toHaveText('231,200')
  await expect(page.getByTestId('summary-token-cache-write')).toHaveText('11,750')
  await expect(page.getByTestId('summary-token-total-input')).toHaveText('629,100')

  await expect(page.getByTestId('summary-cost-actual')).toHaveText('$0.0484')
  await expect(page.getByTestId('summary-cost-estimated')).toHaveText('$0.0075')
  await expect(page.getByTestId('summary-cost-unavailable')).toHaveText('1')

  await expect(page.getByTestId('summary-message-count')).toHaveText('109')
  await expect(page.getByTestId('summary-active-session-count')).toHaveText('14')
})

test('the 部分缺失 badge marks the mixed-cost day and never merges cost buckets', async ({
  page,
}) => {
  await openOverview(page)

  await expect(page.locator('[data-testid="summary-cost-card"] .cost-badge-partial')).toBeVisible()
  await expect(page.locator('[data-testid="trend-notes"] .cost-badge-partial')).toBeVisible()

  await page.getByTestId('trend-metric-cost').click()
  await expect(page.getByTestId('trend-metric-cost')).toHaveAttribute('aria-pressed', 'true')

  await hoverBucket(page, MIXED_COST_INDEX)
  const tooltip = page.getByTestId('trend-tooltip')
  await expect(tooltip).toBeVisible()
  await expect(tooltip).toHaveAttribute('data-bucket', MIXED_COST_BUCKET)
  await expect(tooltip.locator('.cost-badge-partial')).toBeVisible()
  await expect(tooltip).toContainText('部分缺失')
  // actual 0.0102 and estimated 0.0000 stay separate; nothing shows their sum.
  await expect(tooltip).toContainText('$0.0102')
  await expect(tooltip).toContainText('$0.0000')
  await expect(tooltip).not.toContainText('$0.0102$')

  await qaScreenshot(page, 'cost-mixed.png')
})

test('a coverage gap breaks the line while a covered zero day plots 0', async ({ page }) => {
  await openOverview(page)

  // Six dots, not seven: the uncovered bucket contributes no point at all.
  await expect(dots(page)).toHaveCount(BUCKET_COUNT - 1)
  await expect(page.locator(`[data-testid="trend-dot-tokens"][data-bucket="${GAP_BUCKET}"]`)).toHaveCount(0)

  // The conflation-catching assertion: covered-but-idle is a real 0, not a break.
  const zeroDot = page.locator(`[data-testid="trend-dot-tokens"][data-bucket="${ZERO_BUCKET}"]`)
  await expect(zeroDot).toHaveCount(1)
  await expect(zeroDot).toHaveAttribute('data-value', '0')
  await expect(zeroDot).toHaveAttribute('data-coverage', 'full')

  // Exactly two non-full buckets get a band, styled per coverage state.
  const bands = page.locator('[data-testid="coverage-band"]')
  await expect(bands).toHaveCount(2)
  const gapBand = page.locator(`[data-testid="coverage-band"][data-bucket="${GAP_BUCKET}"]`)
  await expect(gapBand).toHaveAttribute('data-coverage', 'none')
  await expect(gapBand).toHaveAttribute('stroke-dasharray', '3 3')
  const partialBand = page.locator(`[data-testid="coverage-band"][data-bucket="${PARTIAL_BUCKET}"]`)
  await expect(partialBand).toHaveAttribute('data-coverage', 'partial')
  await expect(partialBand).toHaveAttribute('opacity', '0.65')

  // The partial bucket still plots its known aggregate, with a distinct dot outline.
  const partialDot = page.locator(
    `[data-testid="trend-dot-tokens"][data-bucket="${PARTIAL_BUCKET}"]`,
  )
  await expect(partialDot).toHaveAttribute('data-value', '57000')
  await expect(partialDot).toHaveAttribute('stroke-dasharray', '2 2')

  await hoverBucket(page, GAP_INDEX)
  const tooltip = page.getByTestId('trend-tooltip')
  await expect(tooltip).toBeVisible()
  await expect(tooltip).toHaveAttribute('data-bucket', GAP_BUCKET)
  await expect(tooltip).toHaveAttribute('data-coverage', 'none')
  await expect(tooltip.getByTestId('trend-tooltip-coverage')).toHaveText('无数据覆盖')
  await expect(tooltip.getByTestId('trend-tooltip-gap-note')).toBeVisible()

  await qaScreenshot(page, 'coverage-gap.png')
})

test('granularity adapts to the range span until the user pins it', async ({ page }) => {
  await openOverview(page)

  await expect(page.getByTestId('granularity-auto')).toHaveAttribute('aria-pressed', 'true')
  await expect(page.getByTestId('granularity-effective')).toHaveText('天')

  await page.getByTestId('range-preset-today').click()
  await expect(page.getByTestId('granularity-effective')).toHaveText('小时')
  await expect.poll(async () => (await mockCalls(page, 'get_trend')).at(-1)?.args.granularity).toBe(
    'hour',
  )

  await page.getByTestId('granularity-day').click()
  await expect(page.getByTestId('granularity-auto')).toHaveAttribute('aria-pressed', 'false')
  await page.getByTestId('range-preset-last30Days').click()
  await expect(page.getByTestId('granularity-effective')).toHaveText('天')

  await page.getByTestId('granularity-month').click()
  await page.getByTestId('range-preset-today').click()
  // Pinned granularity survives a preset change that would otherwise derive `hour`.
  await expect(page.getByTestId('granularity-effective')).toHaveText('月')
  await expect.poll(async () => (await mockCalls(page, 'get_trend')).at(-1)?.args.granularity).toBe(
    'month',
  )
})

test('switching a preset refetches with the new half-open window', async ({ page }) => {
  await openOverview(page)

  const before = await mockCalls(page, 'get_trend')
  expect(before.length).toBeGreaterThan(0)
  const initialWindow = await page.getByTestId('range-window').textContent()

  await page.getByTestId('range-preset-last30Days').click()
  await expect(page.getByTestId('range-window')).not.toHaveText(initialWindow ?? '')

  await expect
    .poll(async () => (await mockCalls(page, 'get_trend')).length)
    .toBeGreaterThan(before.length)
  await expect
    .poll(async () => (await mockCalls(page, 'get_summary')).length)
    .toBeGreaterThan(0)

  const latest = (await mockCalls(page, 'get_trend')).at(-1)
  const range = latest?.args.range as { startDate: string; endDateExclusive: string } | undefined
  expect(range).toBeDefined()
  if (range === undefined) return
  expect(range.startDate < range.endDateExclusive).toBe(true)
})

test('the custom range picker dispatches a half-open window', async ({ page }) => {
  await openOverview(page)

  await page.getByTestId('range-preset-custom').click()
  await expect(page.getByTestId('range-calendar')).toBeVisible()

  await expect.poll(async () => page.getByTestId('calendar-month').textContent()).not.toBeNull()
  const month = (await page.getByTestId('calendar-month').textContent()) ?? ''
  await page.getByTestId(`calendar-day-${month}-10`).click()
  await page.getByTestId(`calendar-day-${month}-12`).click()
  await page.getByTestId('calendar-apply').click()

  // The user picks an inclusive end (12); the stored window is half-open, so end is 13.
  await expect(page.getByTestId('range-window')).toHaveText(`[${month}-10, ${month}-13)`)
  await expect(page.getByTestId('range-preset-custom')).toHaveAttribute('aria-pressed', 'true')
  await expect
    .poll(async () => (await mockCalls(page, 'get_trend')).at(-1)?.args.granularity)
    .toBe('day')
})

test('an IPC failure renders the shared error state instead of a white screen', async ({
  page,
}) => {
  await openShell(page, {
    errors: {
      get_summary: {
        code: 'database',
        message: 'archive database is locked',
        fields: { table: 'usage_record' },
      },
    },
  })

  await expect(page.getByTestId('view-overview')).toBeVisible()
  await expect(page.getByTestId('error-state')).toBeVisible()
  await expect(page.getByTestId('error-code')).toHaveText('database')
  await expect(page.getByTestId('error-message')).toHaveText('archive database is locked')
  await expect(page.getByTestId('overview-summary')).toHaveCount(0)
  // The trend half of the page keeps working; the failure is scoped, not a blank window.
  await expect(page.getByTestId('overview-trend')).toBeVisible()
})

test('a trend IPC failure is scoped to the chart card', async ({ page }) => {
  await openShell(page, {
    errors: {
      get_trend: { code: 'invalidRange', message: 'end precedes start', fields: {} },
    },
  })

  await expect(page.getByTestId('overview-summary')).toBeVisible()
  await expect(page.getByTestId('error-code')).toHaveText('invalidRange')
  await expect(page.getByTestId('overview-trend')).toHaveCount(0)
})

test('an empty series renders the empty state rather than a broken chart', async ({ page }) => {
  await openShell(page, { dataset: { trend: [] } })

  await expect(page.getByTestId('overview-summary')).toBeVisible()
  await expect(page.getByTestId('empty-state')).toBeVisible()
  await expect(page.locator('[data-testid="trend-dot-tokens"]')).toHaveCount(0)
})

test('an all-uncovered series draws only bands and says so', async ({ page }) => {
  const allGap = [0, 1, 2].map((day) => ({
    bucket: {
      startUtcMs: Date.UTC(2026, 0, 1) + day * 86_400_000,
      endUtcMs: Date.UTC(2026, 0, 1) + (day + 1) * 86_400_000,
      label: `2026-01-0${day + 1}`,
    },
    coverage: 'none' as const,
    tokens: null,
    cost: null,
    messageCount: null,
  }))

  await openShell(page, { dataset: { trend: allGap } })

  await expect(page.getByTestId('overview-trend')).toBeVisible()
  await expect(page.getByTestId('trend-all-gap')).toBeVisible()
  await expect(page.locator('[data-testid="coverage-band"]')).toHaveCount(3)
  await expect(page.locator('[data-testid="trend-dot-tokens"]')).toHaveCount(0)
  await expect(page.getByTestId('error-state')).toHaveCount(0)
})

test('an all-zero summary renders zeros, never an empty state', async ({ page }) => {
  await openShell(page, {
    dataset: {
      summary: {
        tokens: {
          tokInput: 0,
          tokOutput: 0,
          tokReasoning: 0,
          tokCacheRead: 0,
          tokCacheWrite: 0,
          totalInput: 0,
        },
        cost: { actualSum: 0, estimatedSum: 0, unavailableCount: 0 },
        messageCount: 0,
        activeSessionCount: 0,
      },
    },
  })

  await expect(page.getByTestId('summary-token-input')).toHaveText('0')
  await expect(page.getByTestId('summary-cost-actual')).toHaveText('$0.0000')
  await expect(page.getByTestId('summary-cost-unavailable')).toHaveText('0')
  await expect(
    page.locator('[data-testid="summary-cost-card"] .cost-badge-partial'),
  ).toHaveCount(0)
})
