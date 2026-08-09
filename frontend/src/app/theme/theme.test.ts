import { describe, expect, it } from 'vitest'

import {
  DARK_CLASS,
  THEME_ATTRIBUTE,
  THEME_CACHE_KEY,
  applyCachedTheme,
  applyTheme,
  currentTheme,
  readCachedTheme,
  reconcileTheme,
  writeCachedTheme,
} from '@/app/theme/applyTheme'
import {
  DEFAULT_THEME,
  THEME_KEYS,
  THEME_MODE,
  THEME_SWATCH,
  isThemeKey,
  parseTheme,
  themeMode,
} from '@/app/theme/themes'
import { zh } from '@/i18n/zh'

function element(): HTMLElement {
  return document.createElement('html')
}

describe('主题注册表', () => {
  it('每个主题都有模式、色板与两条文案，不存在半配置的主题', () => {
    for (const theme of THEME_KEYS) {
      expect(THEME_MODE[theme]).toMatch(/^(light|dark)$/)
      expect(THEME_SWATCH[theme]).toHaveLength(3)
      expect(zh.theme.names[theme].length).toBeGreaterThan(0)
      expect(zh.theme.modes[theme].length).toBeGreaterThan(0)
    }
    expect(Object.keys(zh.theme.names).sort()).toEqual([...THEME_KEYS].sort())
  })

  it('至少提供三套配色，且浅色与深色都覆盖到', () => {
    expect(THEME_KEYS.length).toBeGreaterThanOrEqual(3)
    const modes = new Set(THEME_KEYS.map((theme) => THEME_MODE[theme]))
    expect(modes).toEqual(new Set(['light', 'dark']))
  })

  it('parseTheme 对未知值回落到默认主题而不是抛错', () => {
    expect(parseTheme('violet')).toBe('violet')
    expect(parseTheme('not-a-theme')).toBe(DEFAULT_THEME)
    expect(parseTheme(null)).toBe(DEFAULT_THEME)
    expect(parseTheme(undefined)).toBe(DEFAULT_THEME)
    expect(parseTheme(42)).toBe(DEFAULT_THEME)
    expect(isThemeKey('ocean')).toBe(true)
    expect(isThemeKey('OCEAN')).toBe(false)
  })
})

describe('主题落到 DOM', () => {
  it('深色主题同时带 data-theme 与 dark 类，浅色主题只带 data-theme', () => {
    const root = element()

    applyTheme(root, 'ocean')
    expect(root.getAttribute(THEME_ATTRIBUTE)).toBe('ocean')
    expect(root.classList.contains(DARK_CLASS)).toBe(true)

    applyTheme(root, 'forest')
    expect(root.getAttribute(THEME_ATTRIBUTE)).toBe('forest')
    expect(root.classList.contains(DARK_CLASS)).toBe(false)
  })

  it('每个主题的 dark 类都与注册表里的 mode 一致', () => {
    const root = element()
    for (const theme of THEME_KEYS) {
      applyTheme(root, theme)
      expect(root.classList.contains(DARK_CLASS)).toBe(themeMode(theme) === 'dark')
      expect(currentTheme(root)).toBe(theme)
    }
  })

  it('currentTheme 读到无法识别的属性值时回落到默认主题', () => {
    const root = element()
    root.setAttribute(THEME_ATTRIBUTE, 'chartreuse')
    expect(currentTheme(root)).toBe(DEFAULT_THEME)
  })
})

describe('启动缓存（仅首帧提示，app_settings 仍是权威）', () => {
  it('写入后能读回同一个主题', () => {
    const store = new Map<string, string>()
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, value),
    }

    writeCachedTheme(storage, 'amber')
    expect(store.get(THEME_CACHE_KEY)).toBe('amber')
    expect(readCachedTheme(storage)).toBe('amber')
  })

  it('存储不可用时读写都不抛错，只是回落到默认主题', () => {
    const throwing = {
      getItem: () => {
        throw new Error('storage disabled')
      },
      setItem: () => {
        throw new Error('storage disabled')
      },
    }

    expect(() => writeCachedTheme(throwing, 'violet')).not.toThrow()
    expect(readCachedTheme(throwing)).toBe(DEFAULT_THEME)
    expect(readCachedTheme(null)).toBe(DEFAULT_THEME)
  })

  it('缓存里的脏值不会被应用', () => {
    const storage = { getItem: () => 'rgb(1,2,3)' }
    expect(readCachedTheme(storage)).toBe(DEFAULT_THEME)
  })

  it('reconcileTheme 同时更新 DOM 与缓存，构成一次持久化往返', () => {
    const store = new Map<string, string>()
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, value),
    }
    const root = element()

    reconcileTheme(root, 'violet', storage)

    expect(currentTheme(root)).toBe('violet')
    expect(root.classList.contains(DARK_CLASS)).toBe(true)
    expect(store.get(THEME_CACHE_KEY)).toBe('violet')

    // 下一次启动：先按缓存刷首帧，再由 app_settings 覆盖。
    document.documentElement.removeAttribute(THEME_ATTRIBUTE)
    document.documentElement.classList.remove(DARK_CLASS)
    expect(applyCachedTheme(storage)).toBe('violet')
    expect(currentTheme(document.documentElement)).toBe('violet')
    expect(document.documentElement.classList.contains(DARK_CLASS)).toBe(true)
  })

  it('没有缓存时首帧落在默认主题，不是空白也不是深浅混搭', () => {
    document.documentElement.removeAttribute(THEME_ATTRIBUTE)
    document.documentElement.classList.remove(DARK_CLASS)

    expect(applyCachedTheme(null)).toBe(DEFAULT_THEME)
    expect(currentTheme(document.documentElement)).toBe(DEFAULT_THEME)
    expect(document.documentElement.classList.contains(DARK_CLASS)).toBe(
      themeMode(DEFAULT_THEME) === 'dark',
    )
  })
})
