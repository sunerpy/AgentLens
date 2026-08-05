import { readFileSync } from 'node:fs'
import path from 'node:path'

import { describe, expect, it } from 'vitest'

/**
 * `tauri.macos.conf.json` is applied to `tauri.conf.json` with `json_patch::merge`
 * (RFC 7396), which **replaces arrays wholesale** rather than merging them element-wise.
 * `app.windows` is an array, so the macOS override has to restate every geometry field or
 * the macOS build silently ships different window dimensions. These assertions turn that
 * silent drift into a failing test.
 */
const SRC_TAURI = path.resolve(import.meta.dirname, '../../../../src-tauri')

type WindowConfig = Record<string, unknown>

function windowConfig(file: string): WindowConfig {
  const raw = readFileSync(path.join(SRC_TAURI, file), 'utf8')
  const parsed = JSON.parse(raw) as { app?: { windows?: WindowConfig[] } }
  const windows = parsed.app?.windows
  if (windows === undefined || windows.length !== 1) {
    throw new Error(`${file} must declare exactly one app.windows entry`)
  }
  return windows[0]
}

const SHARED_GEOMETRY = ['title', 'width', 'height', 'minWidth', 'minHeight'] as const

describe('tauri window configuration', () => {
  const base = windowConfig('tauri.conf.json')
  const macos = windowConfig('tauri.macos.conf.json')

  it('turns decorations off in the base config so Windows and Linux draw our titlebar', () => {
    expect(base.decorations).toBe(false)
  })

  it('keeps the undecorated shadow on, which is what preserves Windows 11 rounded corners', () => {
    expect(base.shadow).toBe(true)
  })

  it('keeps the native macOS traffic lights instead of drawing a second set', () => {
    expect(macos.decorations).toBe(true)
    expect(macos.titleBarStyle).toBe('Overlay')
    expect(macos.hiddenTitle).toBe(true)
  })

  it('positions the traffic lights where the CSS inset expects them', () => {
    // `--titlebar-inset-start: 5rem` in index.css is derived as
    // x(20) + 2 * pitch(20) + button(12) + breathing(8) = 80px.
    expect(macos.trafficLightPosition).toEqual({ x: 20, y: 18 })
  })

  it('restates the shared geometry identically, because RFC 7396 replaces the array', () => {
    for (const key of SHARED_GEOMETRY) {
      expect(macos[key], `app.windows[0].${key} drifted between the two configs`).toEqual(base[key])
    }
  })

  it('never lets the macOS override touch the bundle icon list', () => {
    const raw = JSON.parse(readFileSync(path.join(SRC_TAURI, 'tauri.macos.conf.json'), 'utf8')) as {
      bundle?: unknown
    }
    expect(raw.bundle).toBeUndefined()
  })
})

describe('window capability permissions', () => {
  const capability = JSON.parse(
    readFileSync(path.join(SRC_TAURI, 'capabilities/default.json'), 'utf8'),
  ) as { permissions: string[] }

  it('grants every window command the self-drawn titlebar invokes', () => {
    // `core:window:default` already covers `is_maximized` and the `internal_toggle_maximize`
    // that Tauri's own drag script fires on double-click; these four are not in that set and
    // would be rejected at runtime without an explicit grant.
    expect(capability.permissions).toContain('core:window:allow-minimize')
    expect(capability.permissions).toContain('core:window:allow-toggle-maximize')
    expect(capability.permissions).toContain('core:window:allow-close')
    expect(capability.permissions).toContain('core:window:allow-start-dragging')
  })

  it('still includes the core default set that is_maximized relies on', () => {
    expect(capability.permissions).toContain('core:default')
  })
})
