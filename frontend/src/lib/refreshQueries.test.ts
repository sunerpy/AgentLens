import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const listen = vi.fn()

vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) => listen(...args),
}))

const { REFRESH_COMPLETED_EVENT, subscribeRefreshCompletions } = await import('./refreshQueries')
const { REFRESH_STATUS_QUERY_KEY } = await import('./queryKeys')

let client: QueryClient

beforeEach(() => {
  listen.mockReset()
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
})

afterEach(() => {
  client.clear()
})

describe('refreshQueries/subscribeRefreshCompletions', () => {
  it('订阅所有刷新轮次都会发出的完成事件', async () => {
    listen.mockResolvedValue(() => undefined)

    await subscribeRefreshCompletions(client)

    expect(REFRESH_COMPLETED_EVENT).toBe('agentlens://refresh-completed')
    expect(listen).toHaveBeenCalledWith(REFRESH_COMPLETED_EVENT, expect.any(Function))
  })

  it('无归档变化的完成事件仍会重取永久新鲜的刷新状态', async () => {
    let handler: (() => void) | undefined
    listen.mockImplementation(async (_event: unknown, callback: () => void) => {
      handler = callback
      return () => undefined
    })
    const fetchStatus = vi.fn().mockResolvedValue([{ hostId: 'local-host-000001' }])
    const observer = new QueryObserver(client, {
      queryKey: REFRESH_STATUS_QUERY_KEY,
      queryFn: fetchStatus,
      staleTime: Infinity,
    })
    const unsubscribe = observer.subscribe(() => undefined)

    await vi.waitFor(() => expect(fetchStatus).toHaveBeenCalledTimes(1))
    await subscribeRefreshCompletions(client)
    handler?.()

    await vi.waitFor(() => expect(fetchStatus).toHaveBeenCalledTimes(2))
    unsubscribe()
  })

  it('拿不到 Tauri 事件桥时返回 null', async () => {
    listen.mockRejectedValue(new Error('window.__TAURI_INTERNALS__ is undefined'))

    await expect(subscribeRefreshCompletions(client)).resolves.toBeNull()
  })
})
