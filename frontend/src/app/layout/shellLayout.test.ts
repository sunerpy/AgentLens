import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

import { THEME_KEYS, THEME_MODE, type ThemeKey } from '@/app/theme/themes'
import {
  DEFAULT_SIDEBAR_LAYOUT,
  SETTING_KEY_SIDEBAR_COLLAPSED,
  SETTING_KEY_SIDEBAR_PINNED,
  SETTING_KEY_SIDEBAR_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_RAIL_WIDTH,
  SIDEBAR_RECALL_WIDTH,
  SIDEBAR_SETTING_KEYS,
  clampSidebarWidth,
  mainWidthCss,
  parseSidebarFlag,
  parseSidebarWidth,
  readSidebarLayout,
  serializeSidebarLayout,
  sidebarState,
  sidebarWidthPx,
  type SidebarLayout,
} from '@/app/layout/shellLayout'

function layout(patch: Partial<SidebarLayout> = {}): SidebarLayout {
  return { ...DEFAULT_SIDEBAR_LAYOUT, ...patch }
}

describe('侧栏三态', () => {
  it('展开 → 收缩 → 隐藏 三态各自映射到一个宽度', () => {
    expect(sidebarState(layout())).toBe('expanded')
    expect(sidebarWidthPx(layout())).toBe(SIDEBAR_DEFAULT_WIDTH)

    expect(sidebarState(layout({ collapsed: true }))).toBe('collapsed')
    expect(sidebarWidthPx(layout({ collapsed: true }))).toBe(SIDEBAR_RAIL_WIDTH)

    expect(sidebarState(layout({ hidden: true }))).toBe('hidden')
    expect(sidebarWidthPx(layout({ hidden: true }))).toBe(0)
  })

  it('hidden 压过 collapsed：隐藏的侧栏不再是「收缩」，否则召回条不会渲染', () => {
    const both = layout({ collapsed: true, hidden: true })
    expect(sidebarState(both)).toBe('hidden')
    expect(sidebarWidthPx(both)).toBe(0)
  })

  it('召回后回到隐藏之前的收缩态，而不是一律回到展开', () => {
    const collapsedThenHidden = layout({ collapsed: true, hidden: true })
    const recalled = { ...collapsedThenHidden, hidden: false }
    expect(sidebarState(recalled)).toBe('collapsed')
    expect(sidebarWidthPx(recalled)).toBe(SIDEBAR_RAIL_WIDTH)
  })

  it('固定态挤压主内容，浮动态与隐藏态都让主内容占满', () => {
    expect(mainWidthCss(layout())).toBe(`calc(100% - ${String(SIDEBAR_DEFAULT_WIDTH)}px)`)
    expect(mainWidthCss(layout({ collapsed: true }))).toBe(
      `calc(100% - ${String(SIDEBAR_RAIL_WIDTH)}px)`,
    )
    expect(mainWidthCss(layout({ pinned: false }))).toBe('100%')
    expect(mainWidthCss(layout({ hidden: true }))).toBe('100%')
  })

  it('召回触发条比 work-kit 的 5px 宽，且不超过主内容 24px 的左侧留白', () => {
    expect(SIDEBAR_RECALL_WIDTH).toBeGreaterThan(5)
    expect(SIDEBAR_RECALL_WIDTH).toBeLessThan(24)
  })
})

describe('宽度 clamp 与解析', () => {
  it('宽度被夹在 200..320，越界不抛错', () => {
    expect(clampSidebarWidth(100)).toBe(SIDEBAR_MIN_WIDTH)
    expect(clampSidebarWidth(1000)).toBe(SIDEBAR_MAX_WIDTH)
    expect(clampSidebarWidth(260)).toBe(260)
    expect(clampSidebarWidth(260.4)).toBe(260)
    expect(clampSidebarWidth(Number.NaN)).toBe(SIDEBAR_DEFAULT_WIDTH)
    expect(clampSidebarWidth(Number.POSITIVE_INFINITY)).toBe(SIDEBAR_DEFAULT_WIDTH)
  })

  it('持久化里的脏宽度回落到默认值而不是 0 宽侧栏', () => {
    expect(parseSidebarWidth('280')).toBe(280)
    expect(parseSidebarWidth(' 280 ')).toBe(280)
    expect(parseSidebarWidth('9999')).toBe(SIDEBAR_MAX_WIDTH)
    expect(parseSidebarWidth('abc')).toBe(SIDEBAR_DEFAULT_WIDTH)
    expect(parseSidebarWidth('')).toBe(SIDEBAR_DEFAULT_WIDTH)
    expect(parseSidebarWidth(undefined)).toBe(SIDEBAR_DEFAULT_WIDTH)
  })

  it('布尔量的假值拼写与 resolve_auto_refresh_enabled 一致，缺键走 fallback', () => {
    for (const falsy of ['false', '0', 'off', 'no', 'FALSE', ' No ']) {
      expect(parseSidebarFlag(falsy, true)).toBe(false)
    }
    for (const truthy of ['true', '1', 'on', 'yes', 'TRUE']) {
      expect(parseSidebarFlag(truthy, false)).toBe(true)
    }
    expect(parseSidebarFlag(undefined, true)).toBe(true)
    expect(parseSidebarFlag(undefined, false)).toBe(false)
    expect(parseSidebarFlag('maybe', true)).toBe(true)
  })
})

describe('持久化边界：collapsed / width / pinned 存盘，hidden 不存', () => {
  it('本模块只拥有三个设置键，hidden 不在其中', () => {
    expect([...SIDEBAR_SETTING_KEYS]).toEqual([
      SETTING_KEY_SIDEBAR_COLLAPSED,
      SETTING_KEY_SIDEBAR_WIDTH,
      SETTING_KEY_SIDEBAR_PINNED,
    ])
    expect(SIDEBAR_SETTING_KEYS.join(' ')).not.toContain('hidden')
  })

  it('序列化只写出 patch 里出现的键，且宽度先 clamp 再落盘', () => {
    expect(serializeSidebarLayout({ collapsed: true })).toEqual({
      [SETTING_KEY_SIDEBAR_COLLAPSED]: 'true',
    })
    expect(serializeSidebarLayout({ pinned: false })).toEqual({
      [SETTING_KEY_SIDEBAR_PINNED]: 'false',
    })
    expect(serializeSidebarLayout({ width: 9999 })).toEqual({
      [SETTING_KEY_SIDEBAR_WIDTH]: String(SIDEBAR_MAX_WIDTH),
    })
    expect(serializeSidebarLayout({})).toEqual({})
  })

  it('序列化产物里永远不会出现表示隐藏的键', () => {
    const values = serializeSidebarLayout({ collapsed: true, pinned: false, width: 240 })
    expect(Object.keys(values).sort()).toEqual([...SIDEBAR_SETTING_KEYS].sort())
  })

  it('读回持久化的三元组，缺键时用默认值', () => {
    expect(readSidebarLayout({})).toEqual({
      collapsed: false,
      pinned: true,
      width: SIDEBAR_DEFAULT_WIDTH,
    })
    expect(
      readSidebarLayout({
        [SETTING_KEY_SIDEBAR_COLLAPSED]: 'true',
        [SETTING_KEY_SIDEBAR_PINNED]: 'false',
        [SETTING_KEY_SIDEBAR_WIDTH]: '300',
      }),
    ).toEqual({ collapsed: true, pinned: false, width: 300 })
  })

  it('读回的结果里没有 hidden 字段，所以重启后侧栏一定回来', () => {
    const restored = readSidebarLayout({ [SETTING_KEY_SIDEBAR_COLLAPSED]: 'true' })
    expect('hidden' in restored).toBe(false)
    expect(sidebarState({ ...restored, hidden: false })).toBe('collapsed')
  })
})

/**
 * 六主题下侧栏必须可读。
 *
 * jsdom 不解析 Tailwind，也不加载 index.css，所以这里断言的是 **CSS 源文件**：把每个
 * `[data-theme]` 块按真实层叠顺序叠在 `:root` / `.dark` 上，解析 oklch 的亮度，再检查侧栏
 * 实际用到的那几个 token 的对比。这正是被删掉的 `--sidebar-*` 预设分支翻车的地方 ——
 * 那八个 token 只在 `:root` 与 `.dark` 里声明过，四个彩色主题一个都没有重声明。
 */
const CSS_PATH = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../index.css')
const CSS = readFileSync(CSS_PATH, 'utf8')

/** `:root {` / `.dark {` / `[data-theme='x'] {` … 到行首 `}` 为止。 */
function cssBlock(selector: string): string {
  const escaped = selector.replaceAll(/[[\].$^*+?()|{}\\]/g, String.raw`\$&`)
  const match = new RegExp(`^${escaped} \\{$([\\s\\S]*?)^\\}$`, 'm').exec(CSS)
  expect(match, `index.css 里找不到 ${selector} 块`).not.toBeNull()
  return match?.[1] ?? ''
}

function declarations(block: string): Record<string, string> {
  const values: Record<string, string> = {}
  for (const line of block.split('\n')) {
    const match = /^\s{2}(--[\w-]+):\s*(.+?);\s*$/.exec(line)
    if (match !== null) values[match[1]] = match[2].trim()
  }
  return values
}

/** 与浏览器一致的层叠：浅色主题叠在 `:root` 上，深色主题还要先叠 `.dark`。 */
function effectiveTokens(theme: ThemeKey): Record<string, string> {
  const base = declarations(cssBlock(':root'))
  const dark = THEME_MODE[theme] === 'dark' ? declarations(cssBlock('.dark')) : {}
  const tinted =
    theme === 'light' || theme === 'dark' ? {} : declarations(cssBlock(`[data-theme='${theme}']`))
  return { ...base, ...dark, ...tinted }
}

function lightness(value: string): number {
  const match = /oklch\(\s*([\d.]+)/.exec(value)
  expect(match, `不是 oklch 值：${value}`).not.toBeNull()
  return Number(match?.[1] ?? 0)
}

describe('六个主题下的侧栏可读性（断言 index.css 源码）', () => {
  it('从未被渲染过的 --sidebar-* 预设分支已从 index.css 删除', () => {
    const declared = /^\s*--sidebar[\w-]*\s*:/m.exec(CSS)
    expect(declared, '不要重新引入 --sidebar-*，理由见 index.css 的 @theme 注释').toBeNull()
    expect(/^\s*--color-sidebar[\w-]*\s*:/m.exec(CSS)).toBeNull()
  })

  it('每个主题都自己声明了侧栏用到的全部 token，没有一个靠继承别的模式', () => {
    const required = ['--card', '--border', '--muted', '--muted-foreground', '--foreground']
    for (const theme of THEME_KEYS) {
      const tokens = effectiveTokens(theme)
      for (const name of required) {
        expect(tokens[name], `${theme} 缺少 ${name}`).toBeDefined()
      }
    }
    // 四个彩色主题必须在自己的块里重声明这些 token，而不是落到 :root / .dark ——
    // 这正是 --sidebar-* 当年出问题的形状。
    for (const theme of ['forest', 'ocean', 'amber', 'violet'] as const) {
      const own = declarations(cssBlock(`[data-theme='${theme}']`))
      for (const name of ['--card', '--border', '--muted', '--muted-foreground', '--foreground']) {
        expect(own[name], `${theme} 没有自己声明 ${name}`).toBeDefined()
      }
    }
  })

  it('侧栏底色与其上的文字、次要文字、描边都有足够亮度差', () => {
    for (const theme of THEME_KEYS) {
      const tokens = effectiveTokens(theme)
      const card = lightness(tokens['--card'])
      expect(
        Math.abs(card - lightness(tokens['--foreground'])),
        `${theme} 侧栏底色与正文亮度太接近`,
      ).toBeGreaterThan(0.6)
      expect(
        Math.abs(card - lightness(tokens['--muted-foreground'])),
        `${theme} 侧栏底色与未选中项文字亮度太接近`,
      ).toBeGreaterThan(0.4)
      expect(
        Math.abs(card - lightness(tokens['--border'])),
        `${theme} 侧栏与主内容之间的分隔线看不见`,
      ).toBeGreaterThan(0.05)
    }
  })

  it('选中项的填充色与其前景文字亮度相反，深浅两种模式都不会自己盖住自己', () => {
    for (const theme of THEME_KEYS) {
      const tokens = effectiveTokens(theme)
      expect(
        Math.abs(lightness(tokens['--primary']) - lightness(tokens['--primary-foreground'])),
        `${theme} 选中项填充与其文字亮度太接近`,
      ).toBeGreaterThan(0.35)
    }
  })
})
