import { expect, test, type Locator, type Page } from '@playwright/test'

import type { SeriesPoint, SeriesQueryResult, TokenValues } from '../src/generated'
import { mockDataset } from '../src/lib/mockIpc'
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
 * `get_trend` 一次返回全部预聚合趋势，切换按钮只筛选本地结果。
 */

/** Sum of the five atomic token buckets across the complete seeded report window. */
const WINDOW_TOTAL_TOKENS = 660_800

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
    nodes.map((node) => Number(node.getAttribute('data-total-value') ?? '')),
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

function pointTokens(point: SeriesPoint): number {
  if (point.tokens === null) return 0
  return (
    point.tokens.tokInput +
    point.tokens.tokOutput +
    point.tokens.tokReasoning +
    point.tokens.tokCacheRead +
    point.tokens.tokCacheWrite
  )
}

function partition(value: number, weights: readonly number[]): number[] {
  const weightSum = weights.reduce((sum, weight) => sum + weight, 0)
  let assigned = 0
  return weights.map((weight, index) => {
    const part =
      index === weights.length - 1 ? value - assigned : Math.floor((value * weight) / weightSum)
    assigned += part
    return part
  })
}

/** `count` distinct model series, weighted so ranking is unambiguous and every bucket reconciles. */
function manyModelTrend(count: number): SeriesQueryResult {
  const total = mockDataset().trend.total
  const weights = Array.from({ length: count }, (_unused, index) => count - index)
  const groups = weights.map((_weight, groupIndex) => ({
    dimension: 'model' as const,
    id: `openai\u0000model-${String(groupIndex).padStart(2, '0')}`,
    label: `openai / model-${String(groupIndex).padStart(2, '0')}`,
    series: total.map((point) => {
      if (point.tokens === null || point.cost === null) return point
      const value = partition(pointTokens(point), weights)[groupIndex]
      return {
        ...point,
        tokens: tokens(value),
        cost: {
          actualSum:
            (point.cost.actualSum * weights[groupIndex]) / weights.reduce((a, b) => a + b, 0),
          estimatedSum:
            (point.cost.estimatedSum * weights[groupIndex]) / weights.reduce((a, b) => a + b, 0),
          unavailableCount: partition(point.cost.unavailableCount, weights)[groupIndex],
        },
        messageCount:
          point.messageCount === null ? null : partition(point.messageCount, weights)[groupIndex],
        sessionRecordCount:
          point.sessionRecordCount === null
            ? null
            : partition(point.sessionRecordCount, weights)[groupIndex],
      }
    }),
  }))
  return { total, groups, coverageNotes: [] }
}

test('不分组 is the default and issues no extra reads', async ({ page }) => {
  await openOverview(page)

  await expect(page.getByTestId('overview-trend')).toHaveAttribute('data-group-mode', 'none')
  await expect(page.getByTestId('trend-group-none')).toHaveAttribute('aria-pressed', 'true')
  await expect(page.getByTestId('trend-group-legend')).toHaveCount(0)
  expect(await mockCalls(page, 'get_breakdown')).toHaveLength(0)
  const trendCalls = await mockCalls(page, 'get_trend')
  expect(trendCalls).toHaveLength(1)
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
  expect(await legendValues(page)).toEqual([WINDOW_TOTAL_TOKENS])
  await expect(page.getByTestId('trend-group-topn')).toContainText('1')

  expect(await mockCalls(page, 'get_trend')).toHaveLength(1)
  expect(await mockCalls(page, 'get_breakdown')).toHaveLength(0)

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
  // Per-bucket rounding across 5 atomic buckets × 3 groups bounds the window-total drift under 20.
  const sum = values.reduce((total, value) => total + value, 0)
  expect(Math.abs(sum - WINDOW_TOTAL_TOKENS)).toBeLessThan(20)
  // A full partition needs no 其他 line.
  await expect(
    page.locator('[data-testid="trend-group-legend-item"][data-other="true"]'),
  ).toHaveCount(0)

  await qaScreenshot(page, 'trend-group-model.png')
})

test('按 agent groups by agent_key and labels with the raw agent name', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'agent')

  await expect(legendItems(page)).toHaveCount(3)
  expect(await legendLabels(page)).toEqual(['Atlas - Plan Executor', 'build', 'Research Assistant'])

  expect(await mockCalls(page, 'get_trend')).toHaveLength(1)

  await qaScreenshot(page, 'trend-group-agent.png')
})

test('a wide window is capped at the palette size and the tail becomes 其他', async ({ page }) => {
  const modelCount = TREND_GROUP_LIMIT + 5
  await openOverview(page, { dataset: { trend: manyModelTrend(modelCount) } })
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
  expect(otherValue).toBeLessThanOrEqual(WINDOW_TOTAL_TOKENS)

  expect(await mockCalls(page, 'get_trend')).toHaveLength(1)
  expect(await mockCalls(page, 'get_breakdown')).toHaveLength(0)

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
  await openOverview(page, {
    dataset: { trend: { total: mockDataset().trend.total, groups: [], coverageNotes: [] } },
  })
  await selectGroup(page, 'model')

  await expect(page.getByTestId('empty-state')).toBeVisible()
  await expect(page.getByTestId('trend-group-legend')).toHaveCount(0)
})

test('the single trend-query failure is scoped to the chart card', async ({ page }) => {
  await openShell(page, {
    errors: {
      get_trend: { code: 'database', message: 'archive database is locked', fields: {} },
    },
  })

  await expect(page.getByTestId('view-overview')).toBeVisible()
  await expect(page.getByTestId('error-code')).toHaveText('database')
  await expect(page.getByTestId('overview-trend')).toHaveCount(0)
  // The summary half of the page is unaffected.
  await expect(page.getByTestId('overview-summary')).toBeVisible()
})

/**
 * Legend single-select (round-8 user report: "点击图例…应该只显示当前选中，隐去其余统计图").
 *
 * The chart's own legend is hand-drawn — recharts' built-in legend is switched off with
 * `legendType="none"` — so selection is asserted against the drawn `.recharts-line` count, which
 * is the only ground truth for "the other lines are gone".
 */
function lines(page: Page): Locator {
  return page.locator('.recharts-line')
}

test('clicking a legend entry keeps only that line and the coverage bands stay', async ({
  page,
}) => {
  await openOverview(page)
  await selectGroup(page, 'model')
  await expect(lines(page)).toHaveCount(3)
  await expect(page.locator('[data-testid="coverage-band"]')).toHaveCount(2)

  const second = legendItems(page).nth(1)
  const label = await second.getAttribute('data-series')
  await second.click()

  await expect(lines(page)).toHaveCount(1)
  await expect(second).toHaveAttribute('aria-pressed', 'true')
  await expect(second).toHaveAttribute('data-selected', 'true')
  await expect(legendItems(page).nth(0)).toHaveAttribute('data-selected', 'false')
  // Coverage is a property of the window, taken from the ungrouped total, so selecting one group
  // must not hide the bands — otherwise the user reads the selected range as fully collected.
  await expect(page.locator('[data-testid="coverage-band"]')).toHaveCount(2)
  await expect(page.getByTestId('trend-legend-selected')).toContainText(label ?? '')
  // Every legend entry stays listed and readable; only the plotted lines are narrowed.
  await expect(legendItems(page)).toHaveCount(3)

  await qaScreenshot(page, 'trend-legend-selected.png')
})

test('clicking the selected legend entry again restores every line', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'model')

  const first = legendItems(page).first()
  await first.click()
  await expect(lines(page)).toHaveCount(1)

  await first.click()
  await expect(lines(page)).toHaveCount(3)
  await expect(first).toHaveAttribute('data-selected', 'false')
  await expect(page.getByTestId('trend-legend-selected')).toHaveCount(0)
  await expect(page.getByTestId('trend-legend-hint')).toBeVisible()
})

test('显示全部曲线 is an escape hatch out of single-select', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'model')
  await legendItems(page).nth(2).click()
  await expect(lines(page)).toHaveCount(1)

  await page.getByTestId('trend-legend-show-all').click()

  await expect(lines(page)).toHaveCount(3)
  await expect(page.getByTestId('trend-legend-show-all')).toHaveCount(0)
})

test('其他 is selectable on its own, because it is the folded-away remainder', async ({ page }) => {
  const modelCount = TREND_GROUP_LIMIT + 5
  await openOverview(page, { dataset: { trend: manyModelTrend(modelCount) } })
  await selectGroup(page, 'model')
  await expect(lines(page)).toHaveCount(TREND_GROUP_LIMIT + 1)

  const other = page.locator('[data-testid="trend-group-legend-item"][data-other="true"]')
  await other.click()

  await expect(lines(page)).toHaveCount(1)
  await expect(other).toHaveAttribute('data-selected', 'true')
  await expect(page.getByTestId('trend-legend-selected')).toContainText('其他')
})

test('switching the group dimension drops a stale selection instead of plotting the wrong line', async ({
  page,
}) => {
  await openOverview(page)
  await selectGroup(page, 'model')
  await legendItems(page).nth(1).click()
  await expect(lines(page)).toHaveCount(1)

  // Group keys are index-derived, so `g1` also exists under 按 agent — it must not stay selected.
  await selectGroup(page, 'agent')

  await expect(lines(page)).toHaveCount(3)
  await expect(page.getByTestId('trend-legend-selected')).toHaveCount(0)
})

test('a legend entry is reachable and activatable from the keyboard', async ({ page }) => {
  await openOverview(page)
  await selectGroup(page, 'model')

  const first = legendItems(page).first()
  await first.focus()
  await page.keyboard.press('Enter')

  await expect(first).toHaveAttribute('aria-pressed', 'true')
  await expect(lines(page)).toHaveCount(1)
})
