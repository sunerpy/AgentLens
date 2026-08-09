import { expect, test, type Locator, type Page } from '@playwright/test'

import type { BreakdownRow, TokenValues } from '../src/generated'
import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Drilldown spec (todo 16). Component-level: real Chromium, mocked IPC, seeded dataset.
 *
 * Every number below is derived from the four `breakdown` rows in `src/lib/mockIpc.ts`.
 * Two of those rows share `(kiro-auth, claude-opus-5-max)` and differ only by `variant`
 * (`"xhigh"` vs `null`), which is what makes the level-3 expansion assertable.
 */
interface MetricExpectation {
  input: string
  output: string
  reasoning: string
  cacheTotal: string
  cacheSplit: string
  actual: string
  estimated: string
  messages: string
  share: string
}

async function expectMetrics(row: Locator, expected: MetricExpectation): Promise<void> {
  await expect(row.getByTestId('cell-input')).toHaveText(expected.input)
  await expect(row.getByTestId('cell-output')).toHaveText(expected.output)
  await expect(row.getByTestId('cell-reasoning')).toHaveText(expected.reasoning)
  await expect(row.getByTestId('cell-cache-total')).toHaveText(expected.cacheTotal)
  await expect(row.getByTestId('cell-cache-split')).toHaveText(expected.cacheSplit)
  await expect(row.getByTestId('cell-cost-actual')).toContainText(expected.actual)
  await expect(row.getByTestId('cell-cost-estimated')).toHaveText(expected.estimated)
  await expect(row.getByTestId('cell-messages')).toHaveText(expected.messages)
  await expect(row.getByTestId('share-bar')).toHaveAttribute('data-share', expected.share)
}

async function openDrilldown(page: Page, config: Parameters<typeof openShell>[1] = {}) {
  await openShell(page, config)
  await page.getByTestId('nav-drilldown').click()
  await expect(page.getByTestId('view-drilldown')).toBeVisible()
}

function agentRow(page: Page, agentKey: string): Locator {
  return page.locator(`[data-testid="drilldown-agent-row"][data-agent-key="${agentKey}"]`)
}

function zeroTokens(): TokenValues {
  return {
    tokInput: 0,
    tokOutput: 0,
    tokReasoning: 0,
    tokCacheRead: 0,
    tokCacheWrite: 0,
    totalInput: 0,
  }
}

test('level 1 aggregates the source and level 2 aggregates by agent_key with exact seeded values', async ({
  page,
}) => {
  await openDrilldown(page)

  await expect(page.getByTestId('drilldown-level-source')).toBeVisible()
  const sourceRows = page.getByTestId('drilldown-source-row')
  await expect(sourceRows).toHaveCount(1)
  await expect(sourceRows.first()).toHaveAttribute('data-source', 'opencode')
  await expectMetrics(sourceRows.first(), {
    input: '386,150',
    output: '29,550',
    reasoning: '2,150',
    cacheTotal: '242,950',
    cacheSplit: '231,200+11,750',
    actual: '$0.0484',
    estimated: '$0.0075',
    messages: '109',
    share: '100',
  })
  await expect(sourceRows.first().getByTestId('cost-unavailable-badge')).toContainText('1')

  // Four breakdown rows collapse into three agents: the two Atlas rows share one agent_key.
  const agentRows = page.getByTestId('drilldown-agent-row')
  await expect(agentRows).toHaveCount(3)
  expect(await agentRows.evaluateAll((rows) => rows.map((row) => row.dataset.agentKey))).toEqual([
    'atlas-plan-executor',
    'build',
    'research-assistant',
  ])

  await expectMetrics(agentRow(page, 'atlas-plan-executor'), {
    input: '276,150',
    output: '19,450',
    reasoning: '1,500',
    cacheTotal: '189,450',
    cacheSplit: '181,200+8,250',
    actual: '$0.0420',
    estimated: '$0.0000',
    messages: '74',
    share: '73.6',
  })
  await expectMetrics(agentRow(page, 'build'), {
    input: '78,000',
    output: '6,300',
    reasoning: '650',
    cacheTotal: '37,400',
    cacheSplit: '35,000+2,400',
    actual: '$0.0064',
    estimated: '$0.0075',
    messages: '24',
    share: '18.5',
  })
  await expectMetrics(agentRow(page, 'research-assistant'), {
    input: '32,000',
    output: '3,800',
    reasoning: '0',
    cacheTotal: '16,100',
    cacheSplit: '15,000+1,100',
    actual: '$0.0000',
    estimated: '$0.0000',
    messages: '11',
    share: '7.9',
  })

  // The raw label is display-only; the aggregation key is the normalized one.
  await expect(agentRow(page, 'atlas-plan-executor')).toContainText('Atlas - Plan Executor')
  await expect(page.getByTestId('drilldown-breadcrumb')).toContainText('opencode')
})

test('level 3 keeps variants collapsed and reveals the xhigh row on expand', async ({ page }) => {
  await openDrilldown(page)

  const modelRows = page.getByTestId('drilldown-model-row')
  await expect(modelRows).toHaveCount(1)
  await expect(modelRows.first()).toHaveAttribute('data-model-key', /kiro-auth.*claude-opus-5-max/)
  await expect(modelRows.first()).toHaveAttribute('data-expanded', 'false')
  await expect(page.getByTestId('drilldown-variant-row')).toHaveCount(0)

  // Collapsed, the model row is the sum of both variants.
  await expectMetrics(modelRows.first(), {
    input: '276,150',
    output: '19,450',
    reasoning: '1,500',
    cacheTotal: '189,450',
    cacheSplit: '181,200+8,250',
    actual: '$0.0420',
    estimated: '$0.0000',
    messages: '74',
    share: '100',
  })

  await page.getByTestId('drilldown-model-expand').click()
  await expect(modelRows.first()).toHaveAttribute('data-expanded', 'true')

  const variantRows = page.getByTestId('drilldown-variant-row')
  await expect(variantRows).toHaveCount(2)
  await expect(variantRows.first()).toHaveAttribute('data-variant', 'xhigh')
  await expect(variantRows.first().getByTestId('drilldown-variant-label')).toHaveText('xhigh')
  await expectMetrics(variantRows.first(), {
    input: '180,000',
    output: '12,400',
    reasoning: '1,500',
    cacheTotal: '125,200',
    cacheSplit: '120,000+5,200',
    actual: '$0.0301',
    estimated: '$0.0000',
    messages: '44',
    share: '65.6',
  })

  await expect(variantRows.nth(1)).toHaveAttribute('data-variant', '')
  await expectMetrics(variantRows.nth(1), {
    input: '96,150',
    output: '7,050',
    reasoning: '0',
    cacheTotal: '64,250',
    cacheSplit: '61,200+3,050',
    actual: '$0.0119',
    estimated: '$0.0000',
    messages: '30',
    share: '34.4',
  })

  await qaScreenshot(page, 'drilldown.png')
})

test('agents stay merged when agent_raw drifts, and the latest raw label is shown', async ({
  page,
}) => {
  const drifted: BreakdownRow[] = [
    {
      source: 'opencode',
      agentKey: 'atlas-plan-executor',
      agentRaw: 'Atlas - Plan Executor',
      providerId: 'kiro-auth',
      modelId: 'claude-opus-5-max',
      variant: 'xhigh',
      tokens: {
        tokInput: 100,
        tokOutput: 10,
        tokReasoning: 0,
        tokCacheRead: 0,
        tokCacheWrite: 0,
        totalInput: 100,
      },
      cost: { actualSum: 0.001, estimatedSum: 0, unavailableCount: 0 },
      messageCount: 2,
      sessionRecordCount: 0,
      activeSessionCount: 1,
    },
    {
      source: 'opencode',
      agentKey: 'atlas-plan-executor',
      agentRaw: 'Atlas Plan Executor',
      providerId: 'kiro-auth',
      modelId: 'claude-opus-5-max',
      variant: null,
      tokens: {
        tokInput: 300,
        tokOutput: 20,
        tokReasoning: 0,
        tokCacheRead: 0,
        tokCacheWrite: 0,
        totalInput: 300,
      },
      cost: { actualSum: 0.002, estimatedSum: 0, unavailableCount: 0 },
      messageCount: 3,
      sessionRecordCount: 0,
      activeSessionCount: 1,
    },
    {
      source: 'opencode',
      agentKey: 'build',
      agentRaw: 'build',
      providerId: 'openai',
      modelId: 'gpt-5-codex',
      variant: null,
      tokens: {
        tokInput: 50,
        tokOutput: 5,
        tokReasoning: 0,
        tokCacheRead: 0,
        tokCacheWrite: 0,
        totalInput: 50,
      },
      cost: { actualSum: 0, estimatedSum: 0.0005, unavailableCount: 0 },
      messageCount: 1,
      sessionRecordCount: 0,
      activeSessionCount: 1,
    },
  ]

  await openDrilldown(page, { dataset: { breakdown: drifted } })

  // Grouping by `agent_raw` would produce three rows here instead of two.
  await expect(page.getByTestId('drilldown-agent-row')).toHaveCount(2)
  const atlas = agentRow(page, 'atlas-plan-executor')
  await expect(atlas.getByTestId('cell-input')).toHaveText('400')
  await expect(atlas.getByTestId('cell-messages')).toHaveText('5')
  await expect(atlas).toContainText('Atlas Plan Executor')
})

test('unknown sources and degenerate rows render without crashing', async ({ page }) => {
  const degenerate: BreakdownRow[] = [
    {
      source: 'opencode',
      agentKey: 'build',
      agentRaw: 'build',
      providerId: 'openai',
      modelId: 'gpt-5-codex',
      variant: null,
      tokens: {
        tokInput: 10,
        tokOutput: 1,
        tokReasoning: 0,
        tokCacheRead: 0,
        tokCacheWrite: 0,
        totalInput: 10,
      },
      cost: { actualSum: 0.0001, estimatedSum: 0, unavailableCount: 0 },
      messageCount: 1,
      sessionRecordCount: 0,
      activeSessionCount: 1,
    },
    {
      source: 'codex',
      agentKey: '',
      agentRaw: '',
      providerId: 'openai',
      modelId: 'gpt-5-codex',
      variant: null,
      tokens: zeroTokens(),
      cost: { actualSum: 0, estimatedSum: 0, unavailableCount: 2 },
      messageCount: 0,
      sessionRecordCount: 0,
      activeSessionCount: 0,
    },
  ]

  await openDrilldown(page, { dataset: { breakdown: degenerate } })

  const sourceRows = page.getByTestId('drilldown-source-row')
  await expect(sourceRows).toHaveCount(2)
  expect(await sourceRows.evaluateAll((rows) => rows.map((row) => row.dataset.source))).toEqual([
    'opencode',
    'codex',
  ])

  const codex = page.locator('[data-testid="drilldown-source-row"][data-source="codex"]')
  await expect(codex.getByTestId('share-bar')).toHaveAttribute('data-share', '0')
  await expect(codex.getByTestId('cost-unavailable-badge')).toContainText('2')

  // A zero-token level must not divide by zero: the share renders as 0%, never NaN.
  await codex.getByRole('button').click()
  const agentRows = page.getByTestId('drilldown-agent-row')
  await expect(agentRows).toHaveCount(1)
  await expect(agentRows.first()).toHaveAttribute('data-agent-key', '')
  await expect(agentRows.first().getByTestId('cell-share')).toHaveText('0.0%')
  await expect(page.getByTestId('drilldown-level-model')).toBeVisible()
  await expect(page.getByTestId('error-state')).toHaveCount(0)
})

test('an empty range renders the empty state and keeps the host filter reachable', async ({
  page,
}) => {
  await openDrilldown(page, { responses: { get_breakdown: [] } })

  await expect(page.getByTestId('empty-state')).toBeVisible()
  await expect(page.getByTestId('empty-state')).toContainText('该区间无记录')
  await expect(page.getByTestId('drilldown-level-source')).toHaveCount(0)
  await expect(page.getByTestId('drilldown-level-agent')).toHaveCount(0)
  await expect(page.getByTestId('drilldown-level-model')).toHaveCount(0)
  await expect(page.getByTestId('drilldown-host-filter')).toBeVisible()
})

test('a structured IpcError renders the shared error state, not a blank view', async ({ page }) => {
  await openDrilldown(page, {
    errors: {
      get_breakdown: {
        code: 'database',
        message: 'archive database is locked',
        fields: { table: 'usage_record' },
      },
    },
  })

  await expect(page.getByTestId('error-state')).toBeVisible()
  await expect(page.getByTestId('error-code')).toHaveText('database')
  await expect(page.getByTestId('error-message')).toHaveText('archive database is locked')
  await expect(page.getByTestId('drilldown-level-source')).toHaveCount(0)
  await expect(page.getByTestId('drilldown-host-filter')).toBeVisible()
})

test('the query is driven by the shared range state and refetches when the host filter changes', async ({
  page,
}) => {
  await openDrilldown(page)
  await expect(page.getByTestId('drilldown-level-model')).toBeVisible()

  // The mock settings report UTC while the container runs Asia/Shanghai, so a UTC timezone
  // here proves the view consumes the shared, settings-hydrated range state.
  await expect(page.getByTestId('drilldown-timezone-value')).toHaveText('UTC')

  const initial = await mockCalls(page, 'get_breakdown')
  expect(initial.length).toBeGreaterThan(0)
  const args = initial[initial.length - 1].args as {
    range: { startDate: string; endDateExclusive: string; weekStart: string }
    dims: { timezone: string; expandVariant: boolean; filters: { hostId: string | null } }
  }
  expect(args.dims.timezone).toBe('UTC')
  expect(args.dims.expandVariant).toBe(true)
  expect(args.dims.filters.hostId).toBeNull()
  expect(args.range.weekStart).toBe('monday')
  expect(args.range.startDate).toMatch(/^\d{4}-\d{2}-\d{2}$/)
  expect(args.range.endDateExclusive).toMatch(/^\d{4}-\d{2}-\d{2}$/)
  await expect(page.getByTestId('drilldown-range-value')).toContainText(args.range.startDate)
  await expect(page.getByTestId('drilldown-range-value')).toContainText(args.range.endDateExclusive)

  await page.getByTestId('drilldown-host-filter').selectOption('ssh-host-0000002')

  await expect
    .poll(async () => (await mockCalls(page, 'get_breakdown')).length)
    .toBeGreaterThan(initial.length)
  const after = await mockCalls(page, 'get_breakdown')
  const refetched = after[after.length - 1].args as { dims: { filters: { hostId: string | null } } }
  expect(refetched.dims.filters.hostId).toBe('ssh-host-0000002')
})
