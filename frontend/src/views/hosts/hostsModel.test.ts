import { describe, expect, it } from 'vitest'

import type { Host, SourceStatus } from '@/generated'
import { zh } from '@/i18n/zh'

import {
  composeSshTarget,
  formatKib,
  formatTimestampInZone,
  hostErrorText,
  hostLastSuccessUtc,
  hostStateKey,
  hostStateLabel,
  ipcRemediation,
  ipcVariant,
  isMachineIdHash,
  joinHostStatus,
  needsKeyFileGuidance,
  rowStateKey,
  statusLastSuccessUtc,
} from './hostsModel'

/**
 * 主机视图背后的纯函数。这里值得钉住的两处语义：
 *
 * 1. **错误文案优先取 `SourceState::Error.last_error`**，而不是扁平的 `lastError` 镜像：
 *    前者是刷新轮真正失败时后端写下的权威消息（来自 ssh 传输层时已经是中文处置建议）。
 * 2. **`last_success` 跨失败保留**，所以一台坏掉的主机仍要显示"上次成功于何时"，
 *    而不是退化成"从未成功"。
 */
function host(overrides: Partial<Host> = {}): Host {
  return {
    hostId: 'local',
    machineIdHash: 'a'.repeat(64),
    displayName: '本机',
    kind: 'local',
    sshTarget: null,
    remoteDataDir: null,
    lastSuccessUtc: null,
    enabledSources: ['opencode'],
    ...overrides,
  }
}

function status(overrides: Partial<SourceStatus> = {}): SourceStatus {
  return {
    hostId: 'local',
    source: 'opencode',
    displayName: '本机',
    kind: 'local',
    state: { state: 'idle' },
    trigger: 'auto',
    lastError: null,
    lastSuccessUtc: null,
    lastCompletedUtc: null,
    lastDurationMs: null,
    intervalMs: 300_000,
    nextDueUtc: null,
    interrupted: false,
    cursorTimeUpdated: null,
    ...overrides,
  }
}

describe('hostsModel/joinHostStatus', () => {
  it('按 hostId 关联，顺序跟随 hosts 而不是 statuses', () => {
    const rows = joinHostStatus(
      [host({ hostId: 'a' }), host({ hostId: 'b' })],
      [status({ hostId: 'b' }), status({ hostId: 'a' })],
    )
    expect(rows.map((item) => item.host.hostId)).toEqual(['a', 'b'])
    expect(rows[0].statuses.map((item) => item.hostId)).toEqual(['a'])
  })

  it('缺状态的主机得到空数组而不是被丢弃', () => {
    const rows = joinHostStatus(
      [host({ hostId: 'a' }), host({ hostId: 'b' })],
      [status({ hostId: 'a' })],
    )
    expect(rows).toHaveLength(2)
    expect(rows[1].statuses).toEqual([])
  })

  it('多余的状态不会凭空造出主机行', () => {
    const rows = joinHostStatus([host({ hostId: 'a' })], [status({ hostId: 'ghost' })])
    expect(rows).toHaveLength(1)
    expect(rows[0].statuses).toEqual([])
  })

  it('空输入产出空数组', () => {
    expect(joinHostStatus([], [])).toEqual([])
  })

  /**
   * 调度器键是 (host_id, source)，所以同一 hostId 会有多条状态。它们必须全部保留：
   * 只留一条就会把「Claude Code 出错」藏在「OpenCode 空闲」后面。
   */
  it('同一主机的多个采集源全部保留，且保持给入顺序', () => {
    const rows = joinHostStatus(
      [host({ hostId: 'a', enabledSources: ['opencode', 'claude-code'] })],
      [status({ hostId: 'a', source: 'opencode' }), status({ hostId: 'a', source: 'claude-code' })],
    )
    expect(rows).toHaveLength(1)
    expect(rows[0].statuses.map((item) => item.source)).toEqual(['opencode', 'claude-code'])
  })

  it('多主机多采集源不会互相串到别的行上', () => {
    const rows = joinHostStatus(
      [host({ hostId: 'a' }), host({ hostId: 'b' })],
      [
        status({ hostId: 'a', source: 'opencode' }),
        status({ hostId: 'b', source: 'opencode' }),
        status({ hostId: 'b', source: 'claude-code' }),
      ],
    )
    expect(rows[0].statuses.map((item) => item.source)).toEqual(['opencode'])
    expect(rows[1].statuses.map((item) => item.source)).toEqual(['opencode', 'claude-code'])
  })
})

describe('hostsModel/rowStateKey 多采集源汇总', () => {
  it('无状态归 unknown', () => {
    expect(rowStateKey([])).toBe('unknown')
  })

  it('全部空闲才是空闲', () => {
    expect(rowStateKey([status(), status({ source: 'claude-code' })])).toBe('idle')
  })

  /** 出错优先级最高：一个源坏了，整行就不能显示成健康。 */
  it('任一采集源出错，整行读作出错（哪怕其余源空闲）', () => {
    const failing = status({
      source: 'claude-code',
      state: { state: 'error', last_error: 'x', last_success: null },
    })
    expect(rowStateKey([status(), failing])).toBe('error')
    expect(rowStateKey([failing, status()])).toBe('error')
  })

  it('出错压过刷新中，刷新中压过空闲', () => {
    const running = status({ source: 'claude-code', state: { state: 'running' } })
    const failing = status({
      source: 'codex',
      state: { state: 'error', last_error: 'x', last_success: null },
    })
    expect(rowStateKey([status(), running])).toBe('running')
    expect(rowStateKey([status(), running, failing])).toBe('error')
  })
})

describe('hostsModel/hostStateKey 与 hostStateLabel', () => {
  it('三个真实状态原样透出，缺状态归为 unknown', () => {
    expect(hostStateKey(status({ state: { state: 'idle' } }))).toBe('idle')
    expect(hostStateKey(status({ state: { state: 'running' } }))).toBe('running')
    expect(
      hostStateKey(status({ state: { state: 'error', last_error: 'x', last_success: null } })),
    ).toBe('error')
    expect(hostStateKey(undefined)).toBe('unknown')
  })

  it('标签取自 zh 词典，unknown 落到"状态不可用"', () => {
    expect(hostStateLabel('idle')).toBe(zh.hosts.list.stateIdle)
    expect(hostStateLabel('running')).toBe(zh.hosts.list.stateRunning)
    expect(hostStateLabel('error')).toBe(zh.hosts.list.stateError)
    expect(hostStateLabel('unknown')).toBe(zh.hosts.list.statusUnavailable)
  })
})

describe('hostsModel/hostErrorText', () => {
  it('error 变体里的 last_error 胜过扁平的 lastError 镜像', () => {
    expect(
      hostErrorText(
        status({
          state: { state: 'error', last_error: 'ssh 认证失败，请选择密钥文件', last_success: null },
          lastError: '旧的过期消息',
        }),
      ),
    ).toBe('ssh 认证失败，请选择密钥文件')
  })

  it('非 error 状态回落扁平镜像', () => {
    expect(hostErrorText(status({ state: { state: 'idle' }, lastError: '上一轮的告警' }))).toBe(
      '上一轮的告警',
    )
    expect(hostErrorText(status({ state: { state: 'idle' }, lastError: null }))).toBeNull()
  })

  it('无状态返回 null', () => {
    expect(hostErrorText(undefined)).toBeNull()
  })
})

describe('hostsModel/statusLastSuccessUtc', () => {
  it('失败态取 error 变体里保留的 last_success', () => {
    expect(
      statusLastSuccessUtc(
        status({
          state: { state: 'error', last_error: 'x', last_success: 5_000 },
          lastSuccessUtc: null,
        }),
      ),
    ).toBe(5_000)
  })

  it('失败态且从未成功过时回落扁平镜像', () => {
    expect(
      statusLastSuccessUtc(
        status({
          state: { state: 'error', last_error: 'x', last_success: null },
          lastSuccessUtc: 7_000,
        }),
      ),
    ).toBe(7_000)
  })

  it('正常态直接用扁平镜像', () => {
    expect(statusLastSuccessUtc(status({ lastSuccessUtc: 9_000 }))).toBe(9_000)
    expect(statusLastSuccessUtc(status({ lastSuccessUtc: null }))).toBeNull()
  })
})

describe('hostsModel/hostLastSuccessUtc', () => {
  it('失败态仍显示 error 变体里保留的上次成功时间', () => {
    expect(
      hostLastSuccessUtc({
        host: host({ lastSuccessUtc: 1_000 }),
        statuses: [
          status({
            state: { state: 'error', last_error: 'x', last_success: 5_000 },
            lastSuccessUtc: null,
          }),
        ],
      }),
    ).toBe(5_000)
  })

  it('失败态且从未成功过时继续往下回落，而不是谎报', () => {
    expect(
      hostLastSuccessUtc({
        host: host({ lastSuccessUtc: 1_000 }),
        statuses: [
          status({
            state: { state: 'error', last_error: 'x', last_success: null },
            lastSuccessUtc: null,
          }),
        ],
      }),
    ).toBe(1_000)
  })

  it('正常态优先用 status 的时间戳', () => {
    expect(
      hostLastSuccessUtc({
        host: host({ lastSuccessUtc: 1_000 }),
        statuses: [status({ lastSuccessUtc: 9_000 })],
      }),
    ).toBe(9_000)
  })

  it('无状态时退回 host 自身的时间戳', () => {
    expect(hostLastSuccessUtc({ host: host({ lastSuccessUtc: 42 }), statuses: [] })).toBe(42)
    expect(hostLastSuccessUtc({ host: host(), statuses: [] })).toBeNull()
  })

  /** 「最近成功」是主机层面的属性，多个采集源里取最新的那次，与给入顺序无关。 */
  it('多采集源取最新的一次成功，且不受顺序影响', () => {
    const older = status({ source: 'opencode', lastSuccessUtc: 3_000 })
    const newer = status({ source: 'claude-code', lastSuccessUtc: 8_000 })
    expect(hostLastSuccessUtc({ host: host(), statuses: [older, newer] })).toBe(8_000)
    expect(hostLastSuccessUtc({ host: host(), statuses: [newer, older] })).toBe(8_000)
  })

  it('部分采集源从未成功时忽略它们，而不是当成 0', () => {
    expect(
      hostLastSuccessUtc({
        host: host({ lastSuccessUtc: 1_000 }),
        statuses: [
          status({ source: 'opencode', lastSuccessUtc: null }),
          status({ source: 'claude-code', lastSuccessUtc: 6_000 }),
        ],
      }),
    ).toBe(6_000)
  })

  it('全部采集源都没成功过时才回落 host 自身的时间戳', () => {
    expect(
      hostLastSuccessUtc({
        host: host({ lastSuccessUtc: 1_000 }),
        statuses: [
          status({ source: 'opencode', lastSuccessUtc: null }),
          status({ source: 'claude-code', lastSuccessUtc: null }),
        ],
      }),
    ).toBe(1_000)
  })
})

describe('hostsModel/IpcError 字段抽取', () => {
  it('取出 fields.remediation', () => {
    expect(
      ipcRemediation({ code: 'refresh', message: 'x', fields: { remediation: '请选择密钥文件' } }),
    ).toBe('请选择密钥文件')
  })

  it('缺失、空串、非字符串一律得 null', () => {
    expect(ipcRemediation({ code: 'refresh', message: 'x', fields: {} })).toBeNull()
    expect(
      ipcRemediation({ code: 'refresh', message: 'x', fields: { remediation: '' } }),
    ).toBeNull()
    expect(ipcRemediation(new Error('boom'))).toBeNull()
    expect(ipcRemediation(null)).toBeNull()
    expect(ipcRemediation(undefined)).toBeNull()
    expect(ipcRemediation('字符串错误')).toBeNull()
  })

  it('取出 fields.variant', () => {
    expect(ipcVariant({ code: 'refresh', message: 'x', fields: { variant: 'authFailed' } })).toBe(
      'authFailed',
    )
    expect(ipcVariant({ code: 'refresh', message: 'x', fields: {} })).toBeNull()
    expect(ipcVariant(null)).toBeNull()
  })

  it('只有 authFailed / sshUnavailable 触发密钥文件引导', () => {
    const withVariant = (variant: string) => ({
      code: 'refresh',
      message: 'x',
      fields: { variant },
    })
    expect(needsKeyFileGuidance(withVariant('authFailed'))).toBe(true)
    expect(needsKeyFileGuidance(withVariant('sshUnavailable'))).toBe(true)
    expect(needsKeyFileGuidance(withVariant('timeout'))).toBe(false)
    expect(needsKeyFileGuidance(withVariant(''))).toBe(false)
    expect(needsKeyFileGuidance(new Error('boom'))).toBe(false)
    expect(needsKeyFileGuidance(null)).toBe(false)
  })
})

describe('hostsModel/composeSshTarget', () => {
  it('给了用户名就拼 user@host', () => {
    expect(composeSshTarget('ubuntu', 'box')).toBe('ubuntu@box')
  })

  it('用户名为空或全空白时用裸别名（沿用 ~/.ssh/config）', () => {
    expect(composeSshTarget('', 'box')).toBe('box')
    expect(composeSshTarget('   ', 'box')).toBe('box')
  })

  it('两侧都去掉首尾空白', () => {
    expect(composeSshTarget('  ubuntu  ', '  box  ')).toBe('ubuntu@box')
  })
})

/**
 * 时区必须是**显式入参**。之前这里是 `formatUtcTimestamp`，函数名本身就说明它无条件按 UTC
 * 渲染，于是用户在设置里改了报表时区、列表依旧显示 UTC —— 这正是被修掉的缺陷。
 * 因此本组用例的核心断言是：同一个瞬时，喂不同时区必须得到不同字符串。
 */
describe('hostsModel/formatTimestampInZone', () => {
  const INSTANT = Date.UTC(2024, 0, 15, 23, 30, 45)

  it('UTC 毫秒渲染成可按肉眼排序的 YYYY-MM-DD HH:mm:ss', () => {
    expect(formatTimestampInZone(INSTANT, 'UTC')).toBe('2024-01-15 23:30:45')
    expect(formatTimestampInZone(0, 'UTC')).toBe('1970-01-01 00:00:00')
  })

  it('同一瞬时在不同时区渲染出不同结果（设置改时区必须看得见）', () => {
    expect(formatTimestampInZone(INSTANT, 'UTC')).toBe('2024-01-15 23:30:45')
    // 东八区 +8，已跨到次日。
    expect(formatTimestampInZone(INSTANT, 'Asia/Shanghai')).toBe('2024-01-16 07:30:45')
    // 东京 +9。
    expect(formatTimestampInZone(INSTANT, 'Asia/Tokyo')).toBe('2024-01-16 08:30:45')
    // 纽约 1 月为 -5，仍停在同一天。
    expect(formatTimestampInZone(INSTANT, 'America/New_York')).toBe('2024-01-15 18:30:45')
    // 加德满都 +5:45，用来钉住非整小时偏移。
    expect(formatTimestampInZone(INSTANT, 'Asia/Kathmandu')).toBe('2024-01-16 05:15:45')
  })

  it('同一时区的夏令时前后偏移不同（不是固定加常数）', () => {
    const january = Date.UTC(2024, 0, 15, 12)
    const july = Date.UTC(2024, 6, 15, 12)
    // 纽约 1 月 -5、7 月 -4。
    expect(formatTimestampInZone(january, 'America/New_York')).toBe('2024-01-15 07:00:00')
    expect(formatTimestampInZone(july, 'America/New_York')).toBe('2024-07-15 08:00:00')
    // 上海全年不实行夏令时，两次都是 +8。
    expect(formatTimestampInZone(january, 'Asia/Shanghai')).toBe('2024-01-15 20:00:00')
    expect(formatTimestampInZone(july, 'Asia/Shanghai')).toBe('2024-07-15 20:00:00')
  })

  it('非法时区名回落 UTC 而不是抛错', () => {
    expect(formatTimestampInZone(INSTANT, 'Not/AZone')).toBe('2024-01-15 23:30:45')
    expect(formatTimestampInZone(INSTANT, '')).toBe('2024-01-15 23:30:45')
  })

  it('重复调用同一时区结果一致（缓存不得串味）', () => {
    expect(formatTimestampInZone(INSTANT, 'Asia/Shanghai')).toBe('2024-01-16 07:30:45')
    expect(formatTimestampInZone(INSTANT, 'UTC')).toBe('2024-01-15 23:30:45')
    expect(formatTimestampInZone(INSTANT, 'Asia/Shanghai')).toBe('2024-01-16 07:30:45')
  })

  it('null 与非有限值返回 null 而不是 Invalid Date', () => {
    expect(formatTimestampInZone(null, 'UTC')).toBeNull()
    expect(formatTimestampInZone(Number.NaN, 'UTC')).toBeNull()
    expect(formatTimestampInZone(Number.POSITIVE_INFINITY, 'UTC')).toBeNull()
  })
})

describe('hostsModel/formatKib', () => {
  it('1024 以下停在 KiB', () => {
    expect(formatKib(0)).toBe('0 KiB')
    expect(formatKib(512)).toBe('512 KiB')
    expect(formatKib(1023)).toBe('1023 KiB')
  })

  it('逐级进位到 MiB / GiB / TiB', () => {
    expect(formatKib(1024)).toBe('1 MiB')
    expect(formatKib(1024 * 1024)).toBe('1 GiB')
    expect(formatKib(1024 * 1024 * 1024)).toBe('1 TiB')
  })

  it('TiB 是最大单位，更大的量继续堆在 TiB 上', () => {
    expect(formatKib(1024 * 1024 * 1024 * 2048)).toBe('2048 TiB')
  })

  it('小于 100 的非 KiB 值保留 1 位小数，100 以上取整', () => {
    expect(formatKib(1536)).toBe('1.5 MiB')
    expect(formatKib(1024 * 100)).toBe('100 MiB')
    expect(formatKib(1024 * 512 + 512)).toBe('513 MiB')
  })
})

describe('hostsModel/isMachineIdHash', () => {
  it('接受 64 位十六进制，大小写与首尾空白都容忍', () => {
    expect(isMachineIdHash('a'.repeat(64))).toBe(true)
    expect(isMachineIdHash('A'.repeat(64))).toBe(true)
    expect(isMachineIdHash(`  ${'0'.repeat(63)}f  `)).toBe(true)
  })

  it('长度不对、含非十六进制字符、空串一律拒绝', () => {
    expect(isMachineIdHash('a'.repeat(63))).toBe(false)
    expect(isMachineIdHash('a'.repeat(65))).toBe(false)
    expect(isMachineIdHash(`${'a'.repeat(63)}g`)).toBe(false)
    expect(isMachineIdHash('')).toBe(false)
    expect(isMachineIdHash(`${'a'.repeat(32)}-${'a'.repeat(31)}`)).toBe(false)
  })
})
