import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { createWindowControls } from './windowControls'

const getCurrentWindow = vi.hoisted(() => vi.fn())

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow }))

describe('createWindowControls', () => {
  beforeEach(() => {
    getCurrentWindow.mockReset()
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('returns null when getCurrentWindow throws outside a Tauri webview', () => {
    getCurrentWindow.mockImplementation(() => {
      throw new TypeError("Cannot read properties of undefined (reading 'metadata')")
    })

    expect(createWindowControls()).toBeNull()
  })

  it('forwards every control to the Tauri window handle', async () => {
    const appWindow = {
      minimize: vi.fn().mockResolvedValue(undefined),
      toggleMaximize: vi.fn().mockResolvedValue(undefined),
      close: vi.fn().mockResolvedValue(undefined),
      isMaximized: vi.fn().mockResolvedValue(true),
      startDragging: vi.fn().mockResolvedValue(undefined),
      onResized: vi.fn().mockResolvedValue(() => undefined),
    }
    getCurrentWindow.mockReturnValue(appWindow)

    const controls = createWindowControls()
    expect(controls).not.toBeNull()
    if (controls === null) return

    await controls.minimize()
    await controls.toggleMaximize()
    await controls.close()
    await controls.startDragging()
    await expect(controls.isMaximized()).resolves.toBe(true)

    const handler = vi.fn()
    await controls.onResized(handler)
    expect(appWindow.onResized).toHaveBeenCalledTimes(1)

    // The wrapper must not forward Tauri's resize payload to the caller: the handler
    // signature is intentionally argument-free so callers cannot start trusting the
    // payload instead of re-querying `isMaximized()`.
    const forwarded = appWindow.onResized.mock.calls[0][0] as (payload: unknown) => void
    forwarded({ width: 1, height: 2 })
    expect(handler).toHaveBeenCalledWith()

    expect(appWindow.minimize).toHaveBeenCalledTimes(1)
    expect(appWindow.toggleMaximize).toHaveBeenCalledTimes(1)
    expect(appWindow.close).toHaveBeenCalledTimes(1)
    expect(appWindow.startDragging).toHaveBeenCalledTimes(1)
  })
})
