import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { Host } from '@/generated'
import { zh } from '@/i18n/zh'
import { hostsUpdate, triggerRefresh } from '@/lib/ipc'

import { HostList } from './HostList'
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

/** 与 `agentlens_core::host::SUPPORTED_SOURCES` 一致；界面实际值来自 hosts_supported_sources。 */
const SUPPORTED = ['opencode', 'claude-code', 'codex', 'hermes'] as const

const LOCAL_HOST: Host = {
  hostId: 'local-host-000001',
  machineIdHash: 'a'.repeat(64),
  displayName: 'workstation',
  kind: 'local',
  sshTarget: null,
  remoteDataDir: null,
  lastSuccessUtc: null,
  enabledSources: ['opencode'],
}

function rows(host: Host = LOCAL_HOST): HostRowModel[] {
  return [{ host, statuses: [] }]
}

function renderList(host: Host = LOCAL_HOST, supported: readonly string[] = SUPPORTED) {
  const queryClient = new QueryClient({
    defaultOptions: { mutations: { retry: false }, queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <HostList
        rows={rows(host)}
        supportedSources={supported}
        selectedHostId={null}
        onSelect={vi.fn()}
        onRefreshEvent={vi.fn()}
      />
    </QueryClientProvider>,
  )
}

function toggle(source: string) {
  fireEvent.click(screen.getByTestId(`host-source-toggle-local-host-000001-${source}`))
}

function saveButton(): HTMLButtonElement {
  return screen.getByTestId('host-sources-save-local-host-000001') as HTMLButtonElement
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('HostList 采集源勾选', () => {
  it('列出后端导出的全部采集源，并勾上当前已启用的那些', () => {
    renderList()

    for (const source of SUPPORTED) {
      const box = screen.getByTestId(
        `host-source-toggle-local-host-000001-${source}`,
      ) as HTMLInputElement
      expect(box.checked).toBe(source === 'opencode')
    }
    // 无改动时保存按钮不可点，避免一次无意义的 IPC 往返。
    expect(saveButton().disabled).toBe(true)
  })

  it('保存时把完整的启用集合以数组形式传给 hosts_update', async () => {
    vi.mocked(hostsUpdate).mockResolvedValue({
      ...LOCAL_HOST,
      enabledSources: ['opencode', 'codex'],
    })
    renderList()

    toggle('codex')
    expect(saveButton().disabled).toBe(false)
    fireEvent.click(saveButton())

    await waitFor(() => expect(hostsUpdate).toHaveBeenCalledTimes(1))
    expect(vi.mocked(hostsUpdate).mock.calls[0][0]).toEqual({
      hostId: 'local-host-000001',
      displayName: 'workstation',
      kind: 'local',
      sshTarget: null,
      remoteDataDir: null,
      // 显式数组，不是 null：null 的后端语义是「保持库里已存的集合」，等于什么都没改。
      enabledSources: ['opencode', 'codex'],
    })
    expect(await screen.findByTestId('host-sources-saved-local-host-000001')).toBeTruthy()
  })

  it('取消勾选也是真正的写入，而不是被当成「保持原样」', async () => {
    vi.mocked(hostsUpdate).mockResolvedValue({
      ...LOCAL_HOST,
      enabledSources: ['codex'],
    })
    renderList({ ...LOCAL_HOST, enabledSources: ['opencode', 'codex'] })

    toggle('opencode')
    fireEvent.click(saveButton())

    await waitFor(() => expect(hostsUpdate).toHaveBeenCalledTimes(1))
    expect(vi.mocked(hostsUpdate).mock.calls[0][0]).toMatchObject({
      enabledSources: ['codex'],
    })
  })

  it('全部取消勾选被前端拦下，一次 IPC 都不发', () => {
    renderList()

    toggle('opencode')
    fireEvent.click(saveButton())

    expect(screen.getByTestId('host-sources-validation-local-host-000001').textContent).toBe(
      zh.hosts.list.sourcesRequireOne,
    )
    expect(hostsUpdate).not.toHaveBeenCalled()
  })

  it('重新勾上一个源后校验提示消失，保存恢复可用', () => {
    renderList()

    toggle('opencode')
    fireEvent.click(saveButton())
    expect(screen.getByTestId('host-sources-validation-local-host-000001')).toBeTruthy()

    toggle('hermes')

    expect(screen.queryByTestId('host-sources-validation-local-host-000001')).toBeNull()
    expect(saveButton().disabled).toBe(false)
  })

  /** 后端对非法集合报「采集源配置无效」，这条文案必须原样出现在界面上。 */
  it('渲染后端对非法采集源集合的拒绝文案', async () => {
    vi.mocked(hostsUpdate).mockRejectedValue({
      code: 'invalidInput',
      message:
        'host local-host-000001 的采集源配置无效：未知采集源 "codex-next"；可用采集源为 opencode, claude-code, codex, hermes',
      fields: {},
    })
    renderList()

    toggle('codex')
    fireEvent.click(saveButton())

    const panel = await screen.findByTestId('host-sources-error-local-host-000001-message')
    expect(panel.textContent).toContain('采集源配置无效')
    expect(screen.getByTestId('host-sources-error-local-host-000001-code').textContent).toContain(
      'invalidInput',
    )
  })

  /**
   * 已启用但不在导出清单里的源（例如降级到旧版本后残留的键）仍要渲染出来：把它从勾选框里
   * 抹掉，下一次保存就会静默地把它禁用。
   */
  it('保留已启用但不在导出清单中的采集源', () => {
    renderList({ ...LOCAL_HOST, enabledSources: ['opencode', 'legacy-source'] }, ['opencode'])

    const legacy = screen.getByTestId(
      'host-source-toggle-local-host-000001-legacy-source',
    ) as HTMLInputElement
    expect(legacy.checked).toBe(true)
  })

  /** 导出清单读取失败时不渲染勾选块，而不是渲染一个空的、点不动的面板。 */
  it('导出清单为空且主机也没有启用源时不渲染勾选块', () => {
    renderList({ ...LOCAL_HOST, enabledSources: [] }, [])

    expect(screen.queryByTestId('host-sources-edit-local-host-000001')).toBeNull()
  })
})

/**
 * 新启用一个源之后，那一轮采集必须对用户可见。
 *
 * 后端本来就会自动跑（本机 Auto 槽新建时 `next_due_utc` 是 `None`，语义是「下一个 tick
 * 立即到期」，tick 每秒一次），但刷新状态只在 invalidate 时重取，所以一次几秒到几分钟的全量
 * 扫描期间界面可能一直停在「状态未知」，看起来就像勾选没生效 —— 用户报的就是这个现象。
 */
describe('HostList 新启用采集源后的首次采集', () => {
  it('新增一个源后立即触发一轮采集，并显示进行中的状态', async () => {
    vi.mocked(hostsUpdate).mockResolvedValue({
      ...LOCAL_HOST,
      enabledSources: ['opencode', 'codex'],
    })
    vi.mocked(triggerRefresh).mockResolvedValue([
      { outcome: 'started', host_id: 'local-host-000001', source: 'codex', started_at_utc: 1 },
    ])
    renderList()

    toggle('codex')
    fireEvent.click(saveButton())

    await waitFor(() => expect(triggerRefresh).toHaveBeenCalledTimes(1))
    expect(vi.mocked(triggerRefresh).mock.calls[0][0]).toBe('local-host-000001')
    const scan = await screen.findByTestId('host-sources-scan-local-host-000001')
    expect(scan.textContent).toBe(zh.hosts.list.sourcesScanStarted)
  })

  /** 关掉一个源不该开始扫描：那一轮既无新数据可采，又会白跑一次全量。 */
  it('只取消勾选时不触发采集', async () => {
    vi.mocked(hostsUpdate).mockResolvedValue({
      ...LOCAL_HOST,
      enabledSources: ['opencode'],
    })
    renderList({ ...LOCAL_HOST, enabledSources: ['opencode', 'codex'] })

    toggle('codex')
    fireEvent.click(saveButton())

    await waitFor(() => expect(hostsUpdate).toHaveBeenCalledTimes(1))
    expect(triggerRefresh).not.toHaveBeenCalled()
    expect(screen.queryByTestId('host-sources-scan-local-host-000001')).toBeNull()
  })

  it('自动轮已经在跑时说明新源会被同一轮采到，而不是谎报已开始', async () => {
    vi.mocked(hostsUpdate).mockResolvedValue({
      ...LOCAL_HOST,
      enabledSources: ['opencode', 'codex'],
    })
    vi.mocked(triggerRefresh).mockResolvedValue([
      {
        outcome: 'alreadyRunning',
        host_id: 'local-host-000001',
        source: 'codex',
        started_at_utc: 1,
      },
    ])
    renderList()

    toggle('codex')
    fireEvent.click(saveButton())

    const scan = await screen.findByTestId('host-sources-scan-local-host-000001')
    expect(scan.textContent).toBe(zh.hosts.list.sourcesScanAlreadyRunning)
  })

  /** 首次采集失败必须说出来：这正是「勾了但没数据」当初无从排查的原因。 */
  it('首次采集失败时把后端错误渲染出来', async () => {
    vi.mocked(hostsUpdate).mockResolvedValue({
      ...LOCAL_HOST,
      enabledSources: ['opencode', 'codex'],
    })
    vi.mocked(triggerRefresh).mockRejectedValue({
      code: 'refresh',
      message: 'archive ingest record origin bak does not match round origin live',
      fields: {},
    })
    renderList()

    toggle('codex')
    fireEvent.click(saveButton())

    const panel = await screen.findByTestId('host-sources-scan-error-local-host-000001-message')
    expect(panel.textContent).toContain('origin bak')
  })
})
