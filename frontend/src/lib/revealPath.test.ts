import { afterEach, describe, expect, it, vi } from 'vitest'

import { revealPath } from './revealPath'

vi.mock('@tauri-apps/plugin-opener', () => ({
  revealItemInDir: vi.fn(),
}))

const { revealItemInDir } = await import('@tauri-apps/plugin-opener')
const revealMock = vi.mocked(revealItemInDir)

const BRIDGE = '__TAURI_INTERNALS__'

function withBridge(): void {
  Object.defineProperty(window, BRIDGE, { value: {}, configurable: true, writable: true })
}

afterEach(() => {
  Reflect.deleteProperty(window, BRIDGE)
  revealMock.mockReset()
})

describe('revealPath', () => {
  it('degrades to "unsupported" without touching the plugin when no shell is present', async () => {
    expect(BRIDGE in window).toBe(false)

    // Must resolve, not throw: a `vite dev` tab and the Playwright QA run both land here,
    // and an unhandled rejection would take the settings view down with it.
    await expect(revealPath('/tmp/agentlens/archive.db')).resolves.toBe('unsupported')
    expect(revealMock).not.toHaveBeenCalled()
  })

  it('reveals the exact path it was given inside the shell', async () => {
    withBridge()
    revealMock.mockResolvedValue(undefined)

    await expect(revealPath('/tmp/agentlens/archive.db')).resolves.toBe('revealed')
    expect(revealMock).toHaveBeenCalledExactlyOnceWith('/tmp/agentlens/archive.db')
  })

  it('reports "failed" instead of rejecting when the OS refuses', async () => {
    withBridge()
    revealMock.mockRejectedValue(new Error('opener.reveal_item_in_dir not allowed'))

    await expect(revealPath('/tmp/agentlens/archive.db')).resolves.toBe('failed')
  })
})
