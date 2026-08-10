/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Re-render accounting for the host list.
 *
 * The user-visible defect this locks down: while a refresh round is in flight the hosts view
 * receives one `RefreshEvent` per `(host_id, source)` slot, each of which rewrites the
 * `REFRESH_STATUS_QUERY_KEY` cache entry. `joinHostStatus` then rebuilds **every** row wrapper
 * object, so before this file existed a single slot's state change re-rendered every row's whole
 * subtree — headline badges, per-source list, the source picker with its two `useMutation`
 * hooks. Scrolling during a refresh dropped frames because React reconciliation owned the main
 * thread, not because collection blocked it (collection runs on a Rust worker thread).
 *
 * ### How a render is counted
 *
 * `rowStateKey` is called exactly once per `HostRow` body execution and `hostStateKey` exactly
 * once per `SourceRow` body execution, and both receive the status objects the row was rendered
 * from — so `hostId` identifies which row ran. Both are therefore deliberately called
 * **directly** rather than through `useMemo`: a `useMemo` around a three-element `.some()` scan
 * would cost more than it saves *and* would make render counting unobservable (a memo hit
 * skips the call whether or not the component re-rendered).
 *
 * The counts below are the whole point of the file, so they are asserted as exact numbers
 * rather than "fewer than before".
 */
import { cleanup, render } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { Host, SourceStatus } from '@/generated'

import { HostList } from './HostList'
import * as hostsModel from './hostsModel'
import type { HostRowModel } from './hostsModel'

vi.mock('@/lib/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/ipc')>()),
  hostsUpdate: vi.fn(),
  hostsDelete: vi.fn(),
  triggerRefresh: vi.fn(),
}))

vi.mock('@/app/reportRange', () => ({
  useReportRange: () => ({ timezone: 'UTC' }),
}))

vi.mock('./hostsModel', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./hostsModel')>()
  return {
    ...actual,
    rowStateKey: vi.fn(actual.rowStateKey),
    hostStateKey: vi.fn(actual.hostStateKey),
  }
})

/** Stable prop references, so a re-render of the list never invalidates a row on identity. */
const SUPPORTED = ['opencode', 'claude-code'] as const
const ON_SELECT = vi.fn()
const ON_REFRESH_EVENT = vi.fn()

function host(hostId: string, sources: readonly string[]): Host {
  return {
    hostId,
    machineIdHash: hostId.padEnd(64, '0'),
    displayName: hostId,
    kind: 'ssh',
    sshTarget: `ci@${hostId}`,
    remoteDataDir: '/srv/opencode',
    lastSuccessUtc: 1_700_000_000_000,
    enabledSources: [...sources],
  }
}

function status(hostId: string, source: string, state: SourceStatus['state']): SourceStatus {
  return {
    hostId,
    source,
    displayName: hostId,
    kind: 'ssh',
    state,
    trigger: 'auto',
    lastError: null,
    lastSuccessUtc: 1_700_000_000_000,
    lastCompletedUtc: 1_700_000_000_000,
    lastDurationMs: 900,
    intervalMs: 600_000,
    nextDueUtc: null,
    interrupted: false,
    cursorTimeUpdated: null,
  }
}

const HOST_IDS = ['host-a', 'host-b', 'host-c'] as const

/** Every host carries two slots, matching the seeded `(host_id, source)` fixture. */
const HOSTS: readonly Host[] = HOST_IDS.map((hostId) => host(hostId, ['opencode', 'claude-code']))

const IDLE_STATUSES: readonly SourceStatus[] = HOST_IDS.flatMap((hostId) => [
  status(hostId, 'opencode', { state: 'idle' }),
  status(hostId, 'claude-code', { state: 'idle' }),
])

/**
 * Rebuilds the row wrappers exactly the way `joinHostStatus` does — **new wrapper objects and
 * new status arrays on every call** — while preserving the identity of every individual
 * `SourceStatus` the caller did not replace. That is the real shape of a refresh event: the
 * scheduler rewrites one slot, TanStack's structural sharing keeps the rest, and the join
 * still hands the list brand-new arrays.
 */
function join(statuses: readonly SourceStatus[]): HostRowModel[] {
  return HOSTS.map((each) => ({
    host: each,
    statuses: statuses.filter((candidate) => candidate.hostId === each.hostId),
  }))
}

function renderList(rows: readonly HostRowModel[]) {
  const queryClient = new QueryClient({
    defaultOptions: { mutations: { retry: false }, queries: { retry: false } },
  })
  const tree = (next: readonly HostRowModel[]) => (
    <QueryClientProvider client={queryClient}>
      <HostList
        rows={next}
        supportedSources={SUPPORTED}
        selectedHostId={null}
        onSelect={ON_SELECT}
        onRefreshEvent={ON_REFRESH_EVENT}
      />
    </QueryClientProvider>
  )
  const view = render(tree(rows))
  return (next: readonly HostRowModel[]) => {
    view.rerender(tree(next))
  }
}

/** `hostId → number of `HostRow` bodies executed`, read off the `rowStateKey` probe. */
function rowRenders(): Record<string, number> {
  const counts: Record<string, number> = {}
  for (const [statuses] of vi.mocked(hostsModel.rowStateKey).mock.calls) {
    const hostId = statuses[0]?.hostId ?? '<no-status>'
    counts[hostId] = (counts[hostId] ?? 0) + 1
  }
  return counts
}

/** `hostId → number of `SourceRow` bodies executed`, read off the `hostStateKey` probe. */
function sourceRenders(): Record<string, number> {
  const counts: Record<string, number> = {}
  for (const [each] of vi.mocked(hostsModel.hostStateKey).mock.calls) {
    const hostId = each?.hostId ?? '<no-status>'
    counts[hostId] = (counts[hostId] ?? 0) + 1
  }
  return counts
}

function clearProbes() {
  vi.mocked(hostsModel.rowStateKey).mockClear()
  vi.mocked(hostsModel.hostStateKey).mockClear()
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('HostList 刷新期间的重渲染范围', () => {
  it('一个采集源转为进行中时，只有该主机那一行重渲染', () => {
    const rerender = renderList(join(IDLE_STATUSES))
    expect(rowRenders()).toEqual({ 'host-a': 1, 'host-b': 1, 'host-c': 1 })
    clearProbes()

    // 一次 `started` 事件：只有 host-b 的 opencode 槽被替换，其余 SourceStatus 保持同一对象。
    const running = IDLE_STATUSES.map((each) =>
      each.hostId === 'host-b' && each.source === 'opencode'
        ? status('host-b', 'opencode', { state: 'running' })
        : each,
    )
    rerender(join(running))

    expect(rowRenders()).toEqual({ 'host-b': 1 })
    // 1, not 2: host-b 的行虽然重渲染了，但 claude-code 槽的 status 是同一个对象，
    // `SourceRow` 自己的 memo 把它也挡住了。
    expect(sourceRenders()).toEqual({ 'host-b': 1 })
  })

  it('整轮刷新的三个事件加起来也只重渲染被改动的那些行', () => {
    let statuses: readonly SourceStatus[] = IDLE_STATUSES
    const rerender = renderList(join(statuses))
    clearProbes()

    // host-a 的两个槽各推一次 started，host-c 完全没动。
    for (const source of ['opencode', 'claude-code']) {
      statuses = statuses.map((each) =>
        each.hostId === 'host-a' && each.source === source
          ? status('host-a', source, { state: 'running' })
          : each,
      )
      rerender(join(statuses))
    }

    expect(rowRenders()).toEqual({ 'host-a': 2 })
    // 每次事件只有被改动的那个槽重渲染，所以是 2 而不是 2 行 × 2 槽 = 4。
    expect(sourceRenders()).toEqual({ 'host-a': 2 })
  })

  it('父组件重渲染但数据一字未改时，一行都不重渲染', () => {
    const statuses = IDLE_STATUSES
    const rerender = renderList(join(statuses))
    clearProbes()

    // `joinHostStatus` 每次都产出新的包装对象与新的数组，行却不该因此重渲染。
    rerender(join(statuses))
    rerender(join(statuses))

    expect(rowRenders()).toEqual({})
    expect(sourceRenders()).toEqual({})
  })

  it('选中态变化只重渲染受影响的那一行', () => {
    const queryClient = new QueryClient({
      defaultOptions: { mutations: { retry: false }, queries: { retry: false } },
    })
    const rows = join(IDLE_STATUSES)
    const tree = (selectedHostId: string | null) => (
      <QueryClientProvider client={queryClient}>
        <HostList
          rows={rows}
          supportedSources={SUPPORTED}
          selectedHostId={selectedHostId}
          onSelect={ON_SELECT}
          onRefreshEvent={ON_REFRESH_EVENT}
        />
      </QueryClientProvider>
    )
    const view = render(tree(null))
    clearProbes()

    view.rerender(tree('host-c'))

    expect(rowRenders()).toEqual({ 'host-c': 1 })
  })
})
