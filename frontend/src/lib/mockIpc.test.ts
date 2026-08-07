import { Channel, invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { describe, expect, it } from 'vitest'

import type { RefreshEvent, TriggerRefreshResult } from '@/generated'

import { installMockIpc } from './mockIpc'

const RESIZE_EVENT = 'tauri://resize'

// `@tauri-apps/api/event` is exercised for real here rather than stubbed: the defect these
// cases pin down was the mock satisfying `invoke` while missing a second global the library
// reads, which only a real `listen()`/`unlisten()` round-trip can catch.
describe('mock IPC event plugin fidelity', () => {
  it('installs the event plugin global that unlisten() reads', () => {
    installMockIpc()

    expect(typeof window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener).toBe('function')
  })

  it('completes a listen/unlisten round-trip and stops delivering afterwards', async () => {
    const controller = installMockIpc()

    const unlisten = await listen(RESIZE_EVENT, () => undefined)
    expect(controller.emitEvent(RESIZE_EVENT)).toBe(1)

    await expect(unlisten()).resolves.toBeUndefined()

    expect(controller.emitEvent(RESIZE_EVENT)).toBe(0)
  })

  it('treats unregisterListener as idempotent so the following unlisten invoke is a no-op', async () => {
    const controller = installMockIpc()
    const unlisten = await listen(RESIZE_EVENT, () => undefined)

    window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener(RESIZE_EVENT, 1)

    await expect(unlisten()).resolves.toBeUndefined()
    expect(controller.emitEvent(RESIZE_EVENT)).toBe(0)
  })
})

describe('mock IPC channel fidelity', () => {
  it('delivers ordered refresh messages and records the serialized channel argument', async () => {
    const controller = installMockIpc()
    const events: RefreshEvent[] = []
    const onEvent = new Channel<RefreshEvent>()
    onEvent.onmessage = (event) => events.push(event)

    await invoke<TriggerRefreshResult>('trigger_refresh', {
      hostId: 'ssh-host-0000002',
      onEvent,
    })

    expect(events.map((event) => event.event)).toEqual(['started', 'finished'])
    expect(controller.lastArgs('trigger_refresh')).toMatchObject({
      hostId: 'ssh-host-0000002',
      onEvent: expect.stringMatching(/^__CHANNEL__:/),
    })
  })
})
