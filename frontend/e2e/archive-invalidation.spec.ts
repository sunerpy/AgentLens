import { expect, test, type Page } from '@playwright/test'

import type { SourceStatus } from '../src/generated'
import { mockCalls, mockEmitEvent, mockSetDataset, openShell } from './harness'

/**
 * Regression guard for F3 DEFECT-2 — a refresh round that commits rows must invalidate the
 * archive query family so the dashboard stops serving its pre-collection cache.
 *
 * The defect survived every earlier gate because mock IPC returned a static dataset: "the
 * numbers change after a refresh" was simply unobservable. These specs make it observable by
 * swapping the dataset mid-session, so a passing run proves a real refetch happened rather
 * than that a call was merely issued.
 *
 * Event name below is the wire literal shared by `src/lib/archiveQueries.ts` and
 * `src-tauri/src/state.rs`; a rename on either side makes `mockEmitEvent` notify nobody and
 * these specs go red.
 */
const ARCHIVE_COMMITTED_EVENT = 'agentlens://archive-committed'
const REFRESH_COMPLETED_EVENT = 'agentlens://refresh-completed'

const SEEDED_MESSAGE_COUNT = '109'
const COLLECTED_MESSAGE_COUNT = '155,498'

/** The aggregate a first collection round would leave behind, at real magnitudes. */
const COLLECTED_SUMMARY = {
  tokens: {
    tokInput: 11_795_202_505,
    tokOutput: 7_674_858,
    tokReasoning: 4_161_739,
    tokCacheRead: 1_136_161_924,
    tokCacheWrite: 492_080_402,
    totalInput: 13_423_444_831,
  },
  cost: { actualSum: 83.52, estimatedSum: 0, unavailableCount: 155_348 },
  messageCount: 155_498,
  activeSessionCount: 4_094,
}

async function openOverview(page: Page): Promise<void> {
  await openShell(page)
  await expect(page.getByTestId('overview-summary')).toBeVisible()
  await expect(page.getByTestId('summary-message-count')).toHaveText(SEEDED_MESSAGE_COUNT)
}

/**
 * `listen()` registers asynchronously, so the first emit can land before the subscriber does.
 * Polling on the delivered-listener count is the deterministic wait for it.
 */
async function emitCommitted(page: Page): Promise<void> {
  await expect
    .poll(() => mockEmitEvent(page, ARCHIVE_COMMITTED_EVENT, 'local-host-000001'))
    .toBeGreaterThan(0)
}

test('a committed refresh round makes the overview refetch its aggregates', async ({ page }) => {
  await openOverview(page)
  const summaryCallsBefore = (await mockCalls(page, 'get_summary')).length
  const trendCallsBefore = (await mockCalls(page, 'get_trend')).length

  await mockSetDataset(page, { summary: COLLECTED_SUMMARY })
  // Nothing has invalidated yet, so the pre-collection cache is still what the user sees.
  // This is the exact state the defect left the dashboard stuck in forever.
  await expect(page.getByTestId('summary-message-count')).toHaveText(SEEDED_MESSAGE_COUNT)

  await emitCommitted(page)

  await expect(page.getByTestId('summary-message-count')).toHaveText(COLLECTED_MESSAGE_COUNT)
  expect((await mockCalls(page, 'get_summary')).length).toBeGreaterThan(summaryCallsBefore)
  expect((await mockCalls(page, 'get_trend')).length).toBeGreaterThan(trendCallsBefore)
})

test('the invalidation reaches a view that was not mounted when the round committed', async ({
  page,
}) => {
  await openOverview(page)

  await page.getByTestId('nav-hosts').click()
  await expect(page.getByTestId('view-hosts')).toBeVisible()

  await mockSetDataset(page, { summary: COLLECTED_SUMMARY })
  await emitCommitted(page)

  // The overview is unmounted here, so this only passes because the family was marked stale
  // rather than refetched in place — the "leave 总览 and come back" recovery path.
  await page.getByTestId('nav-overview').click()
  await expect(page.getByTestId('summary-message-count')).toHaveText(COLLECTED_MESSAGE_COUNT)
})

test('立即刷新 invalidates the aggregates as well as the host rows', async ({ page }) => {
  await openOverview(page)
  await page.getByTestId('nav-hosts').click()
  await expect(page.getByTestId('view-hosts')).toBeVisible()

  await mockSetDataset(page, { summary: COLLECTED_SUMMARY })
  await page.getByTestId('host-refresh-local-host-000001').click()
  await expect(page.getByTestId('host-refresh-outcome-local-host-000001')).toBeVisible()

  await page.getByTestId('nav-overview').click()
  await expect(page.getByTestId('summary-message-count')).toHaveText(COLLECTED_MESSAGE_COUNT)
})

test('a completed refresh with no archive changes refetches the permanently fresh host status', async ({
  page,
}) => {
  await openShell(page)
  await page.getByTestId('nav-hosts').click()
  await expect(page.getByTestId('view-hosts')).toBeVisible()
  const statusCallsBefore = (await mockCalls(page, 'get_refresh_status')).length
  const completedAt = Date.UTC(2026, 0, 8)
  const completedStatus: SourceStatus = {
    hostId: 'local-host-000001',
    displayName: 'workstation',
    kind: 'local',
    state: { state: 'idle' },
    trigger: 'auto',
    lastError: null,
    lastSuccessUtc: completedAt,
    lastCompletedUtc: completedAt,
    lastDurationMs: 811,
    intervalMs: 300_000,
    nextDueUtc: completedAt + 300_000,
    interrupted: false,
    cursorTimeUpdated: completedAt,
  }

  await mockSetDataset(page, { refreshStatus: [completedStatus] })
  await expect(page.getByTestId('host-last-success-local-host-000001')).toHaveText(
    '2026-01-07 00:00:00',
  )
  await expect
    .poll(() => mockEmitEvent(page, REFRESH_COMPLETED_EVENT, 'local-host-000001'))
    .toBeGreaterThan(0)

  await expect(page.getByTestId('host-last-success-local-host-000001')).toHaveText(
    '2026-01-08 00:00:00',
  )
  expect((await mockCalls(page, 'get_refresh_status')).length).toBeGreaterThan(statusCallsBefore)
})

test('the detail page rejoins the archive family and refetches on a commit', async ({ page }) => {
  await openShell(page)
  await page.getByTestId('nav-detail').click()
  await expect(page.getByTestId('view-detail')).toBeVisible()
  const before = (await mockCalls(page, 'query_messages')).length

  await emitCommitted(page)

  await expect
    .poll(async () => (await mockCalls(page, 'query_messages')).length)
    .toBeGreaterThan(before)
})
