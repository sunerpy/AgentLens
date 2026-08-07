import { expect, test, type Locator, type Page } from '@playwright/test'

import type { BreakdownRow, TokenValues } from '../src/generated'
import { TREND_GROUP_LIMIT } from '../src/views/overview/trendGrouping'
import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Grouped trend spec (todo 15). Component-level: real Chromium, mocked IPC.
 *
 * Numbers come from the seeded `breakdown` in `src/lib/mockIpc.ts`, whose four rows carry
 * 660,800 tokens in total: 486,550 on `(kiro-auth, claude-opus-5-max)` (two rows differing only
 * by variant), 122,350 on `(openai, gpt-5-codex)` and 51,900 on `(anthropic, claude-sonnet-5)`.
 * All four share `source: "opencode"`, so 按工具 is a single-group window — which is the case
 * that proves a grouped line reconciles exactly with the ungrouped total.
 *
 * `get_trend` in the mock scales the seeded series by the filtered slice's share of that token
 * weight, so a full partition sums back to the total up to per-bucket rounding.
 */

/** Sum of the five atomic buckets of the last seeded day (2026-01-07). */
const LAST_BUCKET_TOKENS = 58_400 + 4_050 + 310 + 29_700 + 1_450

function legendItems(page: Page): Locator {
  return page.getByTestId('trend-group-legend-item')
}

async function openOverview(page: Page, config: Parameters<typeof openShell>[1] = {}) {
  await openShell(page, config)
  await expect(page.getByTestId('view-overview')).toBeVisible()
  await expect(page.getByTestId('overview-trend')).toBeVisible()
}

async function selectGroup(page: Page, mode: 'none' | 'model' | 'agent' | 'tool') {
  await page.getByTestId(`trend-group-${mode}`).click()
  await expect(page.getByTestId(`trend-group-${mode}`)).toHaveAttribute('aria-pressed', 'true')
  await expect(page.getByTestId('overview-trend')).toHaveAttribute('data-group-mode', mode)
}

async function legendLabels(page: Page): Promise<string[]> {
  return legendItems(page).evaluateAll((nodes) =>
    nodes.map((node) => node.getAttribute('data-series') ?? ''),
  )
}

async function legendValues(page: Page): Promise<number[]> {
  return legendItems(page).evaluateAll((nodes) =>
    nodes.map((node) => Number(node.getAttribute('data-last-value') ?? '')),
  )
}

function tokens(input: number): TokenValues {
  return {
    tokInput: input,
    tokOutput: 0,
    tokReasoning: 0,
    tokCacheRead: 0,
    tokCacheWrite: 0,
    totalInput: input,
  }
}

/** `count` distinct models under one agent, weighted so the ranking is unambiguous. */
function manyModels(count: number): BreakdownRow[] {
  return Array.from({ length: count }, (_unused, index) => ({
    source: 'opencode',
    agentKey: 'build',
    agentRaw: 'build',
    providerId: 'openai',
    modelId: `model-${String(index).padStart(2, '0')}`,
    variant: null,
    tokens: tokens((count - index) * 1_000),
    cost: { actualSum: 0, estimatedSum: 0, unavailableCount: 0 },
    messageCount: 1,
    activeSessionCount: 1,
  }))
}

test('不分组 is the default and issues no extra reads', async ({ page }) => {
  await openOverview(page)

  await expect(page.getByTestId('overview-trend')).toHaveAttribute('data-group-mode', 'none')
  await expect(page.getByTestId('trend-group-none')).toHaveAttribute('aria-pressed', 'true')
  await expect(page.getByTestId('trend-group-legend')).toHaveCount(0)
  // The dimension query is disabled until a grouped mode is picked.
  expect(await mockCalls(page, 'get_breakdown')).toHaveLength(0)
  const trendCalls = await mockCalls(page, 'get_trend')
  expect(trendCalls.every((call) => call.args.filters === null)).toBe(true)
})

test('按工具 draws one line per source and reconciles with the ungrouped total', async ({
  page,
}) => {
  await openOverview(page)
  await selectGroup(page, 'tool')

  await expect(legendItems(page)).toHaveCount(1)
  expect(await legendLabels(page)).toEqual(['opencode'])
  // Every seeded row is `opencode`, so its share is exactly 1 and the line IS the total.
  expect(await legendValues(page)).toEqual([LAST_BUCKET_TOKENS])
  await expect(page.getByTestId('trend-group-topn')).toContainText('1')

  const filters = (await mockCalls(page, 'get_trend'))
    .map((call) => call.args.filters as Record<string, string | null> | null)
    .filter((value): value is Record<string, string | null> => value !== null)
  expect(filters.some((value) => value.source === 'opencode')).toBe(true)

  await qaScreenshot(page, 'trend-group-tool.png')
})

test('按模型 splits by (provider, model) and the parts sum back to the total', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'model')

  await expect(legendItems(page)).toHaveCount(3)
  expect(await legendLabels(page)).toEqual([
    'kiro-auth / claude-opus-5-max',
    'openai / gpt-5-codex',
    'anthropic / claude-sonnet-5',
  ])

  const values = await legendValues(page)
  expect(values.every((value) => value > 0)).toBe(true)
  // Descending token weight is what decides both the order and the colour assignment.
  expect([...values].sort((left, right) => right - left)).toEqual(values)
  // Per-bucket rounding across 5 atomic buckets × 3 groups bounds the drift well under 20.
  const sum = values.reduce((total, value) => total + value, 0)
  expect(Math.abs(sum - LAST_BUCKET_TOKENS)).toBeLessThan(20)
  // A full partition needs no 其他 line.
  await expect(page.locator('[data-testid="trend-group-legend-item"][data-other="true"]')).toHaveCount(0)

  await qaScreenshot(page, 'trend-group-model.png')
})

test('按 agent groups by agent_key and labels with the raw agent name', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'agent')

  await expect(legendItems(page)).toHaveCount(3)
  expect(await legendLabels(page)).toEqual([
    'Atlas - Plan Executor',
    'build',
    'Research Assistant',
  ])

  const filters = (await mockCalls(page, 'get_trend'))
    .map((call) => call.args.filters as Record<string, string | null> | null)
    .filter((value): value is Record<string, string | null> => value !== null)
  // The two Atlas rows share one agent_key, so exactly one query is issued for them.
  expect(filters.filter((value) => value.agentKey === 'atlas-plan-executor')).toHaveLength(1)

  await qaScreenshot(page, 'trend-group-agent.png')
})

test('a wide window is capped at the palette size and the tail becomes 其他', async ({ page }) => {
  const modelCount = TREND_GROUP_LIMIT + 5
  await openOverview(page, { dataset: { breakdown: manyModels(modelCount) } })
  await selectGroup(page, 'model')

  await expect(legendItems(page)).toHaveCount(TREND_GROUP_LIMIT + 1)
  const other = page.locator('[data-testid="trend-group-legend-item"][data-other="true"]')
  await expect(other).toHaveCount(1)
  await expect(other).toContainText('其他')

  // The hint has to name both halves, or the cut is invisible to the reader.
  const hint = page.getByTestId('trend-group-topn')
  await expect(hint).toContainText(String(TREND_GROUP_LIMIT))
  await expect(hint).toContainText(String(modelCount - TREND_GROUP_LIMIT))

  // 其他 is the clamped remainder, never a negative and never larger than the total.
  const values = await legendValues(page)
  const otherValue = values[values.length - 1]
  expect(otherValue).toBeGreaterThanOrEqual(0)
  expect(otherValue).toBeLessThanOrEqual(LAST_BUCKET_TOKENS)

  // The fan-out stays bounded: one dimension query plus one series query per kept group.
  const grouped = (await mockCalls(page, 'get_trend')).filter((call) => call.args.filters !== null)
  expect(grouped.length).toBeLessThanOrEqual(TREND_GROUP_LIMIT)

  await qaScreenshot(page, 'trend-group-topn.png')
})

test('a coverage gap still breaks every grouped line instead of plotting 0', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'model')

  // Coverage is a property of the window, so the bands are unchanged by grouping.
  await expect(page.locator('[data-testid="coverage-band"]')).toHaveCount(2)

  const grid = page.locator('.recharts-cartesian-grid').first()
  await grid.scrollIntoViewIfNeeded()
  const box = await grid.boundingBox()
  expect(box).not.toBeNull()
  if (box === null) return
  const band = box.width / 7
  await page.mouse.move(box.x + band * 0.5, box.y + box.height / 2)

  const tooltip = page.getByTestId('trend-group-tooltip')
  await expect(tooltip).toBeVisible()
  await expect(tooltip).toHaveAttribute('data-bucket', '2026-01-01')
  await expect(tooltip).toHaveAttribute('data-coverage', 'none')
  await expect(tooltip.getByTestId('trend-group-tooltip-gap-note')).toBeVisible()
  await expect(tooltip.getByTestId('trend-group-tooltip-row')).toHaveCount(0)
})

test('the grouped tooltip lists every series for a covered bucket', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'model')

  const grid = page.locator('.recharts-cartesian-grid').first()
  await grid.scrollIntoViewIfNeeded()
  const box = await grid.boundingBox()
  expect(box).not.toBeNull()
  if (box === null) return
  const band = box.width / 7
  await page.mouse.move(box.x + band * 6.5, box.y + box.height / 2)

  const tooltip = page.getByTestId('trend-group-tooltip')
  await expect(tooltip).toBeVisible()
  await expect(tooltip).toHaveAttribute('data-coverage', 'full')
  await expect(tooltip.getByTestId('trend-group-tooltip-row')).toHaveCount(3)
})

test('switching back to 不分组 restores the single total series', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'model')
  await expect(legendItems(page)).toHaveCount(3)

  await selectGroup(page, 'none')
  await expect(page.getByTestId('trend-group-legend')).toHaveCount(0)
  // The ungrouped rendering path is untouched: its per-bucket dots are back.
  await expect(page.locator('[data-testid="trend-dot-tokens"]')).toHaveCount(6)
})

test('grouping survives the cost metric and keeps estimated cost out of the lines', async ({
  page,
}) => {
  await openOverview(page)
  await selectGroup(page, 'model')
  await page.getByTestId('trend-metric-cost').click()

  await expect(page.getByTestId('trend-metric-cost')).toHaveAttribute('aria-pressed', 'true')
  await expect(legendItems(page)).toHaveCount(3)
  const values = await legendValues(page)
  expect(values.every((value) => Number.isFinite(value))).toBe(true)

  const grid = page.locator('.recharts-cartesian-grid').first()
  await grid.scrollIntoViewIfNeeded()
  const box = await grid.boundingBox()
  expect(box).not.toBeNull()
  if (box === null) return
  // 2026-01-04 is the mixed-cost bucket: one actual cost plus one untrustworthy record.
  await page.mouse.move(box.x + (box.width / 7) * 3.5, box.y + box.height / 2)

  const tooltip = page.getByTestId('trend-group-tooltip')
  await expect(tooltip).toBeVisible()
  await expect(tooltip).toHaveAttribute('data-bucket', '2026-01-04')
  await expect(tooltip.locator('.cost-badge-partial')).toBeVisible()

  await qaScreenshot(page, 'trend-group-cost.png')
})

test('an empty archive shows the grouped empty state rather than a blank chart', async ({
  page,
}) => {
  await openOverview(page, { dataset: { breakdown: [] } })
  await selectGroup(page, 'model')

  await expect(page.getByTestId('empty-state')).toBeVisible()
  await expect(page.getByTestId('trend-group-legend')).toHaveCount(0)
})

test('a dimension-query failure is scoped to the chart card', async ({ page }) => {
  await openOverview(page, {
    errors: {
      get_breakdown: { code: 'database', message: 'archive database is locked', fields: {} },
    },
  })

  await page.getByTestId('trend-group-model').click()
  await expect(page.getByTestId('error-code')).toHaveText('database')
  // The summary half of the page is unaffected.
  await expect(page.getByTestId('overview-summary')).toBeVisible()
})
