import { readFileSync } from 'node:fs'
import path from 'node:path'

import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

/**
 * 归档查询族（F3 DEFECT-2 的修复）。这个族存在的唯一原因是一次真实生产缺陷：四个聚合查询
 * 各自持有互不相关的 query key，任何刷新轮都无法把它们作为一组失效，于是 155k 行提交完成后
 * 总览仍在服务采集前的缓存——全新安装上永远显示 0。
 *
 * 因此本套用例的断言对象是**真实 QueryClient 的缓存状态**（哪些 key 被标记为 invalidated），
 * 不是"invalidateQueries 被调了几次"这类实现细节。
 *
 * `@tauri-apps/api/event` 是进程外的桥，属于合法的边界 mock：`listen` 被替换成可控实现，
 * 但事件触发后的可观测结果仍由真实 QueryClient 给出。
 */
const listen = vi.fn()

vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) => listen(...args),
}))

const {
  ARCHIVE_COMMITTED_EVENT,
  ARCHIVE_QUERY_KEY,
  ARCHIVE_QUERY_KEY_ROOT,
  archiveQueryKey,
  invalidateArchiveQueries,
  subscribeArchiveCommits,
} = await import('./archiveQueries')
const { REFRESH_STATUS_QUERY_KEY } = await import('./queryKeys')

/** 与 `src/lib/ipc.ts` 里四个聚合读一一对应的 key 形态。 */
const AGGREGATE_KEYS = [
  archiveQueryKey('summary', { startDate: '2024-01-01' }, 'UTC'),
  archiveQueryKey('trend', { startDate: '2024-01-01' }, 'UTC', 'day'),
  archiveQueryKey('breakdown', { startDate: '2024-01-01' }, { timezone: 'UTC' }),
  archiveQueryKey('messages', { source: 'local' }, 50, 0),
] as const

/** 不属于归档族的 key，用来证明失效有边界、不是"全清"。 */
const SETTINGS_KEY = ['settings'] as const
const HOSTS_KEY = ['hosts', 'list'] as const

function seed(client: QueryClient, keys: readonly (readonly unknown[])[]): void {
  for (const key of keys) client.setQueryData(key, { seeded: true })
}

function invalidatedKeys(client: QueryClient): string[] {
  return client
    .getQueryCache()
    .getAll()
    .filter((query) => query.state.isInvalidated)
    .map((query) => JSON.stringify(query.queryKey))
    .sort()
}

let client: QueryClient

beforeEach(() => {
  listen.mockReset()
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
})

afterEach(() => {
  client.clear()
})

describe('archiveQueries/archiveQueryKey', () => {
  it('每个 key 都以同一个根段开头', () => {
    for (const key of AGGREGATE_KEYS) {
      expect(key[0]).toBe(ARCHIVE_QUERY_KEY_ROOT)
      expect(key[0]).toBe('archive')
    }
  })

  it('段按调用顺序原样追加在根段之后', () => {
    expect(archiveQueryKey('summary')).toEqual(['archive', 'summary'])
    expect(archiveQueryKey('trend', 'day')).toEqual(['archive', 'trend', 'day'])
  })

  it('无段调用退化成失效前缀本身', () => {
    expect(archiveQueryKey()).toEqual([...ARCHIVE_QUERY_KEY])
  })

  it('结构化段保持引用等价的内容（TanStack 按值哈希 key）', () => {
    const range = { startDate: '2024-01-01', endDateExclusive: '2024-01-08' }
    expect(archiveQueryKey('summary', range)).toEqual(['archive', 'summary', range])
  })

  it('相同入参产出结构相等的 key（缓存身份稳定）', () => {
    expect(archiveQueryKey('summary', { tz: 'UTC' })).toEqual(
      archiveQueryKey('summary', { tz: 'UTC' }),
    )
  })

  it('不同入参产出不同 key（不会互相串台）', () => {
    expect(archiveQueryKey('summary', 'UTC')).not.toEqual(
      archiveQueryKey('summary', 'Asia/Shanghai'),
    )
    expect(archiveQueryKey('summary')).not.toEqual(archiveQueryKey('trend'))
  })

  it('ARCHIVE_QUERY_KEY 就是根段单元素数组，可作前缀匹配', () => {
    expect([...ARCHIVE_QUERY_KEY]).toEqual(['archive'])
  })
})

describe('archiveQueries/invalidateArchiveQueries', () => {
  it('一次调用命中整个族的四个聚合查询', async () => {
    seed(client, [...AGGREGATE_KEYS])
    expect(invalidatedKeys(client)).toEqual([])

    await invalidateArchiveQueries(client)

    expect(invalidatedKeys(client)).toEqual(AGGREGATE_KEYS.map((key) => JSON.stringify(key)).sort())
  })

  it('不波及族外的 key（settings / hosts 仍是新鲜的）', async () => {
    seed(client, [...AGGREGATE_KEYS, SETTINGS_KEY, HOSTS_KEY])

    await invalidateArchiveQueries(client)

    expect(client.getQueryState(SETTINGS_KEY)?.isInvalidated).toBe(false)
    expect(client.getQueryState(HOSTS_KEY)?.isInvalidated).toBe(false)
  })

  it('缓存里数据仍在，只是被标记过期（不会闪空）', async () => {
    seed(client, [AGGREGATE_KEYS[0]])

    await invalidateArchiveQueries(client)

    expect(client.getQueryData(AGGREGATE_KEYS[0])).toEqual({ seeded: true })
    expect(client.getQueryState(AGGREGATE_KEYS[0])?.isInvalidated).toBe(true)
  })

  it('空缓存上调用是安全的 no-op', async () => {
    await expect(invalidateArchiveQueries(client)).resolves.toBeUndefined()
    expect(invalidatedKeys(client)).toEqual([])
  })
})

describe('archiveQueries/ARCHIVE_COMMITTED_EVENT', () => {
  it('字面量就是 Rust 侧发出的那根线', () => {
    expect(ARCHIVE_COMMITTED_EVENT).toBe('agentlens://archive-committed')
  })

  /**
   * 事件名是**线上字符串**而不是 ts-rs 生成的 DTO，两侧各写一份，没有编译期约束。
   * 改错一侧只会让自动刷新静默失效——界面照常渲染、数字永远不更新——正好是单测该拦的东西。
   * 因此这里直接读 Rust 源码做漂移门禁，而不是把同一个字面量抄第三遍。
   */
  it('与 src-tauri/src/state.rs 的 EVENT_ARCHIVE_COMMITTED 完全一致', () => {
    const statePath = path.resolve(import.meta.dirname, '../../../src-tauri/src/state.rs')
    const source = readFileSync(statePath, 'utf8')
    const match = /pub const EVENT_ARCHIVE_COMMITTED: &str = "([^"]+)";/.exec(source)
    expect(
      match,
      `未能在 ${statePath} 中定位 EVENT_ARCHIVE_COMMITTED 常量；若 Rust 侧改名，请同步本用例`,
    ).not.toBeNull()
    expect(match?.[1]).toBe(ARCHIVE_COMMITTED_EVENT)
  })
})

describe('archiveQueries/subscribeArchiveCommits', () => {
  it('订阅的是归档提交事件本身', async () => {
    listen.mockResolvedValue(() => undefined)

    await subscribeArchiveCommits(client)

    expect(listen).toHaveBeenCalledTimes(1)
    expect(listen.mock.calls[0]?.[0]).toBe('agentlens://archive-committed')
  })

  it('事件到达即让整个族过期（自动调度轮也能刷新，不只手动按钮）', async () => {
    let handler: (() => void) | undefined
    listen.mockImplementation(async (_event: unknown, callback: () => void) => {
      handler = callback
      return () => undefined
    })
    seed(client, [...AGGREGATE_KEYS])

    await subscribeArchiveCommits(client)
    expect(handler).toBeTypeOf('function')
    handler?.()
    // invalidateQueries 内部是异步的，让微任务队列跑完再看缓存。
    await Promise.resolve()
    await Promise.resolve()

    expect(invalidatedKeys(client)).toEqual(AGGREGATE_KEYS.map((key) => JSON.stringify(key)).sort())
  })

  it('归档提交后会重取已挂载的刷新状态，即使该查询永久新鲜', async () => {
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
    await subscribeArchiveCommits(client)
    handler?.()

    await vi.waitFor(() => expect(fetchStatus).toHaveBeenCalledTimes(2))
    unsubscribe()
  })

  it('拿不到 Tauri 事件桥时返回 null，不让纯浏览器预览启动失败', async () => {
    listen.mockRejectedValue(new Error('window.__TAURI_INTERNALS__ is undefined'))

    await expect(subscribeArchiveCommits(client)).resolves.toBeNull()
  })

  it('订阅成功时把 unlisten 原样交还给调用方', async () => {
    const unlisten = () => undefined
    listen.mockResolvedValue(unlisten)

    await expect(subscribeArchiveCommits(client)).resolves.toBe(unlisten)
  })
})
