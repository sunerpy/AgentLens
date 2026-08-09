import { Channel, invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { describe, expect, it } from 'vitest'

import type { RefreshEvent, TriggerRefreshResult } from '@/generated'

import { installMockIpc, mockDataset, SESSION_GRANULARITY_DATASET } from './mockIpc'

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

    const results = await invoke<TriggerRefreshResult[]>('trigger_refresh', {
      hostId: 'ssh-host-0000002',
      onEvent,
    })

    // 该主机启用了两个采集源，调度器键是 (host_id, source)，所以是两轮、两对事件、两条结果。
    expect(results.map((result) => result.source)).toEqual(['opencode', 'claude-code'])
    expect(events.map((event) => event.event)).toEqual([
      'started',
      'finished',
      'started',
      'finished',
    ])
    expect(controller.lastArgs('trigger_refresh')).toMatchObject({
      hostId: 'ssh-host-0000002',
      onEvent: expect.stringMatching(/^__CHANNEL__:/),
    })
  })

  it('每个采集源的事件都带自己的 source，不会被另一个源盖掉', async () => {
    installMockIpc()
    const events: RefreshEvent[] = []
    const onEvent = new Channel<RefreshEvent>()
    onEvent.onmessage = (event) => events.push(event)

    await invoke<TriggerRefreshResult[]>('trigger_refresh', {
      hostId: 'ssh-host-0000002',
      onEvent,
    })

    const sources = events.map((event) =>
      event.event === 'started' ? event.data.status.source : event.data.source,
    )
    expect(sources).toEqual(['opencode', 'opencode', 'claude-code', 'claude-code'])
  })
})

// 基准种子必须是「只启用 opencode」的常态（会话级记录恒 0），非零形态由独立 fixture 提供；
// 否则 UI 的两条呈现路径里总有一条永远测不到。
describe('mock IPC 记录粒度', () => {
  it('基准种子的会话汇总记录数处处为 0，覆盖缺口仍是 null', () => {
    const dataset = mockDataset()
    expect(dataset.summary.sessionRecordCount).toBe(0)
    expect(dataset.breakdown.every((row) => row.sessionRecordCount === 0)).toBe(true)
    expect(dataset.trend.total.map((point) => point.sessionRecordCount)).toEqual([
      null,
      0,
      0,
      0,
      0,
      0,
      0,
    ])
  })

  it('非零 fixture 的合计与逐桶、逐行明细对账一致', () => {
    const { summary, trend, breakdown } = SESSION_GRANULARITY_DATASET

    const bucketRecords = trend.total.reduce(
      (sum, point) => sum + (point.sessionRecordCount ?? 0),
      0,
    )
    const rowRecords = breakdown.reduce((sum, row) => sum + row.sessionRecordCount, 0)
    expect(summary.sessionRecordCount).toBe(7)
    expect(bucketRecords).toBe(7)
    expect(rowRecords).toBe(7)

    const bucketInput = trend.total.reduce((sum, point) => sum + (point.tokens?.tokInput ?? 0), 0)
    const rowInput = breakdown.reduce((sum, row) => sum + row.tokens.tokInput, 0)
    expect(bucketInput).toBe(summary.tokens.tokInput)
    expect(rowInput).toBe(summary.tokens.tokInput)
  })

  it('会话级来源的行：消息数为 0 而 token 与成本有值', () => {
    const hermes = SESSION_GRANULARITY_DATASET.breakdown.find((row) => row.source === 'hermes')
    expect(hermes?.messageCount).toBe(0)
    expect(hermes?.sessionRecordCount).toBeGreaterThan(0)
    expect(hermes?.tokens.tokInput).toBeGreaterThan(0)
    expect(hermes?.cost.actualSum).toBeGreaterThan(0)
  })

  it('消息数不含会话级来源，所以合计与基准种子一致', () => {
    expect(SESSION_GRANULARITY_DATASET.summary.messageCount).toBe(
      mockDataset().summary.messageCount,
    )
  })
})
