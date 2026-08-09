import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { AppSidebar } from '@/app/AppSidebar'
import { ShellLayoutProvider } from '@/app/layout/ShellLayoutProvider'
import {
  SETTING_KEY_SIDEBAR_COLLAPSED,
  SETTING_KEY_SIDEBAR_PINNED,
  SETTING_KEY_SIDEBAR_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_RAIL_WIDTH,
  SIDEBAR_WIDTH_STEP,
} from '@/app/layout/shellLayout'
import { VIEW_KEYS, type ViewKey } from '@/app/views'
import { zh } from '@/i18n/zh'
import { getSettings, setSettings } from '@/lib/ipc'

vi.mock('@/lib/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/ipc')>()),
  getSettings: vi.fn(),
  setSettings: vi.fn(),
}))

const getSettingsMock = vi.mocked(getSettings)
const setSettingsMock = vi.mocked(setSettings)

/**
 * Stands in for the `app_settings` table. The merge is a real upsert rather than a replace,
 * because that is what `write_app_settings` does — a test that replaced the map would let a
 * write that drops unrelated keys pass.
 */
let stored: Record<string, string | undefined> = {}

beforeEach(() => {
  stored = {}
  getSettingsMock.mockImplementation(() => Promise.resolve({ values: { ...stored } }))
  setSettingsMock.mockImplementation((settings) => {
    stored = { ...stored, ...settings.values }
    return Promise.resolve({ values: { ...stored } })
  })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

function renderSidebar(active: ViewKey = 'overview') {
  const onSelect = vi.fn()
  const queryClient = new QueryClient({
    defaultOptions: { mutations: { retry: false }, queries: { retry: false } },
  })
  const result = render(
    <QueryClientProvider client={queryClient}>
      <ShellLayoutProvider>
        <AppSidebar active={active} onSelect={onSelect} />
      </ShellLayoutProvider>
    </QueryClientProvider>,
  )
  return { ...result, onSelect }
}

function rail(): HTMLElement {
  return screen.getByTestId('app-sidebar')
}

function railState(): string | null {
  return rail().getAttribute('data-state')
}

/** Waits for the first `get_settings` to land, so no assertion races the hydration. */
async function hydrated(): Promise<void> {
  await waitFor(() => {
    expect(getSettingsMock).toHaveBeenCalled()
  })
}

async function collapse(): Promise<void> {
  fireEvent.click(screen.getByTestId('sidebar-toggle-collapsed'))
  await waitFor(() => {
    expect(railState()).toBe('collapsed')
  })
}

async function hide(): Promise<void> {
  fireEvent.click(screen.getByTestId('sidebar-toggle-hidden'))
  await waitFor(() => {
    expect(screen.queryByTestId('app-sidebar')).toBeNull()
  })
}

describe('侧栏三态切换', () => {
  it('默认展开：宽度 260px，六个导航项都带可见文字', async () => {
    renderSidebar()
    await hydrated()

    expect(railState()).toBe('expanded')
    expect(rail().style.width).toBe(`${String(SIDEBAR_DEFAULT_WIDTH)}px`)
    for (const view of VIEW_KEYS) {
      expect(screen.getByTestId(`nav-${view}`).textContent).toContain(zh.nav[view])
    }
  })

  it('展开 → 收缩：宽度 64px，文字消失但 aria-label 与 title 仍在', async () => {
    renderSidebar()
    await hydrated()

    await collapse()

    expect(rail().style.width).toBe(`${String(SIDEBAR_RAIL_WIDTH)}px`)
    for (const view of VIEW_KEYS) {
      const item = screen.getByTestId(`nav-${view}`)
      expect(item.textContent).not.toContain(zh.nav[view])
      expect(item.getAttribute('aria-label')).toBe(zh.nav[view])
      expect(item.getAttribute('title')).toBe(zh.nav[view])
    }
  })

  it('收缩 → 隐藏：侧栏离开 DOM，只留下召回触发条', async () => {
    renderSidebar()
    await hydrated()

    await collapse()
    await hide()

    expect(screen.getByTestId('sidebar-recall').getAttribute('aria-label')).toBe(zh.sidebar.show)
  })

  it('隐藏 → 召回：点触发条让侧栏回到隐藏前的收缩态', async () => {
    renderSidebar()
    await hydrated()

    await collapse()
    await hide()

    fireEvent.click(screen.getByTestId('sidebar-recall'))

    await waitFor(() => {
      expect(railState()).toBe('collapsed')
    })
    expect(screen.queryByTestId('sidebar-recall')).toBeNull()
  })

  it('悬停触发条只是浮动预览，不改变隐藏状态', async () => {
    renderSidebar()
    await hydrated()

    await hide()

    fireEvent.mouseEnter(screen.getByTestId('sidebar-recall'))
    await waitFor(() => {
      expect(rail().getAttribute('data-floating')).toBe('true')
    })
    // 仍然是 hidden：预览不是「显示」，所以触发条还在，移出后又收回去。
    expect(railState()).toBe('hidden')
    expect(screen.queryByTestId('sidebar-recall')).not.toBeNull()

    // 指针移出预览的右边缘就收回。这里刻意不用 mouseLeave —— 侧栏是在静止的光标
    // 「底下」出现的，浏览器没派发过对应的 mouseenter，所以也不会有 mouseleave。
    fireEvent.pointerMove(document, { clientX: 1000, clientY: 400 })
    await waitFor(() => {
      expect(screen.queryByTestId('app-sidebar')).toBeNull()
    })
  })

  it('隐藏态的悬停预览按展开后的宽度渲染，不是 0 宽的一条缝', async () => {
    renderSidebar()
    await hydrated()

    await hide()
    fireEvent.mouseEnter(screen.getByTestId('sidebar-recall'))

    await waitFor(() => {
      expect(rail().getAttribute('data-floating')).toBe('true')
    })
    expect(rail().style.width).toBe(`${String(SIDEBAR_DEFAULT_WIDTH)}px`)
  })

  it('从收缩态隐藏后，预览按 64px 渲染并且内容也是收缩版', async () => {
    renderSidebar()
    await hydrated()

    await collapse()
    await hide()
    fireEvent.mouseEnter(screen.getByTestId('sidebar-recall'))
    await waitFor(() => {
      expect(rail().getAttribute('data-floating')).toBe('true')
    })

    // 宽度与内容必须同时是收缩版。此前 collapsed 是从 state 推出来的，而 state 在隐藏时
    // 报 'hidden'，于是 64px 的盒子里渲染了展开版内容，页脚文案被压成一列单字。
    expect(rail().style.width).toBe(`${String(SIDEBAR_RAIL_WIDTH)}px`)
    for (const view of VIEW_KEYS) {
      expect(screen.getByTestId(`nav-${view}`).textContent).not.toContain(zh.nav[view])
    }
    expect(screen.getByTestId('sidebar-toggle-collapsed').textContent).not.toContain(
      zh.sidebar.collapse,
    )
  })

  it('固定/浮动切换只改布局模式，不改三态', async () => {
    renderSidebar()
    await hydrated()

    expect(rail().getAttribute('data-pinned')).toBe('true')
    expect(rail().getAttribute('data-floating')).toBe('false')

    fireEvent.click(screen.getByTestId('sidebar-toggle-pinned'))

    await waitFor(() => {
      expect(rail().getAttribute('data-pinned')).toBe('false')
    })
    expect(rail().getAttribute('data-floating')).toBe('true')
    expect(railState()).toBe('expanded')
  })
})

/** 子元素的身份标记：有 testid 用 testid，没有的（`<nav>`）用标签名。 */
function railChildren(): string[] {
  return [...rail().children].map((node) => node.getAttribute('data-testid') ?? node.tagName)
}

const CONTROL_IDS = [
  'sidebar-toggle-collapsed',
  'sidebar-toggle-pinned',
  'sidebar-toggle-hidden',
] as const

describe('控件位于顶部', () => {
  it('控制区是侧栏的第一个子元素，导航排在它之后', async () => {
    renderSidebar()
    await hydrated()

    const controls = screen.getByTestId('sidebar-controls')
    expect(rail().firstElementChild).toBe(controls)

    const nav = screen.getByRole('tablist')
    expect(
      controls.compareDocumentPosition(nav) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeGreaterThan(0)
    for (const testId of CONTROL_IDS) {
      expect(controls.contains(screen.getByTestId(testId))).toBe(true)
    }
  })

  it('页脚已移除：侧栏只剩控制区、导航与宽度手柄', async () => {
    renderSidebar()
    await hydrated()

    expect(railChildren()).toEqual(['sidebar-controls', 'NAV', 'sidebar-resize'])

    // 收缩态没有宽度手柄，所以只剩两个 —— 关键是控制区仍在最前，且末尾不再有页脚。
    await collapse()
    expect(railChildren()).toEqual(['sidebar-controls', 'NAV'])
  })

  it('收缩态下三个控件仍在顶部、纵向堆叠，且逐个都能聚焦', async () => {
    renderSidebar()
    await hydrated()

    await collapse()

    const controls = screen.getByTestId('sidebar-controls')
    // 64px 的内容盒放不下三个横向控件，所以收缩态必须是纵向的。
    expect(controls.className).toContain('flex-col')
    for (const testId of CONTROL_IDS) {
      const control = screen.getByTestId(testId)
      expect(controls.contains(control)).toBe(true)
      control.focus()
      expect(document.activeElement).toBe(control)
    }
  })

  it('收缩态保留隐藏控件：少了它，悬停预览就再没有退出口', async () => {
    renderSidebar()
    await hydrated()

    await collapse()
    await hide()
    fireEvent.mouseEnter(screen.getByTestId('sidebar-recall'))
    await waitFor(() => {
      expect(rail().getAttribute('data-floating')).toBe('true')
    })

    const restore = screen.getByTestId('sidebar-toggle-hidden')
    expect(restore.getAttribute('aria-label')).toBe(zh.sidebar.show)
    fireEvent.click(restore)
    await waitFor(() => {
      expect(railState()).toBe('collapsed')
    })
  })
})

describe('持久化边界', () => {
  it('收缩写 ui.sidebar.collapsed', async () => {
    renderSidebar()
    await hydrated()

    fireEvent.click(screen.getByTestId('sidebar-toggle-collapsed'))

    await waitFor(() => {
      expect(setSettingsMock).toHaveBeenCalledWith({
        values: { [SETTING_KEY_SIDEBAR_COLLAPSED]: 'true' },
      })
    })
  })

  it('固定切换写 ui.sidebar.pinned', async () => {
    renderSidebar()
    await hydrated()

    fireEvent.click(screen.getByTestId('sidebar-toggle-pinned'))

    await waitFor(() => {
      expect(setSettingsMock).toHaveBeenCalledWith({
        values: { [SETTING_KEY_SIDEBAR_PINNED]: 'false' },
      })
    })
  })

  it('改宽度写 ui.sidebar.width', async () => {
    renderSidebar()
    await hydrated()

    fireEvent.keyDown(screen.getByTestId('sidebar-resize'), { key: 'ArrowRight' })

    const expected = String(SIDEBAR_DEFAULT_WIDTH + SIDEBAR_WIDTH_STEP)
    await waitFor(() => {
      expect(setSettingsMock).toHaveBeenCalledWith({
        values: { [SETTING_KEY_SIDEBAR_WIDTH]: expected },
      })
    })
    expect(rail().style.width).toBe(`${expected}px`)
  })

  it('隐藏一次写盘都不产生：这是会话内动作，重启不该还是隐藏', async () => {
    renderSidebar()
    await hydrated()

    await hide()
    fireEvent.mouseEnter(screen.getByTestId('sidebar-recall'))
    await waitFor(() => {
      expect(rail().getAttribute('data-floating')).toBe('true')
    })

    expect(setSettingsMock).not.toHaveBeenCalled()
    expect(Object.keys(stored)).toHaveLength(0)
  })

  it('重挂载后 collapsed / width / pinned 恢复，hidden 不恢复', async () => {
    const first = renderSidebar()
    await hydrated()

    await collapse()
    await waitFor(() => {
      expect(stored[SETTING_KEY_SIDEBAR_COLLAPSED]).toBe('true')
    })
    fireEvent.click(screen.getByTestId('sidebar-toggle-pinned'))
    await waitFor(() => {
      expect(stored[SETTING_KEY_SIDEBAR_PINNED]).toBe('false')
    })
    await hide()

    // 「重启」：卸载后用同一份 app_settings 重新挂载。
    first.unmount()
    renderSidebar()

    await waitFor(() => {
      expect(railState()).toBe('collapsed')
    })
    expect(rail().getAttribute('data-pinned')).toBe('false')
    expect(screen.queryByTestId('sidebar-recall')).toBeNull()
  })
})

describe('键盘可达', () => {
  it('每个导航项都留在 Tab 顺序里，收缩态也一样', async () => {
    renderSidebar()
    await hydrated()

    for (const view of VIEW_KEYS) {
      expect(screen.getByTestId(`nav-${view}`).tabIndex).toBeGreaterThanOrEqual(0)
    }

    await collapse()
    for (const view of VIEW_KEYS) {
      expect(screen.getByTestId(`nav-${view}`).tabIndex).toBeGreaterThanOrEqual(0)
    }
  })

  it('导航项是原生 button，Enter 与 Space 都会派发 click', async () => {
    const { onSelect } = renderSidebar()
    await hydrated()

    const hosts = screen.getByTestId('nav-hosts')
    hosts.focus()
    // jsdom 不把按键转成 click，所以这里断言的是使浏览器这样做的那个前提：
    // 元素必须是 <button type="button">，而不是挂了 onClick 的 div。
    expect(hosts.tagName).toBe('BUTTON')
    expect(hosts.getAttribute('type')).toBe('button')
    fireEvent.click(hosts)
    expect(onSelect).toHaveBeenCalledWith('hosts')
  })

  it('上下方向键在导航项之间移动焦点并切换视图，Home / End 跳到两端', async () => {
    const { onSelect } = renderSidebar()
    await hydrated()

    const first = screen.getByTestId(`nav-${VIEW_KEYS[0]}`)
    first.focus()
    fireEvent.keyDown(first, { key: 'ArrowDown' })
    expect(onSelect).toHaveBeenLastCalledWith(VIEW_KEYS[1])
    expect(document.activeElement).toBe(screen.getByTestId(`nav-${VIEW_KEYS[1]}`))

    fireEvent.keyDown(document.activeElement as HTMLElement, { key: 'ArrowUp' })
    expect(onSelect).toHaveBeenLastCalledWith(VIEW_KEYS[0])

    fireEvent.keyDown(document.activeElement as HTMLElement, { key: 'End' })
    expect(onSelect).toHaveBeenLastCalledWith(VIEW_KEYS[VIEW_KEYS.length - 1])

    fireEvent.keyDown(document.activeElement as HTMLElement, { key: 'Home' })
    expect(onSelect).toHaveBeenLastCalledWith(VIEW_KEYS[0])
  })

  it('上方向键在首项回绕到末项，而不是卡住', async () => {
    const { onSelect } = renderSidebar()
    await hydrated()

    const first = screen.getByTestId(`nav-${VIEW_KEYS[0]}`)
    first.focus()
    fireEvent.keyDown(first, { key: 'ArrowUp' })
    expect(onSelect).toHaveBeenLastCalledWith(VIEW_KEYS[VIEW_KEYS.length - 1])
  })

  it('侧栏控件与宽度手柄都可聚焦，且带无障碍名称', async () => {
    renderSidebar()
    await hydrated()

    for (const testId of [
      'sidebar-toggle-collapsed',
      'sidebar-toggle-pinned',
      'sidebar-toggle-hidden',
      'sidebar-resize',
    ]) {
      const control = screen.getByTestId(testId)
      expect(control.tabIndex).toBeGreaterThanOrEqual(0)
      expect(control.getAttribute('aria-label')).toBeTruthy()
    }

    const handle = screen.getByTestId('sidebar-resize')
    expect(handle.getAttribute('role')).toBe('separator')
    expect(handle.getAttribute('aria-valuenow')).toBe(String(SIDEBAR_DEFAULT_WIDTH))
  })

  it('隐藏后的召回入口是真按钮，键盘也能召回', async () => {
    renderSidebar()
    await hydrated()

    await hide()

    const recall = screen.getByTestId('sidebar-recall')
    expect(recall.tagName).toBe('BUTTON')
    expect(recall.tabIndex).toBeGreaterThanOrEqual(0)

    fireEvent.focus(recall)
    await waitFor(() => {
      expect(rail().getAttribute('data-floating')).toBe('true')
    })
    fireEvent.click(recall)
    await waitFor(() => {
      expect(railState()).toBe('expanded')
    })
  })
})

describe('无障碍语义与选中反馈', () => {
  it('侧栏是纵向 tablist，选中项带 aria-selected', async () => {
    renderSidebar('hosts')
    await hydrated()

    const list = screen.getByRole('tablist')
    expect(list.getAttribute('aria-orientation')).toBe('vertical')
    expect(list.getAttribute('aria-label')).toBe(zh.sidebar.label)
    expect(screen.getByTestId('nav-hosts').getAttribute('aria-selected')).toBe('true')
    expect(screen.getByTestId('nav-overview').getAttribute('aria-selected')).toBe('false')
  })

  it('收缩态只剩图标时，选中项仍有非文字的标记', async () => {
    renderSidebar('detail')
    await hydrated()

    await collapse()

    expect(screen.queryByTestId('nav-marker-detail')).not.toBeNull()
    expect(screen.queryByTestId('nav-marker-overview')).toBeNull()
    // 选中项此时没有任何文字，所以这个标记是除填充色以外的第二个视觉信号。
    expect(screen.getByTestId('nav-detail').textContent).not.toContain(zh.nav.detail)
  })

  it('收缩态下侧栏控件保留 title，只有图标也能知道是什么', async () => {
    renderSidebar()
    await hydrated()

    await collapse()

    expect(screen.getByTestId('sidebar-toggle-collapsed').getAttribute('title')).toBe(
      zh.sidebar.expand,
    )
    expect(screen.getByTestId('sidebar-toggle-hidden').getAttribute('title')).toBe(zh.sidebar.hide)
  })
})
