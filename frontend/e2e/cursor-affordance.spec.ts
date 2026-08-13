import { expect, test, type Locator, type Page } from '@playwright/test'

import { openShell } from './harness'

/**
 * 鼠标反馈回归闸。
 *
 * 这个 spec 存在的唯一理由：光标形态是 `getComputedStyle` 才能看见的属性，截图里看不出来，
 * 人工点也只会在真机上才注意到。Tailwind v4 的 preflight 删掉了 v3 的
 * `button { cursor: pointer }`，一次升级就能把全应用的按钮悄悄变回箭头而不让任何测试变红 ——
 * 这就是它已经发生过一次的原因。
 */

/** 断言目标：testid → 期望的 `cursor` 计算值。 */
interface CursorCase {
  view: string
  testId: string
  expected: 'pointer' | 'not-allowed'
}

async function computedCursor(target: Locator): Promise<string> {
  return target.evaluate((element) => getComputedStyle(element).cursor)
}

/**
 * 渲染像素快照。hover 断言必须落在像素上：`animations: 'disabled'` 是必需的，因为按钮带
 * `transition-all`，否则截到的是过渡中间帧；`mouse.move(0, 0)` 先把指针挪开，否则上一次
 * hover 会残留在"静止"基准里，让断言恒真。
 */
async function pixels(target: Locator): Promise<Buffer> {
  return target.screenshot({ animations: 'disabled' })
}

async function restingPixels(page: Page, target: Locator): Promise<Buffer> {
  await page.mouse.move(0, 0)
  return pixels(target)
}

async function gotoView(page: Page, view: string): Promise<void> {
  await page.getByTestId(`nav-${view}`).click()
  await expect(page.getByTestId(`view-${view}`)).toBeVisible()
}

/**
 * 每页至少一个代表性目标，外加窗口 chrome。挑的是"用户第一眼会去点"的那些控件，
 * 不追求穷举 55 个共享 Button —— 它们共享同一个 base class，一个过全部过。
 */
const CHROME_CASES: readonly CursorCase[] = [
  { view: 'overview', testId: 'titlebar-minimize', expected: 'pointer' },
  { view: 'overview', testId: 'titlebar-maximize', expected: 'pointer' },
  { view: 'overview', testId: 'titlebar-close', expected: 'pointer' },
  { view: 'overview', testId: 'theme-menu-trigger', expected: 'pointer' },
  { view: 'overview', testId: 'sidebar-toggle-collapsed', expected: 'pointer' },
  { view: 'overview', testId: 'sidebar-toggle-pinned', expected: 'pointer' },
  { view: 'overview', testId: 'sidebar-toggle-hidden', expected: 'pointer' },
  { view: 'overview', testId: 'nav-overview', expected: 'pointer' },
  { view: 'overview', testId: 'nav-settings', expected: 'pointer' },
] as const

const VIEW_CASES: readonly CursorCase[] = [
  { view: 'overview', testId: 'range-preset-last7Days', expected: 'pointer' },
  { view: 'overview', testId: 'range-preset-custom', expected: 'pointer' },
  { view: 'overview', testId: 'granularity-auto', expected: 'pointer' },
  { view: 'drilldown', testId: 'drilldown-host-filter', expected: 'pointer' },
  { view: 'detail', testId: 'detail-next-page', expected: 'pointer' },
  { view: 'hosts', testId: 'host-refresh-all', expected: 'pointer' },
  { view: 'settings', testId: 'settings-timezone', expected: 'pointer' },
  { view: 'settings', testId: 'settings-week-start', expected: 'pointer' },
  { view: 'settings', testId: 'settings-auto-refresh', expected: 'pointer' },
  { view: 'settings', testId: 'settings-archive-copy', expected: 'pointer' },
  { view: 'diagnostics', testId: 'diagnostics-refresh', expected: 'pointer' },
] as const

test('window chrome and rail controls all report a pointer cursor', async ({ page }) => {
  await openShell(page)
  await expect(page.getByTestId('view-overview')).toBeVisible()

  const observed: Record<string, string> = {}
  for (const item of CHROME_CASES) {
    observed[item.testId] = await computedCursor(page.getByTestId(item.testId))
  }

  expect(observed).toEqual(
    Object.fromEntries(CHROME_CASES.map((item) => [item.testId, item.expected])),
  )
})

test('every view exposes a pointer cursor on its primary controls', async ({ page }) => {
  // 归档路径必须种上：为空时"复制路径"按钮是禁用的，那条断言就会去量禁用态而不是可点态。
  await openShell(page, {
    dataset: {
      settings: {
        values: {
          'report.timezone': 'UTC',
          'report.weekStart': 'monday',
          'archive.path': '/tmp/agentlens-qa/agentlens/archive.db',
        },
      },
    },
  })
  await expect(page.getByTestId('view-overview')).toBeVisible()

  const observed: Record<string, string> = {}
  for (const item of VIEW_CASES) {
    if (item.view !== 'overview') {
      await gotoView(page, item.view)
    }
    const target = page.getByTestId(item.testId)
    await expect(target).toBeVisible()
    observed[`${item.view}/${item.testId}`] = await computedCursor(target)
  }

  expect(observed).toEqual(
    Object.fromEntries(VIEW_CASES.map((item) => [`${item.view}/${item.testId}`, item.expected])),
  )
})

/**
 * 禁用态是这个 spec 里最容易回归的一条：`cursor-pointer` 一旦写在 base class 上而没有
 * `disabled:cursor-not-allowed` 压过去，禁用按钮就会承诺一次点不动的点击。
 *
 * 用第一页的"上一页"作为夹具：它由 `offset === 0` 驱动，不依赖任何 mock 数据形状。
 */
test('a disabled button reports not-allowed, never pointer', async ({ page }) => {
  await openShell(page)
  await gotoView(page, 'detail')

  const previous = page.getByTestId('detail-prev-page')
  await expect(previous).toBeDisabled()
  expect(await computedCursor(previous)).toBe('not-allowed')

  const next = page.getByTestId('detail-next-page')
  await expect(next).toBeEnabled()
  expect(await computedCursor(next)).toBe('pointer')
})

/**
 * `disabled:pointer-events-none` 被移走了 —— 否则禁用按钮根本不参与命中测试，
 * `not-allowed` 就只存在于计算值里而用户永远看不到。代价是禁用按钮现在会收到 hover，
 * 所以必须证明 hover 底色没有跟着亮起来，不然禁用态反而看起来像可点。
 *
 * 判据是渲染像素，不是 `getComputedStyle().backgroundColor`：Chrome 把 `oklch()` 的计算值
 * 重序列化成 `oklab()`，字符串会在颜色完全没变时也不相等，那样的断言两个方向都会说谎。
 */
test('hovering a disabled button changes nothing visually', async ({ page }) => {
  await openShell(page)
  await gotoView(page, 'detail')

  const previous = page.getByTestId('detail-prev-page')
  await expect(previous).toBeDisabled()

  const before = await restingPixels(page, previous)
  await previous.hover({ force: true })
  const after = await pixels(previous)

  expect(after.equals(before)).toBe(true)
})

/**
 * 只读元素不得拿到 pointer：滥用比缺失更糟，它会把用户引去点一个点不动的东西。
 * 标题栏拖动区和纯展示文本是这条规则的哨兵。
 */
test('non-interactive surfaces keep the default cursor', async ({ page }) => {
  await openShell(page)
  await expect(page.getByTestId('view-overview')).toBeVisible()

  expect(await computedCursor(page.getByTestId('titlebar-title'))).not.toBe('pointer')
  expect(await computedCursor(page.getByTestId('granularity-hint'))).not.toBe('pointer')
})

/**
 * 光标只是第一信号。这里守住第二个：可点元素 hover 时必须真的重绘。
 * 覆盖 ghost 图标按钮（六套主题里最容易做得看不见的那种）、分段控件里的 ghost、
 * 以及下钻表格行首列的真实点击靶区。
 */
const REPAINT_CASES: readonly { view: string; testId: string }[] = [
  { view: 'overview', testId: 'theme-menu-trigger' },
  { view: 'overview', testId: 'sidebar-toggle-collapsed' },
  { view: 'overview', testId: 'granularity-day' },
  { view: 'overview', testId: 'range-preset-today' },
] as const

test('hover repaints every kind of enabled control', async ({ page }) => {
  await openShell(page)
  await expect(page.getByTestId('view-overview')).toBeVisible()

  for (const item of REPAINT_CASES) {
    const target = page.getByTestId(item.testId)
    await expect(target).toBeVisible()
    const rest = await restingPixels(page, target)
    await target.hover()
    const hovered = await pixels(target)
    expect(hovered.equals(rest), `${item.testId} 悬停时没有任何视觉变化`).toBe(false)
  }
})

/**
 * 像素不等只说明有变化，不说明看得见 —— 光靠 `hover:text-foreground` 改一行字色也能让像素不等，
 * 而 ghost 的 hover 底色曾经在六套主题里都恰好等于所在背板，实测亮度差是 0.0000。
 *
 * 所以这里量的是渲染后的均值亮度：截图交回页面用 Chromium 自己解码到 canvas 逐像素求 sRGB
 * 相对亮度。不猜背板、不解析 `oklab()`（Chrome 会把 `oklch()` 重序列化成它），这是唯一
 * 不会说谎的量。地板取 1.15 的亮度比 —— 修复前那批 0 是 1.000，修复后实测最低 1.18。
 */
const LUMINANCE_RATIO_FLOOR = 1.15

async function backgroundLuminance(page: Page, target: Locator): Promise<number> {
  const png = await target.screenshot({ animations: 'disabled' })
  return page.evaluate(async (base64: string) => {
    const image = new Image()
    image.src = `data:image/png;base64,${base64}`
    await image.decode()
    const canvas = document.createElement('canvas')
    canvas.width = image.naturalWidth
    canvas.height = image.naturalHeight
    const ctx = canvas.getContext('2d')
    if (ctx === null) return Number.NaN
    ctx.drawImage(image, 0, 0)

    /*
     * 只取顶边中段那条带：控件里的文字是单行居中的，整幅均值会被字形稀释 —— 一个以文案为主
     * 的按钮即使底色完全没变，均值也只挪动一点，反过来底色真变了也被摊薄。列区间避开圆角，
     * 行区间取内缩 2px 起的两行，落在 padding 里，是纯底色。
     */
    const top = 2
    const rows = 2
    const left = Math.floor(canvas.width * 0.3)
    const width = Math.max(1, Math.floor(canvas.width * 0.4))
    const { data } = ctx.getImageData(left, top, width, Math.min(rows, canvas.height - top))
    const lin = (channel: number): number => {
      const v = channel / 255
      return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4
    }
    let sum = 0
    let count = 0
    for (let i = 0; i < data.length; i += 4) {
      sum += 0.2126 * lin(data[i]) + 0.7152 * lin(data[i + 1]) + 0.0722 * lin(data[i + 2])
      count += 1
    }
    return sum / count
  }, png.toString('base64'))
}

for (const theme of ['light', 'dark'] as const) {
  test(`hover luminance stays perceptible under the ${theme} theme`, async ({ page }) => {
    await openShell(page, {
      dataset: {
        settings: {
          values: { 'report.timezone': 'UTC', 'report.weekStart': 'monday', 'ui.theme': theme },
        },
      },
    })
    await expect(page.getByTestId('view-overview')).toBeVisible()

    for (const item of REPAINT_CASES) {
      const target = page.getByTestId(item.testId)
      await page.mouse.move(0, 0)
      const rest = await backgroundLuminance(page, target)
      await target.hover()
      const hovered = await backgroundLuminance(page, target)
      const ratio = (Math.max(rest, hovered) + 0.05) / (Math.min(rest, hovered) + 0.05)
      expect(ratio, `${item.testId} 的 hover 亮度比只有 ${ratio.toFixed(3)}`).toBeGreaterThan(
        LUMINANCE_RATIO_FLOOR,
      )
    }
  })
}

test('hover repaints the drill-down target inside a table row', async ({ page }) => {
  await openShell(page)
  await gotoView(page, 'drilldown')

  const target = page.getByTestId('drilldown-source-row').first().getByRole('button').first()
  await expect(target).toBeVisible()
  const rest = await restingPixels(page, target)
  await target.hover()
  const hovered = await pixels(target)
  expect(hovered.equals(rest)).toBe(false)
})
