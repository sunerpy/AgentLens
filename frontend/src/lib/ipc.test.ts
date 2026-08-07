import { readFileSync } from 'node:fs'
import path from 'node:path'

import { beforeEach, describe, expect, it, vi } from 'vitest'

import type {
  AggregateFilters,
  AppSettings,
  BreakdownDimensions,
  DateRange,
  HostCreateInput,
  HostUpdateInput,
  IpcError,
  MessageFilters,
  PriceTable,
} from '@/generated'

/**
 * `@/lib/ipc` 的两块纯逻辑：错误收窄，以及**每个 wrapper 递给 Tauri 的实参名**。
 *
 * 后者为什么值得单测：Tauri v2 把 Rust 命令参数暴露成一个 camelCase 键的对象
 * （Rust `host_id` → 线上 `hostId`）。键名写错不会有任何编译错误，只会在运行时静默失败——
 * 命令收到 `undefined`，返回一个看起来像"数据为空"的结果。这正是单测该拦的东西，
 * 因此下面逐个断言 `invoke` 的**精确键集合**，实参名逐字取自 `src-tauri/src/commands.rs`。
 */
const invoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  Channel: class<T> {
    onmessage: (message: T) => void = () => undefined
  },
  invoke: (...args: unknown[]) => invoke(...args),
}))

const ipc = await import('./ipc')
const hostsIpc = await import('@/views/hosts/hostsIpc')

const RANGE: DateRange = {
  startDate: '2024-01-01',
  endDateExclusive: '2024-01-08',
  weekStart: 'monday',
}

const FILTERS: AggregateFilters = {
  hostId: null,
  source: null,
  agentKey: null,
  providerId: null,
  modelId: null,
}

const MESSAGE_FILTERS: MessageFilters = {
  range: RANGE,
  timezone: 'UTC',
  hostId: null,
  source: null,
  agentKey: null,
  providerId: null,
  modelId: null,
  isIncomplete: null,
}

const SETTINGS: AppSettings = { values: { 'report.timezone': 'UTC' } }

const PRICES: PriceTable = { schemaVersion: 1, entries: [], extra: {} }

/** 单次调用的实参快照：命令名 + 载荷键集合（排序后）+ 载荷本身。 */
function callShape(): { command: unknown; keys: string[]; payload: Record<string, unknown> } {
  expect(invoke).toHaveBeenCalledTimes(1)
  const [command, payload] = invoke.mock.calls[0] as [unknown, Record<string, unknown> | undefined]
  return {
    command,
    keys: payload === undefined ? [] : Object.keys(payload).sort(),
    payload: payload ?? {},
  }
}

beforeEach(() => {
  invoke.mockReset()
  invoke.mockResolvedValue(undefined)
})

describe('ipc/isIpcError', () => {
  it('认得结构完整的 IpcError', () => {
    expect(ipc.isIpcError({ code: 'notFound', message: '找不到主机', fields: {} })).toBe(true)
    expect(ipc.isIpcError({ code: 'invalidRange', message: 'x' })).toBe(true)
  })

  it('九个合法 code 全部认得', () => {
    const codes: IpcError['code'][] = [
      'invalidInput',
      'invalidRange',
      'invalidTimezone',
      'notFound',
      'conflict',
      'database',
      'pricing',
      'refresh',
      'internal',
    ]
    for (const code of codes) {
      expect(ipc.isIpcError({ code, message: 'x' })).toBe(true)
    }
  })

  it('code 不在白名单内即拒绝（防止后端随手新造 code 混过来）', () => {
    expect(ipc.isIpcError({ code: 'somethingNew', message: 'x' })).toBe(false)
    expect(ipc.isIpcError({ code: '', message: 'x' })).toBe(false)
  })

  it('普通 Error 不是 IpcError——它有 string message 但没有 code', () => {
    expect(ipc.isIpcError(new Error('boom'))).toBe(false)
    expect(ipc.isIpcError(new TypeError('boom'))).toBe(false)
  })

  it('缺字段、错类型、非对象一律拒绝', () => {
    expect(ipc.isIpcError({ code: 'notFound' })).toBe(false)
    expect(ipc.isIpcError({ message: 'x' })).toBe(false)
    expect(ipc.isIpcError({ code: 'notFound', message: 42 })).toBe(false)
    expect(ipc.isIpcError({ code: 404, message: 'x' })).toBe(false)
    expect(ipc.isIpcError(null)).toBe(false)
    expect(ipc.isIpcError(undefined)).toBe(false)
    expect(ipc.isIpcError('notFound')).toBe(false)
    expect(ipc.isIpcError(42)).toBe(false)
    expect(ipc.isIpcError([])).toBe(false)
    expect(ipc.isIpcError([{ code: 'notFound', message: 'x' }])).toBe(false)
  })
})

describe('ipc/toIpcError', () => {
  it('原样保留 IpcError 的 code 与 message，缺失的 fields 补成空对象', () => {
    expect(ipc.toIpcError({ code: 'notFound', message: '找不到主机' })).toEqual({
      code: 'notFound',
      message: '找不到主机',
      fields: {},
    })
  })

  it('保留 fields 里的后端补充信息（remediation / variant 就靠它传）', () => {
    expect(
      ipc.toIpcError({
        code: 'refresh',
        message: 'ssh 认证失败',
        fields: { remediation: '请选择密钥文件', variant: 'authFailed' },
      }).fields,
    ).toEqual({ remediation: '请选择密钥文件', variant: 'authFailed' })
  })

  it('普通 Error 收窄成 internal + 原始 message', () => {
    expect(ipc.toIpcError(new Error('boom'))).toEqual({
      code: 'internal',
      message: 'boom',
      fields: {},
    })
  })

  it('字符串直接当 message', () => {
    expect(ipc.toIpcError('裸字符串错误')).toEqual({
      code: 'internal',
      message: '裸字符串错误',
      fields: {},
    })
  })

  it('null / 普通对象走 JSON 序列化，不会漏出 [object Object]', () => {
    expect(ipc.toIpcError(null).message).toBe('null')
    expect(ipc.toIpcError({}).message).toBe('{}')
    expect(ipc.toIpcError({ nested: { a: 1 } }).message).toBe('{"nested":{"a":1}}')
    expect(ipc.toIpcError([1, 2]).message).toBe('[1,2]')
    for (const value of [null, {}, [1, 2], 42]) {
      expect(ipc.toIpcError(value).message).not.toContain('[object Object]')
    }
  })

  it('undefined 落成空 message 而不是字符串 "undefined"', () => {
    // JSON.stringify(undefined) 返回 undefined 本身，`?? ''` 就是为了兜住这一步。
    expect(ipc.toIpcError(undefined)).toEqual({ code: 'internal', message: '', fields: {} })
  })

  it('返回值恒满足 isIpcError（收窄是幂等的）', () => {
    for (const value of [
      null,
      undefined,
      'x',
      42,
      {},
      new Error('e'),
      { code: 'bad', message: 1 },
    ]) {
      const narrowed = ipc.toIpcError(value)
      expect(ipc.isIpcError(narrowed)).toBe(true)
      expect(ipc.toIpcError(narrowed)).toEqual(narrowed)
    }
  })
})

describe('ipc/聚合查询的实参名', () => {
  it('get_summary 递 { range, tz, filters }', async () => {
    await ipc.getSummary(RANGE, 'Asia/Shanghai', FILTERS)
    const { command, keys, payload } = callShape()
    expect(command).toBe('get_summary')
    expect(keys).toEqual(['filters', 'range', 'tz'])
    expect(payload.range).toBe(RANGE)
    expect(payload.tz).toBe('Asia/Shanghai')
    expect(payload.filters).toBe(FILTERS)
  })

  it('get_trend 递 { range, tz, granularity, filters }', async () => {
    await ipc.getTrend(RANGE, 'UTC', 'day', FILTERS)
    const { command, keys, payload } = callShape()
    expect(command).toBe('get_trend')
    expect(keys).toEqual(['filters', 'granularity', 'range', 'tz'])
    expect(payload.granularity).toBe('day')
  })

  it('get_trend 的 filters 默认显式传 null，而不是省略键', async () => {
    // Rust 侧是 Option<AggregateFilters>；省略键与传 null 在 serde 上都能过，但显式 null
    // 才让"无筛选"这件事在线上可见，也让键集合恒定。
    await ipc.getTrend(RANGE, 'UTC', 'hour')
    const { keys, payload } = callShape()
    expect(keys).toEqual(['filters', 'granularity', 'range', 'tz'])
    expect(payload.filters).toBeNull()
  })

  it('get_breakdown 递 { range, dims }', async () => {
    const dims: BreakdownDimensions = {
      timezone: 'UTC',
      filters: FILTERS,
      expandVariant: false,
    }
    await ipc.getBreakdown(RANGE, dims)
    const { command, keys, payload } = callShape()
    expect(command).toBe('get_breakdown')
    expect(keys).toEqual(['dims', 'range'])
    expect(payload.dims).toBe(dims)
  })

  it('query_messages 递 { filters, limit, offset }', async () => {
    await ipc.queryMessages(MESSAGE_FILTERS, 50, 100)
    const { command, keys, payload } = callShape()
    expect(command).toBe('query_messages')
    expect(keys).toEqual(['filters', 'limit', 'offset'])
    expect(payload.limit).toBe(50)
    expect(payload.offset).toBe(100)
  })
})

describe('ipc/主机与刷新的实参名', () => {
  it('hosts_list 不带任何载荷', async () => {
    await ipc.hostsList()
    const { command, keys } = callShape()
    expect(command).toBe('hosts_list')
    expect(keys).toEqual([])
    expect(invoke.mock.calls[0]).toHaveLength(1)
  })

  it('hosts_get 递 { hostId }——Rust 的 host_id 在线上是 camelCase', async () => {
    await ipc.hostsGet('local')
    const { command, keys, payload } = callShape()
    expect(command).toBe('hosts_get')
    expect(keys).toEqual(['hostId'])
    expect(payload.hostId).toBe('local')
    expect(payload).not.toHaveProperty('host_id')
  })

  it('hosts_create 递 { input }', async () => {
    const input: HostCreateInput = {
      displayName: 'box',
      kind: 'ssh',
      machineIdHash: 'a'.repeat(64),
      sshTarget: 'box',
      remoteDataDir: null,
    }
    await ipc.hostsCreate(input)
    const { command, keys, payload } = callShape()
    expect(command).toBe('hosts_create')
    expect(keys).toEqual(['input'])
    expect(payload.input).toBe(input)
  })

  it('hosts_update 递 { input }', async () => {
    const input: HostUpdateInput = {
      hostId: 'local',
      displayName: 'this machine',
      kind: 'local',
      sshTarget: null,
      remoteDataDir: null,
    }
    await ipc.hostsUpdate(input)
    const { command, keys } = callShape()
    expect(command).toBe('hosts_update')
    expect(keys).toEqual(['input'])
  })

  it('hosts_delete 递 { hostId }', async () => {
    await ipc.hostsDelete('remote-1')
    const { command, keys, payload } = callShape()
    expect(command).toBe('hosts_delete')
    expect(keys).toEqual(['hostId'])
    expect(payload.hostId).toBe('remote-1')
  })

  it('trigger_refresh 递 { hostId, onEvent } 并把 Channel 消息交给调用方', async () => {
    const onEvent = vi.fn()
    await ipc.triggerRefresh('local', onEvent)
    const { command, keys, payload } = callShape()
    expect(command).toBe('trigger_refresh')
    expect(keys).toEqual(['hostId', 'onEvent'])
    expect(payload.hostId).toBe('local')

    const channel = payload.onEvent as { onmessage: (message: unknown) => void }
    const event = { event: 'finished', data: { hostId: 'local', status: null } }
    channel.onmessage(event)
    expect(onEvent).toHaveBeenCalledWith(event)
  })

  it('get_refresh_status 不带任何载荷', async () => {
    await ipc.getRefreshStatus()
    const { command, keys } = callShape()
    expect(command).toBe('get_refresh_status')
    expect(keys).toEqual([])
  })
})

describe('ipc/设置与价格的实参名', () => {
  it('get_settings / price_catalog_get / prices_get 不带载荷', async () => {
    await ipc.getSettings()
    expect(callShape().command).toBe('get_settings')
    invoke.mockReset()
    invoke.mockResolvedValue(undefined)
    await ipc.priceCatalogGet()
    expect(callShape().command).toBe('price_catalog_get')
    invoke.mockReset()
    invoke.mockResolvedValue(undefined)
    await ipc.pricesGet()
    const { command, keys } = callShape()
    expect(command).toBe('prices_get')
    expect(keys).toEqual([])
  })

  it('set_settings 递 { settings }', async () => {
    await ipc.setSettings(SETTINGS)
    const { command, keys, payload } = callShape()
    expect(command).toBe('set_settings')
    expect(keys).toEqual(['settings'])
    expect(payload.settings).toBe(SETTINGS)
  })

  it('prices_set 递 { prices }', async () => {
    await ipc.pricesSet(PRICES)
    const { command, keys, payload } = callShape()
    expect(command).toBe('prices_set')
    expect(keys).toEqual(['prices'])
    expect(payload.prices).toBe(PRICES)
  })
})

describe('ipc/主机视图侧的五个命令（hostsIpc）', () => {
  it('credential_set 递 { hostId, kind, secret }', async () => {
    await hostsIpc.credentialSet('remote-1', 'password', 's3cret')
    const { command, keys, payload } = callShape()
    expect(command).toBe('credential_set')
    expect(keys).toEqual(['hostId', 'kind', 'secret'])
    expect(payload.hostId).toBe('remote-1')
    expect(payload.kind).toBe('password')
  })

  it('credential_status / credential_delete 递 { hostId, kind }', async () => {
    await hostsIpc.credentialStatus('remote-1', 'password')
    expect(callShape().keys).toEqual(['hostId', 'kind'])
    invoke.mockReset()
    invoke.mockResolvedValue(undefined)
    await hostsIpc.credentialDelete('remote-1', 'passphrase')
    const { command, keys } = callShape()
    expect(command).toBe('credential_delete')
    expect(keys).toEqual(['hostId', 'kind'])
  })

  it('ssh_probe 递 { input, requestId }，取消命令只递 requestId', async () => {
    await hostsIpc.sshProbe(
      { sshTarget: 'box', identityFile: null, remoteDataDir: null },
      'probe-01',
    )
    expect(callShape().keys).toEqual(['input', 'requestId'])
    invoke.mockReset()
    invoke.mockResolvedValue(undefined)
    await hostsIpc.sshProbeCancel('probe-01')
    expect(callShape()).toMatchObject({
      command: 'ssh_probe_cancel',
      keys: ['requestId'],
      payload: { requestId: 'probe-01' },
    })
  })

  it('local_machine_identity 不带载荷', async () => {
    invoke.mockReset()
    invoke.mockResolvedValue(undefined)
    await hostsIpc.localMachineIdentity()
    const { command, keys } = callShape()
    expect(command).toBe('local_machine_identity')
    expect(keys).toEqual([])
  })
})

describe('ipc/命令清单与 Rust 注册表一致', () => {
  it('IPC_COMMANDS 恰好是 16 个且无重复', () => {
    expect(ipc.IPC_COMMANDS).toHaveLength(16)
    expect(new Set(ipc.IPC_COMMANDS).size).toBe(16)
  })

  /**
   * 漂移门禁：命令名是线上字符串，Rust 侧改名不会让前端编译失败，只会在运行时收到
   * "command not found"。这里直接读 `generate_handler!` 的注册表来比对，而不是抄一份清单。
   */
  it('两侧 wrapper 覆盖的命令集合 === src-tauri/src/lib.rs 的 generate_handler 注册表', () => {
    const libPath = path.resolve(import.meta.dirname, '../../../src-tauri/src/lib.rs')
    const source = readFileSync(libPath, 'utf8')
    const registered = [...source.matchAll(/^\s*commands::([a-z0-9_]+),$/gm)].map(
      (match) => match[1],
    )
    expect(
      registered.length,
      `未能在 ${libPath} 的 generate_handler! 里解析出命令列表`,
    ).toBeGreaterThan(0)

    const wrapped = [...ipc.IPC_COMMANDS, ...hostsIpc.HOSTS_IPC_COMMANDS]
    expect([...wrapped].sort()).toEqual([...registered].sort())
  })
})
