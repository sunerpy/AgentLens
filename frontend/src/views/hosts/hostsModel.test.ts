import { describe, expect, it } from 'vitest'

import type { Host, SourceStatus } from '@/generated'
import { zh } from '@/i18n/zh'

import {
  composeSshTarget,
  formatKib,
  formatUtcTimestamp,
  hostErrorText,
  hostLastSuccessUtc,
  hostStateKey,
  hostStateLabel,
  ipcRemediation,
  ipcVariant,
  isMachineIdHash,
  joinHostStatus,
  needsKeyFileGuidance,
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
    ...overrides,
  }
}

function status(overrides: Partial<SourceStatus> = {}): SourceStatus {
  return {
    hostId: 'local',
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
    expect(rows[0].status?.hostId).toBe('a')
  })

  it('缺状态的主机得到 undefined 而不是被丢弃', () => {
    const rows = joinHostStatus(
      [host({ hostId: 'a' }), host({ hostId: 'b' })],
      [status({ hostId: 'a' })],
    )
    expect(rows).toHaveLength(2)
    expect(rows[1].status).toBeUndefined()
  })

  it('多余的状态不会凭空造出主机行', () => {
    const rows = joinHostStatus([host({ hostId: 'a' })], [status({ hostId: 'ghost' })])
    expect(rows).toHaveLength(1)
    expect(rows[0].status).toBeUndefined()
  })

  it('空输入产出空数组', () => {
    expect(joinHostStatus([], [])).toEqual([])
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

describe('hostsModel/hostLastSuccessUtc', () => {
  it('失败态仍显示 error 变体里保留的上次成功时间', () => {
    expect(
      hostLastSuccessUtc({
        host: host({ lastSuccessUtc: 1_000 }),
        status: status({
          state: { state: 'error', last_error: 'x', last_success: 5_000 },
          lastSuccessUtc: null,
        }),
      }),
    ).toBe(5_000)
  })

  it('失败态且从未成功过时继续往下回落，而不是谎报', () => {
    expect(
      hostLastSuccessUtc({
        host: host({ lastSuccessUtc: 1_000 }),
        status: status({
          state: { state: 'error', last_error: 'x', last_success: null },
          lastSuccessUtc: null,
        }),
      }),
    ).toBe(1_000)
  })

  it('正常态优先用 status 的时间戳', () => {
    expect(
      hostLastSuccessUtc({
        host: host({ lastSuccessUtc: 1_000 }),
        status: status({ lastSuccessUtc: 9_000 }),
      }),
    ).toBe(9_000)
  })

  it('无状态时退回 host 自身的时间戳', () => {
    expect(hostLastSuccessUtc({ host: host({ lastSuccessUtc: 42 }), status: undefined })).toBe(42)
    expect(hostLastSuccessUtc({ host: host(), status: undefined })).toBeNull()
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

describe('hostsModel/formatUtcTimestamp', () => {
  it('UTC 毫秒渲染成可按肉眼排序的 YYYY-MM-DD HH:mm:ss', () => {
    expect(formatUtcTimestamp(Date.UTC(2024, 0, 15, 23, 30, 45))).toBe('2024-01-15 23:30:45')
    expect(formatUtcTimestamp(0)).toBe('1970-01-01 00:00:00')
  })

  it('null 与非有限值返回 null 而不是 Invalid Date', () => {
    expect(formatUtcTimestamp(null)).toBeNull()
    expect(formatUtcTimestamp(Number.NaN)).toBeNull()
    expect(formatUtcTimestamp(Number.POSITIVE_INFINITY)).toBeNull()
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
