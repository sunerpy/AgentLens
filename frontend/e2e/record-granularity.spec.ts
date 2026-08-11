import { expect, test, type Page } from '@playwright/test'

import { zh } from '../src/i18n/zh'
import { SESSION_GRANULARITY_DATASET } from '../src/lib/mockIpc'
import { openShell, qaScreenshot } from './harness'

/**
 * Record-granularity presentation (`Summary.sessionRecordCount` and friends).
 *
 * Two render paths have to be covered, and only one of them exists in the seeded dataset:
 * `sessionRecordCount` is 0 for any installation whose `enabled_sources` is the default
 * `'opencode'` alone, and non-zero only once a session-only source is enabled. The zero path
 * is asserted against the plain seed; the non-zero path is driven by
 * {@link SESSION_GRANULARITY_DATASET}, whose `hermes` rows carry `messageCount 0` beside
 * non-zero tokens — the shape the granularity copy exists to explain.
 *
 * The trend buckets carrying session records are 2026-01-04 (3) and 2026-01-05 (4); every
 * other bucket keeps 0, which is what makes the "only when non-zero" tooltip rule assertable.
 */
const BUCKET_COUNT = 7
const SESSION_BUCKET_INDEX = 3
const MESSAGE_ONLY_BUCKET_INDEX = 5

async function openOverview(page: Page, dataset?: typeof SESSION_GRANULARITY_DATASET) {
  await openShell(page, dataset === undefined ? {} : { dataset })
  await expect(page.getByTestId('view-overview')).toBeVisible()
  await expect(page.getByTestId('overview-summary')).toBeVisible()
}

async function openDrilldown(page: Page, dataset?: typeof SESSION_GRANULARITY_DATASET) {
  await openShell(page, dataset === undefined ? {} : { dataset })
  await page.getByTestId('nav-drilldown').click()
  await expect(page.getByTestId('view-drilldown')).toBeVisible()
}

/** Hover the horizontal centre of category `index`, measured from the rendered plot area. */
async function hoverBucket(page: Page, index: number): Promise<void> {
  const grid = page.locator('.recharts-cartesian-grid').first()
  await expect(grid).toBeVisible()
  await grid.scrollIntoViewIfNeeded()
  const box = await grid.boundingBox()
  expect(box).not.toBeNull()
  if (box === null) return
  const band = box.width / BUCKET_COUNT
  await page.mouse.move(box.x + band * (index + 0.5), box.y + box.height / 2)
}

test('the volume card stays two metrics and shows no granularity note when there are no session records', async ({
  page,
}) => {
  await openOverview(page)

  await expect(page.getByTestId('summary-message-count')).toHaveText('109')
  await expect(page.getByTestId('summary-active-session-count')).toHaveText('14')

  // No dry "0": neither the metric nor the explainer occupies space when it has nothing to say.
  await expect(page.getByTestId('summary-session-record-count')).toHaveCount(0)
  await expect(page.getByTestId('summary-granularity-note')).toHaveCount(0)
  await expect(page.getByTestId('overview-summary')).not.toContainText(zh.common.sessionRecordCount)

  await qaScreenshot(page, 'granularity-summary-zero.png')
})

test('both granularities appear side by side, with a visible explanation, once session records exist', async ({
  page,
}) => {
  await openOverview(page, SESSION_GRANULARITY_DATASET)

  await expect(page.getByTestId('summary-message-count')).toHaveText('109')
  await expect(page.getByTestId('summary-session-record-count')).toHaveText('7')
  // `count(DISTINCT session_id)` spans both granularities, so it is not messageCount-derived.
  await expect(page.getByTestId('summary-active-session-count')).toHaveText('21')

  // The explanation is page copy, not a tooltip: it must be readable without hovering.
  const note = page.getByTestId('summary-granularity-note')
  await expect(note).toBeVisible()
  await expect(note).toContainText(zh.overview.summary.granularityNoteTitle)
  await expect(note).toContainText('109')
  await expect(note).toContainText('7')
  await expect(note).toContainText(zh.common.messageCount)

  // Tokens cover both granularities, so they exceed the message-level-only seed's totals.
  await expect(page.getByTestId('summary-token-input')).toHaveAttribute('title', '450,150')
  // 来源自带的金额在折叠披露里（默认收起），所以核对它之前要先展开。
  await page.getByTestId('summary-cost-source-toggle').click()
  await expect(page.getByTestId('summary-cost-actual')).toHaveText('$0.0611')
  // Cost is still explicitly partial: the unavailable count survives the overlay.
  await expect(page.getByTestId('summary-cost-unavailable')).toHaveText('1')
  await expect(page.getByTestId('summary-cost-card')).toContainText(zh.common.cost.partial)

  await qaScreenshot(page, 'granularity-summary-nonzero.png')
})

test('the trend tooltip lists session records for the buckets that have them and omits the row elsewhere', async ({
  page,
}) => {
  await openOverview(page, SESSION_GRANULARITY_DATASET)

  await hoverBucket(page, SESSION_BUCKET_INDEX)
  const tooltip = page.getByTestId('trend-tooltip')
  await expect(tooltip).toBeVisible()
  await expect(tooltip).toHaveAttribute('data-bucket', '2026-01-04')
  const sessionRow = page.getByTestId('trend-tooltip-session-records')
  await expect(sessionRow).toBeVisible()
  await expect(sessionRow).toContainText(zh.common.sessionRecordCount)
  await expect(sessionRow).toContainText('3')
  await expect(tooltip).toContainText(zh.common.messageCount)

  await qaScreenshot(page, 'granularity-trend-tooltip.png')

  await hoverBucket(page, MESSAGE_ONLY_BUCKET_INDEX)
  await expect(tooltip).toHaveAttribute('data-bucket', '2026-01-06')
  // A bucket with 0 session records keeps the message-count row and drops this one entirely.
  await expect(page.getByTestId('trend-tooltip-session-records')).toHaveCount(0)
  await expect(tooltip).toContainText(zh.common.messageCount)
})

test('the drilldown table grows a session-record column only when the range holds such records', async ({
  page,
}) => {
  await openDrilldown(page)

  await expect(page.getByTestId('drilldown-level-source')).toBeVisible()
  await expect(page.getByTestId('cell-session-records')).toHaveCount(0)
  await expect(page.getByTestId('drilldown-session-record-note')).toHaveCount(0)
  await expect(page.getByTestId('drilldown-total-session-records')).toHaveCount(0)
})

test('a session-only source renders zero messages beside non-zero session records and tokens', async ({
  page,
}) => {
  await openDrilldown(page, SESSION_GRANULARITY_DATASET)

  const hermes = page.locator('[data-testid="drilldown-source-row"][data-source="hermes"]')
  await expect(hermes).toBeVisible()
  // The row shape the copy has to explain: no messages, yet real tokens and real money.
  await expect(hermes.getByTestId('cell-messages')).toHaveText('0')
  await expect(hermes.getByTestId('cell-session-records')).toHaveText('7')
  await expect(hermes.getByTestId('cell-input')).toHaveText('64,000')
  await expect(hermes.getByTestId('cell-cost-actual')).toContainText('$0.0127')

  const opencode = page.locator('[data-testid="drilldown-source-row"][data-source="opencode"]')
  await expect(opencode.getByTestId('cell-messages')).toHaveText('109')
  await expect(opencode.getByTestId('cell-session-records')).toHaveText('0')

  // The column exists at every level, not just the one that holds the session rows.
  await expect(page.getByTestId('drilldown-level-agent')).toBeVisible()
  await expect(page.getByTestId('drilldown-level-model')).toBeVisible()
  const note = page.getByTestId('drilldown-session-record-note')
  await expect(note).toBeVisible()
  await expect(note).toContainText(zh.common.messageCount)
  await expect(page.getByTestId('drilldown-total-session-records')).toContainText('7')

  await qaScreenshot(page, 'granularity-drilldown.png')
})
