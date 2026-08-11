import { expect, test, type Locator, type Page } from '@playwright/test'

import { zh } from '../src/i18n/zh'
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
async function hoverBucketOf(page: Page, index: number, count: number): Promise<void> {
  const grid = page.locator('.recharts-cartesian-grid').first()
  await expect(grid).toBeVisible()
  // `mouse.move` takes viewport coordinates and does not scroll; the chart sits below the
  // fold on a 720px-tall viewport, so it must be scrolled in before the box is measured.
  await grid.scrollIntoViewIfNeeded()
  const box = await grid.boundingBox()
  expect(box).not.toBeNull()
  if (box === null) return
  const band = box.width / count
  await page.mouse.move(box.x + band * (index + 0.5), box.y + box.height / 2)
}

async function hoverBucket(page: Page, index: number): Promise<void> {
  await hoverBucketOf(page, index, BUCKET_COUNT)
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

  // 本地估算是唯一的主数字；来源自带的金额在折叠披露里，展开后才有。
  await expect(page.getByTestId('summary-cost-estimated')).toHaveText('$0.0075')
  await expect(page.getByTestId('summary-cost-actual')).toHaveCount(0)
  await page.getByTestId('summary-cost-source-toggle').click()
  await expect(page.getByTestId('summary-cost-actual')).toHaveText('$0.0484')
  await expect(page.getByTestId('summary-cost-unavailable')).toHaveText('1')

  await expect(page.getByTestId('summary-message-count')).toHaveText('109')
  await expect(page.getByTestId('summary-active-session-count')).toHaveText('14')
})

/**
 * 用户第三次问「实际和估算为什么差那么多」（$83.5228 对 $312,235.4418），前两轮（补覆盖量标注、
 * 改竖排分层加单价）都没救回来。
 *
 * 因为缺陷有两条，两轮都没碰到：`actual` 的中文名是错的（那是 OpenCode 自带的估算值，不是账单），
 * 而且它覆盖 0.03% 却与覆盖 99.97% 的估算等重并排。所以这里断言的是**主次分层**：默认视图只有
 * 本地估算一个金额，来源自带在折叠披露里，展开后才连同「不是账单」「不要相加相减」一起出现。
 *
 * 种子数据的覆盖是 538,000 / 118,550 / 2,100 可计费 Token（合计 658,650），
 * 所以来源自带层 81.68%、本地估算层 18.00%。
 */
test('成本卡以本地估算为主数字，来源自带降为折叠披露', async ({ page }) => {
  await openOverview(page)

  const card = page.getByTestId('summary-cost-card')

  // 默认视图：只有一个金额，不再有两个大数邀请相减。
  await expect(page.getByTestId('summary-cost-tier-estimated')).toHaveAttribute(
    'data-emphasis',
    'primary',
  )
  await expect(page.getByTestId('summary-cost-estimated')).toHaveText('$0.0075')
  await expect(page.getByTestId('summary-cost-tier-actual')).toHaveCount(0)
  await expect(card).not.toContainText('$0.0484')
  await expect(card).toContainText(zh.overview.summary.costPrimaryHint)

  // 折叠入口用记录数说明这一层覆盖了多少。种子里带上游金额的是 90 条。
  const toggle = page.getByTestId('summary-cost-source-toggle')
  await expect(toggle).toHaveText(zh.overview.summary.costSourceShow('90'))
  await toggle.click()

  // 覆盖占比：展开后读者一眼看出两层覆盖的不是同一批记录。
  await expect(page.getByTestId('summary-cost-actual-share')).toHaveText(
    zh.overview.summary.costTierShare('81.68%'),
  )
  await expect(page.getByTestId('summary-cost-estimated-share')).toHaveText(
    zh.overview.summary.costTierShare('18.00%'),
  )

  // 单价是唯一可横向比较的数：$0.0484 / 0.538M ≈ $0.09，$0.0075 / 0.11855M ≈ $0.063。
  await expect(page.getByTestId('summary-cost-actual-unit-price')).toHaveText('$0.0900')
  await expect(page.getByTestId('summary-cost-estimated-unit-price')).toHaveText('$0.0633')

  const note = page.getByTestId('summary-cost-source-explain')
  await expect(note).toBeVisible()
  await expect(note).toHaveAttribute('data-comparability', 'incomparable')
  await expect(note).toContainText(zh.overview.summary.costSourceExplain)
  await expect(note).toContainText('不能相减')
  await expect(note).toContainText(zh.overview.summary.costUnitPriceHint)

  // 三态永不相加：卡片里不得出现 0.0484 + 0.0075 = 0.0559。
  await expect(card).not.toContainText('$0.0559')

  // 「永不相加」是实现约束，不是给用户的信息 —— 卡内不得出现。
  await expect(card).not.toContainText('永不相加')
  await expect(card).not.toContainText('缺失分层')

  await qaScreenshot(page, 'cost-incomparable.png')
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
  await expect(
    page.locator(`[data-testid="trend-dot-tokens"][data-bucket="${GAP_BUCKET}"]`),
  ).toHaveCount(0)

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
  await expect
    .poll(async () => (await mockCalls(page, 'get_trend')).at(-1)?.args.granularity)
    .toBe('hour')

  await page.getByTestId('granularity-day').click()
  await expect(page.getByTestId('granularity-auto')).toHaveAttribute('aria-pressed', 'false')
  await page.getByTestId('range-preset-last30Days').click()
  await expect(page.getByTestId('granularity-effective')).toHaveText('天')

  await page.getByTestId('granularity-month').click()
  await page.getByTestId('range-preset-today').click()
  // Pinned granularity survives a preset change that would otherwise derive `hour`.
  await expect(page.getByTestId('granularity-effective')).toHaveText('月')
  await expect
    .poll(async () => (await mockCalls(page, 'get_trend')).at(-1)?.args.granularity)
    .toBe('month')
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
  await expect.poll(async () => (await mockCalls(page, 'get_summary')).length).toBeGreaterThan(0)

  const latest = (await mockCalls(page, 'get_trend')).at(-1)
  const range = latest?.args.range as { startDate: string; endDateExclusive: string } | undefined
  expect(range).toBeDefined()
  if (range === undefined) return
  expect(range.startDate < range.endDateExclusive).toBe(true)
})

/**
 * 需求 3 的后半：自定义区间是**两个独立的日期选择器**（起始一个、截止一个），不再让用户在
 * 单个月历里来回翻页去凑两个端点。截止日期对用户是含当天的，派发时才 +1 天变成半开区间。
 */
test('the custom range picker takes a start and an end date and dispatches a half-open window', async ({
  page,
}) => {
  await openOverview(page)

  await page.getByTestId('range-preset-custom').click()
  await expect(page.getByTestId('range-custom-panel')).toBeVisible()

  const start = page.getByTestId('range-start-date')
  const end = page.getByTestId('range-end-date')
  // 两个字段各自是原生日期输入，不是同一个控件里的两次点击。
  expect(await start.evaluate((node) => (node as HTMLInputElement).type)).toBe('date')
  expect(await end.evaluate((node) => (node as HTMLInputElement).type)).toBe('date')

  await start.fill('2026-01-10')
  await end.fill('2026-01-12')
  await page.getByTestId('range-custom-apply').click()

  // The user picks an inclusive end (12); the stored window is half-open, so end is 13.
  await expect(page.getByTestId('range-window')).toHaveText('[2026-01-10, 2026-01-13)')
  await expect(page.getByTestId('range-preset-custom')).toHaveAttribute('aria-pressed', 'true')
  await expect
    .poll(async () => (await mockCalls(page, 'get_trend')).at(-1)?.args.granularity)
    .toBe('day')
})

/** 截止早于起始、或任一端为空，都必须阻断应用并说明原因，而不是派发一个倒置区间。 */
test('the custom range refuses an inverted or incomplete pair', async ({ page }) => {
  await openOverview(page)

  const windowBefore = await page.getByTestId('range-window').textContent()
  await page.getByTestId('range-preset-custom').click()

  const start = page.getByTestId('range-start-date')
  const end = page.getByTestId('range-end-date')
  const apply = page.getByTestId('range-custom-apply')
  const hint = page.getByTestId('range-custom-hint')

  await page.getByTestId('range-custom-clear').click()
  await expect(hint).toHaveText(zh.overview.range.requireBothDates)
  await expect(apply).toBeDisabled()

  await start.fill('2026-01-20')
  await expect(hint).toHaveText(zh.overview.range.requireBothDates)
  await expect(apply).toBeDisabled()

  // 起始晚于截止：报错且不可应用。
  await end.fill('2026-01-10')
  await expect(hint).toHaveText(zh.overview.range.invalidOrder)
  await expect(apply).toBeDisabled()

  // 期间从未派发过区间，看板仍停在原窗口上。
  await expect(page.getByTestId('range-window')).toHaveText(windowBefore ?? '')

  // 改成合法顺序后立刻可用。
  await end.fill('2026-01-25')
  await expect(hint).toHaveText(zh.overview.range.endDateInclusiveHint)
  await expect(apply).toBeEnabled()
  await apply.click()
  await expect(page.getByTestId('range-window')).toHaveText('[2026-01-20, 2026-01-26)')
})

/**
 * 需求 3 的前半：季度与年是**日历对齐**的周期预设。Granularity 只有 hour/day/week/month，
 * 没有 quarter/year 变体，所以季度窗口自动落到周桶、年窗口落到月桶 —— 365 个日点画在一根轴
 * 上是一条实心带，不是趋势。
 */
test('季度 and 年 presets are calendar-aligned and pick a readable granularity', async ({
  page,
}) => {
  await openOverview(page)

  await page.getByTestId('range-preset-thisQuarter').click()
  await expect(page.getByTestId('range-preset-thisQuarter')).toHaveAttribute('aria-pressed', 'true')
  const quarterWindow = (await page.getByTestId('range-window').textContent()) ?? ''
  // 起点必须是季度首日，即 1 / 4 / 7 / 10 月 1 日。
  expect(quarterWindow).toMatch(/^\[\d{4}-(01|04|07|10)-01, /)
  await expect(page.getByTestId('granularity-effective')).toHaveText(zh.common.granularity.week)
  await expect
    .poll(async () => (await mockCalls(page, 'get_trend')).at(-1)?.args.granularity)
    .toBe('week')

  await page.getByTestId('range-preset-thisYear').click()
  const yearWindow = (await page.getByTestId('range-window').textContent()) ?? ''
  expect(yearWindow).toMatch(/^\[\d{4}-01-01, /)
  await expect(page.getByTestId('granularity-effective')).toHaveText(zh.common.granularity.month)
  await expect
    .poll(async () => (await mockCalls(page, 'get_trend')).at(-1)?.args.granularity)
    .toBe('month')

  // 后端只接受 hour/day/week/month，任何一次请求都不能出现 quarter / year。
  const granularities = (await mockCalls(page, 'get_trend')).map((call) => call.args.granularity)
  expect(granularities).not.toContain('quarter')
  expect(granularities).not.toContain('year')
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
  await openShell(page, { dataset: { trend: { total: [], groups: [], coverageNotes: [] } } })

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
    sessionRecordCount: null,
  }))

  await openShell(page, { dataset: { trend: { total: allGap, groups: [], coverageNotes: [] } } })

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
        costCoverage: {
          actual: { recordCount: 0, billableTokens: 0 },
          estimated: { recordCount: 0, billableTokens: 0 },
          unavailable: { recordCount: 0, billableTokens: 0 },
        },
        messageCount: 0,
        sessionRecordCount: 0,
        activeSessionCount: 0,
      },
    },
  })

  await expect(page.getByTestId('summary-token-input')).toHaveText('0')
  await expect(page.getByTestId('summary-cost-estimated')).toHaveText('$0.0000')
  await expect(page.getByTestId('summary-message-count')).toHaveText('0')
  await expect(page.getByTestId('summary-cost-card')).toBeVisible()
  await expect(page.getByTestId('empty-state')).toHaveCount(0)
  await expect(page.locator('[data-testid="summary-cost-card"] .cost-badge-partial')).toHaveCount(0)
  /**
   * 全零边界：主数字的 $0.0000 照常渲染（它是真实合计），但必须挂上「这是没有数据，不是没花钱」
   * 的说明；来源自带零覆盖时连折叠入口都不出现，那个 $0 从此不在任何视觉层级上。
   */
  const primaryNote = page.getByTestId('summary-cost-primary-note')
  await expect(primaryNote).toHaveAttribute('data-primary-note', 'noCoverage')
  await expect(primaryNote).toHaveText(zh.overview.summary.costNoCoverage)
  await expect(page.getByTestId('summary-cost-source-toggle')).toHaveCount(0)
  await expect(page.getByTestId('summary-cost-actual')).toHaveCount(0)
  /**
   * 目录无价为 0 条时整段不渲染（round-8 用户反馈：「成本也没有任何显示 无可信成本 0 条
   * 这些记录不计入任何金额，也不当 0」）。那句是解释性文案，0 条时没有要解释的对象。
   */
  await expect(page.getByTestId('summary-cost-unavailable-block')).toHaveCount(0)
  await expect(page.getByTestId('summary-cost-unavailable')).toHaveCount(0)
  await expect(page.getByText(zh.overview.summary.costUnavailableHint)).toHaveCount(0)
})

/**
 * 用户原话：「部分缺失 看不出来是什么没有写价格」。
 *
 * 所以徽章旁边必须有一个**可点开**的明细，而不是只靠 hover 的 tooltip —— 不 hover 的用户
 * 永远不会知道还有更多信息。
 *
 * 清单的口径是**当前区间**：来自趋势查询的 model 分组，与 `get_summary` 同范围、同筛选。
 * 种子里唯一带 `unavailableCount` 的分组是 `kiro-auth / claude-opus-5-max`（第 4 个桶 1 条），
 * 与 `summary.cost.unavailableCount = 1` 恰好对上。全库口径的 `observedModels`
 * （`private-provider / private-model-v7`，3 条）**不得**出现在这里——两个口径同屏正是上一轮
 * 让用户以为算错了的缺陷。
 */
test('「部分缺失」能点开看到究竟哪些 provider / model 没有价格', async ({ page }) => {
  await openOverview(page)

  const card = page.getByTestId('summary-cost-card')
  await expect(card.locator('.cost-badge-partial')).toBeVisible()
  // 默认收起：关键信息有入口，但不占满卡片。
  await expect(page.getByTestId('cost-missing-list')).toHaveCount(0)

  await page.getByTestId('cost-missing-toggle').click()

  const list = page.getByTestId('cost-missing-list')
  await expect(list).toBeVisible()
  await expect(list.locator('li')).toHaveCount(1)
  await expect(page.getByTestId('cost-missing-entry-kiro-auth-claude-opus-5-max')).toContainText(
    'kiro-auth / claude-opus-5-max',
  )
  await expect(page.getByTestId('cost-missing-count')).toHaveText(
    zh.overview.summary.missingSummary(1, '1'),
  )
  // 已有价格的模型不能混进来，否则「缺价」这个词就没有意义了。
  await expect(list).not.toContainText('gpt-5-codex')
  await expect(list).not.toContainText('claude-sonnet-5')
  await qaScreenshot(page, 'overview-missing-prices.png')

  await page.getByTestId('cost-missing-toggle').click()
  await expect(page.getByTestId('cost-missing-list')).toHaveCount(0)
})

/**
 * 上一轮实测的缺陷：表头写「本范围内 21,947 条」，下面清单合计 50,923 条，用户以为算错了。
 * 根因是表头按区间统计而清单来自不带时间条件的全库 SQL。
 *
 * 这条用例把不变式钉死：清单每一行的记录数相加**必须**等于表头那个总数，且全库口径的清单
 * 不得同屏出现。种子刻意用两个缺价模型（1 + 3 = 4）验证多行相加，而不是只验证单行。
 */
test('缺价清单逐条相加等于表头总数，全库口径不同屏', async ({ page }) => {
  const tokens = {
    tokInput: 1_000,
    tokOutput: 100,
    tokReasoning: 0,
    tokCacheRead: 0,
    tokCacheWrite: 0,
    totalInput: 1_000,
  }
  const bucket = (index: number) => ({
    startUtcMs: Date.UTC(2026, 0, 1 + index),
    endUtcMs: Date.UTC(2026, 0, 2 + index),
    label: `2026-01-0${index + 1}`,
  })
  const covered = (index: number, unavailableCount: number) => ({
    bucket: bucket(index),
    coverage: 'full' as const,
    tokens,
    cost: { actualSum: 0.25, estimatedSum: 0, unavailableCount },
    messageCount: 4,
    sessionRecordCount: 0,
  })

  await openShell(page, {
    dataset: {
      summary: {
        tokens: { ...tokens, tokInput: 2_000, tokOutput: 200, totalInput: 2_000 },
        cost: { actualSum: 0.5, estimatedSum: 0, unavailableCount: 4 },
        costCoverage: {
          actual: { recordCount: 4, billableTokens: 1_100 },
          estimated: { recordCount: 0, billableTokens: 0 },
          unavailable: { recordCount: 4, billableTokens: 1_100 },
        },
        messageCount: 8,
        sessionRecordCount: 0,
        activeSessionCount: 2,
      },
      // 两个 model 分组分别贡献 1 与 3 条缺价记录，合计等于 summary 的 4。
      trend: {
        total: [covered(0, 1), covered(1, 3)],
        groups: [
          {
            dimension: 'model',
            id: 'kiro-auth\u0000claude-opus-5-max',
            label: 'kiro-auth / claude-opus-5-max',
            series: [covered(0, 1), covered(1, 0)],
          },
          {
            dimension: 'model',
            id: 'private-provider\u0000private-model-v7',
            label: 'private-provider / private-model-v7',
            series: [covered(0, 0), covered(1, 3)],
          },
        ],
        coverageNotes: [],
      },
    },
  })
  await expect(page.getByTestId('overview-summary')).toBeVisible()
  await expect(page.getByTestId('summary-cost-unavailable')).toHaveText('4')

  await page.getByTestId('cost-missing-toggle').click()

  const rows = page.getByTestId('cost-missing-list').locator('li')
  await expect(rows).toHaveCount(2)
  // 只读每行的记录数那一格：模型名本身带数字（`claude-opus-5-max`），整行剥非数字会把它们算进去。
  const listed = await rows.evaluateAll((items) =>
    items
      .map((item) =>
        Number((item.querySelectorAll('span')[1]?.textContent ?? '').replace(/\D/g, '')),
      )
      .reduce((sum, value) => sum + value, 0),
  )
  // 表头 4 条 = kiro-auth/claude-opus-5-max 的 1 条 + private-provider/private-model-v7 的 3 条。
  expect(listed).toBe(4)
  await expect(page.getByTestId('cost-missing-count')).toHaveText(
    zh.overview.summary.missingSummary(2, '4'),
  )
  // 相加对得上，所以既没有残差行，也没有全库分区。
  await expect(page.getByTestId('cost-missing-unattributed')).toHaveCount(0)
  await expect(page.getByTestId('cost-missing-archive')).toHaveCount(0)
  await expect(page.getByTestId('cost-missing')).toContainText(
    zh.overview.summary.missingRangeScopeHint,
  )
  // 成因分不了就要说分不了，不能让用户以为每条都「补个单价就好」。
  await expect(page.getByTestId('cost-missing')).toContainText(zh.overview.summary.missingCauseHint)
})

/**
 * 拿不到区间分组时（趋势查询失败）仍要能回答「什么没有价格」，但必须换到独立分区并自带口径
 * 说明——这是唯一允许出现全库清单的地方，且此时区间清单不存在，两个数不会并排。
 */
test('区间分组不可用时全库清单降级到独立分区并标注口径', async ({ page }) => {
  await openShell(page, {
    errors: {
      get_trend: { code: 'database', message: 'archive database is locked', fields: {} },
    },
  })
  await expect(page.getByTestId('overview-summary')).toBeVisible()

  await page.getByTestId('cost-missing-toggle').click()

  await expect(page.getByTestId('cost-missing-list')).toHaveCount(0)
  const archive = page.getByTestId('cost-missing-archive')
  await expect(archive).toBeVisible()
  await expect(archive).toContainText(zh.overview.summary.missingArchiveTitle)
  await expect(archive).toContainText(zh.overview.summary.missingArchiveScopeHint)
  await expect(
    page.getByTestId('cost-missing-archive-entry-private-provider-private-model-v7'),
  ).toContainText('private-provider / private-model-v7')
  // 范围内口径的说明不得出现，否则又变成两个口径同屏。
  await expect(page.getByTestId('cost-missing')).not.toContainText(
    zh.overview.summary.missingRangeScopeHint,
  )
  await qaScreenshot(page, 'overview-missing-prices-archive-fallback.png')
})

/**
 * Coverage explainability (round-8 user report: "还是有 部分覆盖 的情况，但是用户不知道为什么
 * 部分覆盖").
 *
 * The backend's Partial rule is "not every selected (host_id, source) pair covers the whole
 * bucket", so a user running two sources sees 部分覆盖 as soon as one of them has no archived
 * interval in that bucket. Naming the pairs is what turns a bare badge into something actionable.
 *
 * `mockIpc` seeds the partial bucket with one still-collecting host and one that never collected,
 * so both wordings are exercised in the same block.
 */
test('a partial bucket names the (host, source) pairs behind it without hovering', async ({
  page,
}) => {
  await openOverview(page)

  const reasons = page.getByTestId('trend-coverage-reasons')
  await reasons.scrollIntoViewIfNeeded()
  await expect(reasons).toBeVisible()
  await expect(reasons).toContainText(zh.overview.trend.coverageReasonTitle)
  await expect(reasons).toContainText(zh.overview.trend.coverageReasonHint)

  const partialRow = page.locator(
    `[data-testid="trend-coverage-reason-row"][data-bucket="${PARTIAL_BUCKET}"]`,
  )
  await expect(partialRow).toHaveCount(1)
  await expect(partialRow).toContainText(zh.common.coverage.partial)

  // 两种成因必须分开：只覆盖一部分 vs 完全没有采集区间。
  const partlyCovered = partialRow.locator(
    '[data-testid="coverage-reason-pair"][data-partial="true"]',
  )
  const neverCollected = partialRow.locator(
    '[data-testid="coverage-reason-pair"][data-partial="false"]',
  )
  await expect(partlyCovered).toHaveCount(1)
  await expect(partlyCovered).toContainText(zh.overview.trend.coverageReasonPartial)
  await expect(neverCollected).toHaveCount(1)
  await expect(neverCollected).toContainText(zh.overview.trend.coverageReasonMissing)
  // The host and the source are both named, so the user knows where to look.
  await expect(partlyCovered).toHaveAttribute('data-host-id', 'local')
  await expect(partlyCovered).toHaveAttribute('data-source', 'opencode')
  await expect(neverCollected).toHaveAttribute('data-host-id', 'build-box')
  await expect(neverCollected).toHaveAttribute('data-source', 'codex')

  await qaScreenshot(page, 'trend-coverage-reason.png')
})

test('the gap bucket is explained too, and fully covered buckets are not listed', async ({
  page,
}) => {
  await openOverview(page)

  const reasons = page.getByTestId('trend-coverage-reasons')
  await reasons.scrollIntoViewIfNeeded()
  await expect(
    page.locator(`[data-testid="trend-coverage-reason-row"][data-bucket="${GAP_BUCKET}"]`),
  ).toHaveCount(1)
  await expect(
    page.locator(`[data-testid="trend-coverage-reason-row"][data-bucket="${ZERO_BUCKET}"]`),
  ).toHaveCount(0)
  // Exactly the two non-full buckets of the seeded window.
  await expect(page.getByTestId('trend-coverage-reason-row')).toHaveCount(2)
})

test('the partial tooltip repeats the reason for pointer users', async ({ page }) => {
  await openOverview(page)
  await hoverBucket(page, 1)

  const tooltip = page.getByTestId('trend-tooltip')
  await expect(tooltip).toBeVisible()
  await expect(tooltip).toHaveAttribute('data-bucket', PARTIAL_BUCKET)
  await expect(tooltip.getByTestId('coverage-reason')).toBeVisible()
  await expect(tooltip.getByTestId('coverage-reason-pair')).toHaveCount(2)
})

test('a fully covered archive shows no coverage-reason block at all', async ({ page }) => {
  const covered = [0, 1].map((day) => ({
    bucket: {
      startUtcMs: Date.UTC(2026, 0, 1) + day * 86_400_000,
      endUtcMs: Date.UTC(2026, 0, 1) + (day + 1) * 86_400_000,
      label: `2026-01-0${day + 1}`,
    },
    coverage: 'full' as const,
    tokens: {
      tokInput: 10,
      tokOutput: 20,
      tokReasoning: 0,
      tokCacheRead: 0,
      tokCacheWrite: 0,
      totalInput: 10,
    },
    cost: { actualSum: 0.5, estimatedSum: 0, unavailableCount: 0 },
    messageCount: 2,
    sessionRecordCount: 0,
  }))

  await openShell(page, {
    dataset: { trend: { total: covered, groups: [], coverageNotes: [] } },
  })

  await expect(page.getByTestId('overview-trend')).toBeVisible()
  await expect(page.getByTestId('trend-coverage-reasons')).toHaveCount(0)
  await expect(page.locator('[data-testid="coverage-band"]')).toHaveCount(0)
})

/**
 * A non-`full` bucket the backend could not diagnose (no `coverageNotes` entry) must still say
 * something. Silently omitting the row would leave the badge unexplained again, which is the
 * defect this block exists to fix.
 */
test('an undiagnosable partial bucket says so instead of showing nothing', async ({ page }) => {
  const partial = [
    {
      bucket: {
        startUtcMs: Date.UTC(2026, 0, 1),
        endUtcMs: Date.UTC(2026, 0, 2),
        label: '2026-01-01',
      },
      coverage: 'partial' as const,
      tokens: {
        tokInput: 10,
        tokOutput: 20,
        tokReasoning: 0,
        tokCacheRead: 0,
        tokCacheWrite: 0,
        totalInput: 10,
      },
      cost: { actualSum: 0.5, estimatedSum: 0, unavailableCount: 0 },
      messageCount: 2,
      sessionRecordCount: 0,
    },
  ]

  await openShell(page, {
    dataset: { trend: { total: partial, groups: [], coverageNotes: [] } },
  })

  const reasons = page.getByTestId('trend-coverage-reasons')
  await reasons.scrollIntoViewIfNeeded()
  await expect(reasons).toBeVisible()
  await expect(reasons.getByTestId('coverage-reason-unknown')).toBeVisible()
  await expect(reasons.getByTestId('coverage-reason-pair')).toHaveCount(0)
})

/**
 * 进行中的时间桶（round-11 用户反馈：「2026-08-10 部分覆盖 / 六个 (主机, 源) 全是只覆盖了一部分
 * …… 没必要展示这些内容，当天用户也清楚只统计了一部分」）。
 *
 * 采集区间是 `[since, now]`，而后端的 Full 判定要求区间完整压住整个桶，所以「当前时刻所在的
 * 那个桶」永远是 Partial —— 与采集是否健康无关。把它的 (主机, 源) 当缺口逐条列出，等于每天
 * 每次刷新都刷屏，还会把真正漏采的历史桶埋掉。
 *
 * 因此 fixture 用真实的「今天 / 昨天」边界：桶边界照旧由数据提供（前端不做日历推导），
 * 只有「当前时刻落在哪个桶」这一步是前端按绝对时刻比较出来的。
 */
const DAY_MS = 86_400_000

function utcDayStart(offsetDays: number): number {
  return Math.floor(Date.now() / DAY_MS) * DAY_MS + offsetDays * DAY_MS
}

function dayLabel(startUtcMs: number): string {
  return new Date(startUtcMs).toISOString().slice(0, 10)
}

function dayPoint(startUtcMs: number, coverage: 'partial' | 'full') {
  return {
    bucket: { startUtcMs, endUtcMs: startUtcMs + DAY_MS, label: dayLabel(startUtcMs) },
    coverage,
    tokens: {
      tokInput: 120,
      tokOutput: 40,
      tokReasoning: 0,
      tokCacheRead: 0,
      tokCacheWrite: 0,
      totalInput: 120,
    },
    cost: { actualSum: 0.25, estimatedSum: 0, unavailableCount: 0 },
    messageCount: 6,
    sessionRecordCount: 0,
  }
}

const TODAY_START = utcDayStart(0)
const YESTERDAY_START = utcDayStart(-1)
const TODAY_LABEL = dayLabel(TODAY_START)
const YESTERDAY_LABEL = dayLabel(YESTERDAY_START)

/** 六个「只覆盖了一部分」的对，就是用户截图里那一屏。 */
const SIX_PARTIAL_PAIRS = [
  { hostId: 'local', source: 'opencode', partial: true },
  { hostId: 'local', source: 'codex', partial: true },
  { hostId: 'local', source: 'claude-code', partial: true },
  { hostId: 'build-box', source: 'opencode', partial: true },
  { hostId: 'build-box', source: 'codex', partial: true },
  { hostId: 'build-box', source: 'hermes', partial: true },
]

test('进行中的当天桶不进缺口诊断，已结束的历史桶仍逐对列出', async ({ page }) => {
  await openShell(page, {
    dataset: {
      trend: {
        total: [dayPoint(YESTERDAY_START, 'partial'), dayPoint(TODAY_START, 'partial')],
        groups: [],
        coverageNotes: [
          {
            label: YESTERDAY_LABEL,
            shortfalls: [
              { hostId: 'local', source: 'opencode', partial: true },
              { hostId: 'build-box', source: 'codex', partial: false },
            ],
          },
          { label: TODAY_LABEL, shortfalls: SIX_PARTIAL_PAIRS },
        ],
      },
    },
  })

  const reasons = page.getByTestId('trend-coverage-reasons')
  await reasons.scrollIntoViewIfNeeded()
  await expect(reasons).toBeVisible()

  // 当天桶一行都不出现，六个对更不出现。
  await expect(
    page.locator(`[data-testid="trend-coverage-reason-row"][data-bucket="${TODAY_LABEL}"]`),
  ).toHaveCount(0)
  await expect(reasons).not.toContainText('claude-code')

  // 昨天真有缺口，照旧两种成因分开列。
  const historical = page.locator(
    `[data-testid="trend-coverage-reason-row"][data-bucket="${YESTERDAY_LABEL}"]`,
  )
  await expect(historical).toHaveCount(1)
  await expect(historical).toHaveAttribute('data-in-progress', 'false')
  await expect(historical.locator('[data-testid="coverage-reason-pair"]')).toHaveCount(2)
  await expect(
    historical.locator('[data-testid="coverage-reason-pair"][data-partial="true"]'),
  ).toContainText(zh.overview.trend.coverageReasonPartial)
  await expect(
    historical.locator('[data-testid="coverage-reason-pair"][data-partial="false"]'),
  ).toContainText(zh.overview.trend.coverageReasonMissing)
  await expect(page.getByTestId('trend-coverage-reason-row')).toHaveCount(1)

  // 徽章与斜纹带都保留：数据确实不完整，只是理由换成「进行中」。
  const todayNote = page.locator(`[data-testid="trend-note"][data-bucket="${TODAY_LABEL}"]`)
  await expect(todayNote).toContainText(zh.common.coverage.partial)
  await expect(todayNote).toHaveAttribute('data-in-progress', 'true')
  await expect(todayNote.getByTestId('trend-note-in-progress')).toHaveText(
    zh.overview.trend.coverageInProgressTag,
  )
  await expect(
    page.locator(`[data-testid="coverage-band"][data-bucket="${TODAY_LABEL}"]`),
  ).toHaveCount(1)

  await qaScreenshot(page, 'trend-coverage-historical-gap.png')
})

test('只有进行中的桶不完整时，整块缺口诊断消失', async ({ page }) => {
  await openShell(page, {
    dataset: {
      trend: {
        total: [dayPoint(TODAY_START, 'partial')],
        groups: [],
        coverageNotes: [{ label: TODAY_LABEL, shortfalls: SIX_PARTIAL_PAIRS }],
      },
    },
  })

  await expect(page.getByTestId('overview-trend')).toBeVisible()
  await expect(page.getByTestId('trend-coverage-reasons')).toHaveCount(0)
  // 但仍能看出这个桶不完整：斜纹带 + 徽章 + 进行中。
  await expect(page.locator('[data-testid="coverage-band"]')).toHaveCount(1)
  const note = page.locator(`[data-testid="trend-note"][data-bucket="${TODAY_LABEL}"]`)
  await expect(note).toContainText(zh.common.coverage.partial)
  await expect(note.getByTestId('trend-note-in-progress')).toBeVisible()

  await qaScreenshot(page, 'trend-coverage-in-progress.png')
})

/** 「这个桶里完全没有它的采集区间」不是「桶还没结束」能解释的，必须继续报。 */
test('进行中的桶里完全没采到的源仍然被列出', async ({ page }) => {
  await openShell(page, {
    dataset: {
      trend: {
        total: [dayPoint(TODAY_START, 'partial')],
        groups: [],
        coverageNotes: [
          {
            label: TODAY_LABEL,
            shortfalls: [
              { hostId: 'local', source: 'opencode', partial: true },
              { hostId: 'build-box', source: 'codex', partial: false },
            ],
          },
        ],
      },
    },
  })

  const row = page.locator(
    `[data-testid="trend-coverage-reason-row"][data-bucket="${TODAY_LABEL}"]`,
  )
  await row.scrollIntoViewIfNeeded()
  await expect(row).toHaveCount(1)
  await expect(row).toHaveAttribute('data-in-progress', 'true')
  await expect(row.getByTestId('coverage-reason-in-progress-tag')).toBeVisible()
  const pairs = row.locator('[data-testid="coverage-reason-pair"]')
  await expect(pairs).toHaveCount(1)
  await expect(pairs).toHaveAttribute('data-partial', 'false')
  await expect(pairs).toHaveAttribute('data-host-id', 'build-box')
})

test('进行中的桶的悬浮提示说「还没结束」，不列出任何对', async ({ page }) => {
  await openShell(page, {
    dataset: {
      trend: {
        total: [dayPoint(YESTERDAY_START, 'full'), dayPoint(TODAY_START, 'partial')],
        groups: [],
        coverageNotes: [{ label: TODAY_LABEL, shortfalls: SIX_PARTIAL_PAIRS }],
      },
    },
  })
  await expect(page.getByTestId('overview-trend')).toBeVisible()
  await hoverBucketOf(page, 1, 2)

  const tooltip = page.getByTestId('trend-tooltip')
  await expect(tooltip).toBeVisible()
  await expect(tooltip).toHaveAttribute('data-bucket', TODAY_LABEL)
  // 徽章仍是「部分覆盖」，理由换成「还没结束」。
  await expect(tooltip.getByTestId('trend-tooltip-coverage')).toHaveText(zh.common.coverage.partial)
  const reason = tooltip.getByTestId('coverage-reason')
  await expect(reason).toHaveAttribute('data-in-progress', 'true')
  await expect(reason).toContainText(zh.overview.trend.coverageInProgressTitle)
  await expect(reason.getByTestId('coverage-reason-in-progress')).toContainText(
    zh.overview.trend.coverageInProgressNote,
  )
  await expect(reason.locator('[data-testid="coverage-reason-pair"]')).toHaveCount(0)
  await expect(reason.getByTestId('coverage-reason-unknown')).toHaveCount(0)
})
