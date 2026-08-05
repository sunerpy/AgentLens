import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { UnlistenFn } from '@tauri-apps/api/event'

import { zh } from '@/i18n/zh'

import { TitleBar } from './TitleBar'
import { currentPlatform } from './platform'
import { createWindowControls, type WindowControls } from './windowControls'

vi.mock('./platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./platform')>()),
  currentPlatform: vi.fn(),
}))

vi.mock('./windowControls', () => ({ createWindowControls: vi.fn() }))

const platformMock = vi.mocked(currentPlatform)
const controlsMock = vi.mocked(createWindowControls)

type Harness = {
  controls: WindowControls
  emitResize: () => void
  unlisten: ReturnType<typeof vi.fn>
  setMaximized: (value: boolean) => void
}

function harness(overrides: Partial<WindowControls> = {}): Harness {
  let maximized = false
  let resizeHandler: (() => void) | undefined
  const unlisten = vi.fn()

  const controls: WindowControls = {
    minimize: vi.fn().mockResolvedValue(undefined),
    toggleMaximize: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
    startDragging: vi.fn().mockResolvedValue(undefined),
    isMaximized: vi.fn(() => Promise.resolve(maximized)),
    onResized: vi.fn((handler: () => void) => {
      resizeHandler = handler
      return Promise.resolve(unlisten)
    }),
    ...overrides,
  }

  return {
    controls,
    unlisten,
    emitResize: () => resizeHandler?.(),
    setMaximized: (value) => {
      maximized = value
    },
  }
}

const maximizeState = () => screen.getByTestId('titlebar-maximize').getAttribute('data-state')
const maximizeLabel = () => screen.getByTestId('titlebar-maximize').getAttribute('aria-label')

beforeEach(() => {
  platformMock.mockReturnValue('windows')
  controlsMock.mockReturnValue(harness().controls)
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('TitleBar layout', () => {
  it('marks the whole bar as a deep Tauri drag region', () => {
    render(<TitleBar />)

    // `deep` (not the bare attribute) is what makes the title text draggable while the
    // injected script still excludes the BUTTON elements; the bare form would only drag
    // when the container itself is the click target.
    expect(screen.getByTestId('titlebar').getAttribute('data-tauri-drag-region')).toBe('deep')
  })

  it('draws minimize, maximize and close on Windows', () => {
    render(<TitleBar />)

    expect(screen.queryByTestId('titlebar-minimize')).not.toBeNull()
    expect(screen.queryByTestId('titlebar-maximize')).not.toBeNull()
    expect(screen.queryByTestId('titlebar-close')).not.toBeNull()
    expect(screen.getByTestId('titlebar').getAttribute('data-platform')).toBe('windows')
  })

  it('draws its own controls on Linux and on an unknown platform', () => {
    for (const platform of ['linux', 'unknown'] as const) {
      platformMock.mockReturnValue(platform)
      render(<TitleBar />)
      expect(screen.queryByTestId('titlebar-controls')).not.toBeNull()
      expect(screen.getByTestId('titlebar').getAttribute('data-platform')).toBe(platform)
      cleanup()
    }
  })

  it('omits its own controls on macOS so the native traffic lights are not duplicated', () => {
    platformMock.mockReturnValue('macos')

    render(<TitleBar />)

    expect(screen.queryByTestId('titlebar-controls')).toBeNull()
    expect(screen.getByTestId('titlebar-title').textContent).toContain(zh.appName)
    expect(screen.getByTestId('titlebar').getAttribute('data-platform')).toBe('macos')
  })
})

describe('TitleBar maximize state', () => {
  it('starts in the normal state and offers the maximize action', async () => {
    render(<TitleBar />)

    await waitFor(() => expect(maximizeState()).toBe('normal'))
    expect(maximizeLabel()).toBe(zh.titlebar.maximize)
  })

  it('reflects a window that is already maximized on first paint', async () => {
    const bar = harness()
    bar.setMaximized(true)
    controlsMock.mockReturnValue(bar.controls)

    render(<TitleBar />)

    await waitFor(() => expect(maximizeState()).toBe('maximized'))
    expect(maximizeLabel()).toBe(zh.titlebar.restore)
  })

  it('re-syncs on a window resize the user never clicked for', async () => {
    const bar = harness()
    controlsMock.mockReturnValue(bar.controls)

    render(<TitleBar />)
    await waitFor(() => expect(maximizeState()).toBe('normal'))

    // Double-clicking the drag region, Win+Up and the OS window menu all maximize without
    // going through our button, so the icon must follow the resize event rather than the
    // click handler.
    bar.setMaximized(true)
    bar.emitResize()

    await waitFor(() => expect(maximizeState()).toBe('maximized'))
    expect(maximizeLabel()).toBe(zh.titlebar.restore)

    bar.setMaximized(false)
    bar.emitResize()

    await waitFor(() => expect(maximizeState()).toBe('normal'))
    expect(maximizeLabel()).toBe(zh.titlebar.maximize)
  })

  it('unsubscribes from the resize event on unmount', async () => {
    const bar = harness()
    controlsMock.mockReturnValue(bar.controls)

    const view = render(<TitleBar />)
    await waitFor(() => expect(bar.controls.onResized).toHaveBeenCalledTimes(1))

    view.unmount()

    await waitFor(() => expect(bar.unlisten).toHaveBeenCalledTimes(1))
  })
})

describe('TitleBar actions', () => {
  it('wires each button to its window command', async () => {
    const bar = harness()
    controlsMock.mockReturnValue(bar.controls)

    render(<TitleBar />)

    fireEvent.click(screen.getByTestId('titlebar-minimize'))
    fireEvent.click(screen.getByTestId('titlebar-maximize'))
    fireEvent.click(screen.getByTestId('titlebar-close'))

    await waitFor(() => expect(bar.controls.minimize).toHaveBeenCalledTimes(1))
    expect(bar.controls.toggleMaximize).toHaveBeenCalledTimes(1)
    expect(bar.controls.close).toHaveBeenCalledTimes(1)
  })

  it('leaves mouse drags to the native script and only falls back for touch and pen', () => {
    const bar = harness()
    controlsMock.mockReturnValue(bar.controls)

    render(<TitleBar />)
    const titlebar = screen.getByTestId('titlebar')

    fireEvent.pointerDown(titlebar, { pointerType: 'mouse' })
    expect(bar.controls.startDragging).not.toHaveBeenCalled()

    fireEvent.pointerDown(titlebar, { pointerType: 'touch' })
    fireEvent.pointerDown(titlebar, { pointerType: 'pen' })
    expect(bar.controls.startDragging).toHaveBeenCalledTimes(2)
  })
})

describe('TitleBar degradation', () => {
  it('renders a button-less bar instead of blanking when there is no Tauri window', () => {
    controlsMock.mockReturnValue(null)

    render(<TitleBar />)

    expect(screen.queryByTestId('titlebar')).not.toBeNull()
    expect(screen.getByTestId('titlebar-title').textContent).toContain(zh.appName)
    expect(screen.queryByTestId('titlebar-controls')).toBeNull()
  })

  it('keeps the bar mounted when isMaximized is rejected by the IPC layer', async () => {
    const bar = harness({
      isMaximized: vi.fn().mockRejectedValue({ code: 'internal', message: 'no handler' }),
    })
    controlsMock.mockReturnValue(bar.controls)

    render(<TitleBar />)

    await waitFor(() => expect(bar.controls.isMaximized).toHaveBeenCalled())
    expect(maximizeState()).toBe('normal')
    expect(maximizeLabel()).toBe(zh.titlebar.maximize)
  })

  it('keeps the bar mounted when the resize subscription itself is rejected', async () => {
    const bar = harness({
      onResized: vi.fn().mockRejectedValue(new Error('event channel unavailable')),
    })
    controlsMock.mockReturnValue(bar.controls)

    render(<TitleBar />)

    await waitFor(() => expect(bar.controls.onResized).toHaveBeenCalled())
    expect(screen.queryByTestId('titlebar-controls')).not.toBeNull()
  })

  it('swallows a rejected window command instead of surfacing an unhandled rejection', async () => {
    const bar = harness({
      minimize: vi.fn().mockRejectedValue(new Error('window.minimize not allowed')),
      toggleMaximize: vi.fn().mockRejectedValue(new Error('not allowed')),
      close: vi.fn().mockRejectedValue(new Error('not allowed')),
    })
    controlsMock.mockReturnValue(bar.controls)

    render(<TitleBar />)

    fireEvent.click(screen.getByTestId('titlebar-minimize'))
    fireEvent.click(screen.getByTestId('titlebar-maximize'))
    fireEvent.click(screen.getByTestId('titlebar-close'))

    await waitFor(() => expect(bar.controls.close).toHaveBeenCalledTimes(1))
    expect(screen.queryByTestId('titlebar')).not.toBeNull()
  })
})

const UNREGISTER_LISTENER_MISSING =
  "Cannot read properties of undefined (reading 'unregisterListener')"

/**
 * Stands in for the promise Tauri's `UnlistenFn` really returns. Adopting the returned thenable
 * (`await`, `Promise.resolve`, `.then`) invokes `then` with a rejection handler, so
 * `rejectionHandled` separates "the caller owns the failure" from "the caller dropped the
 * returned promise" — the latter being precisely what becomes an unhandled rejection at runtime.
 * Asserting on it is deterministic, unlike waiting for Node's `unhandledRejection` event.
 */
function rejectingUnlisten() {
  const state = { called: 0, rejectionHandled: false }
  const unlisten: UnlistenFn = () => {
    state.called += 1
    return {
      then: (_onFulfilled?: unknown, onRejected?: unknown) => {
        if (typeof onRejected === 'function') {
          state.rejectionHandled = true
          ;(onRejected as (reason: unknown) => void)(new TypeError(UNREGISTER_LISTENER_MISSING))
        }
        return Promise.resolve()
      },
    }
  }
  return { unlisten, state }
}

// Tauri hands back `async () => _unlisten(...)` behind the `() => void` type, so a broken
// unsubscribe surfaces in two different shapes and each needs its own containment.
describe('TitleBar unsubscribe failures', () => {
  it('takes ownership of a rejected unlisten promise instead of dropping it', async () => {
    const { unlisten, state } = rejectingUnlisten()
    const bar = harness({ onResized: vi.fn(() => Promise.resolve(unlisten)) })
    controlsMock.mockReturnValue(bar.controls)

    const view = render(<TitleBar />)
    await waitFor(() => expect(bar.controls.onResized).toHaveBeenCalledTimes(1))

    view.unmount()

    await waitFor(() => expect(state.rejectionHandled).toBe(true))
    expect(state.called).toBe(1)
  })

  it('lets unmount finish when the unlisten thunk throws synchronously', async () => {
    const unlisten = vi.fn(() => {
      throw new TypeError(UNREGISTER_LISTENER_MISSING)
    })
    const bar = harness({ onResized: vi.fn(() => Promise.resolve(unlisten)) })
    controlsMock.mockReturnValue(bar.controls)

    const view = render(<TitleBar />)
    await waitFor(() => expect(bar.controls.onResized).toHaveBeenCalledTimes(1))

    expect(() => view.unmount()).not.toThrow()
    expect(unlisten).toHaveBeenCalledTimes(1)
  })

  it('takes ownership of the rejection when the subscription settles after teardown', async () => {
    // The StrictMode path: the effect is torn down first, so the unsubscribe runs from inside
    // the subscription's `then` instead of from the cleanup function.
    let settle: (fn: UnlistenFn) => void = () => undefined
    const { unlisten, state } = rejectingUnlisten()
    const bar = harness({
      onResized: vi.fn(
        () =>
          new Promise<UnlistenFn>((resolve) => {
            settle = resolve
          }),
      ),
    })
    controlsMock.mockReturnValue(bar.controls)

    const view = render(<TitleBar />)
    await waitFor(() => expect(bar.controls.onResized).toHaveBeenCalledTimes(1))
    view.unmount()

    settle(unlisten)

    await waitFor(() => expect(state.rejectionHandled).toBe(true))
    expect(state.called).toBe(1)
  })
})
