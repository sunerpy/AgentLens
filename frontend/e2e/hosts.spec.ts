import { expect, test, type Page } from '@playwright/test'

import type { SshProbeResult } from '../src/generated'
import { zh } from '../src/i18n/zh'
import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Hosts view spec (todo 18).
 *
 * ### How the todo-18 commands are stubbed
 *
 * `local_machine_identity`, `ssh_probe`, `ssh_probe_cancel`, `credential_set/status/delete` are NOT in the
 * shared `IpcCommand` union, and `src/lib/mockIpc.ts` is shell surface this worker must not
 * edit — its `HANDLERS` table is typed `Record<IpcCommand, …>`, so these commands cannot be
 * registered there. So after the shared mock installs itself over
 * `window.__TAURI_INTERNALS__`, {@link stubHostsIpc} wraps that object's `invoke`: the extra
 * commands are answered locally and everything else is delegated to the shared mock
 * untouched, which keeps the seeded dataset and the call recorder intact. Calls to the extra
 * commands are recorded on `window.__AGENTLENS_HOSTS_CALLS__`.
 *
 * Every assertion waits on an explicit locator; there are no fixed timeouts.
 */

/** `local-host-000001`'s machine id in the seeded dataset, so the local card counts as registered. */
const LOCAL_MACHINE_ID_HASH = 'a'.repeat(64)

/**
 * The machine behind `build-box.internal` — the host these specs add.
 *
 * {@link PROBE_SUCCESS} and {@link fillSshForm} must agree on it: the probe reads this
 * machine's id off the remote, so a second, different constant would describe a fixture
 * where the remote is one machine and the operator typed another's identity. Sharing it is
 * what makes "the auto-filled hash is the one that reaches `hosts_create`" assertable.
 */
const NEW_MACHINE_ID_HASH = 'c'.repeat(64)

/**
 * Typed rather than `as const`: a field added to `SshProbeResult` by the ts-rs gate then
 * fails `tsc -b` here instead of surfacing as `undefined` at Playwright runtime, which is
 * how `machineIdHash` came to be missing from this fixture in the first place.
 */
const PROBE_SUCCESS: SshProbeResult = {
  architecture: 'x86_64',
  xdgDataHome: '/home/ci/.local/share',
  dataDir: '/home/ci/.local/share/opencode',
  availableKib: 8_388_608,
  machineIdSource: '/etc/machine-id',
  machineIdHash: NEW_MACHINE_ID_HASH,
}

/** Verbatim `SshError::AuthFailed` message + `remediation()` from `agentlens_core::transport::ssh`. */
const AUTH_FAILED = {
  code: 'refresh',
  message: 'SSH 认证失败：Permission denied (publickey)',
  fields: {
    variant: 'authFailed',
    remediation: '请检查 SSH 用户、密钥、agent 或钥匙串口令后重试。',
  },
} as const

const TIMED_OUT = {
  code: 'refresh',
  message: 'Stage1 超过 20000 毫秒硬超时',
  fields: {
    variant: 'timedOut',
    remediation: '连接测试已终止 SSH 进程树；请检查网络、代理、远端 sshd 与认证交互后重试。',
  },
} as const

/** Verbatim `HostError::DuplicateMachine` text from `agentlens_core::host`. */
const DUPLICATE_MACHINE = {
  code: 'conflict',
  message: '机器 id 与主机 workstation 重复，同一台机器不能重复添加（否则用量会被双计）',
  fields: {},
} as const

interface HostsStub {
  identity?: unknown
  identityError?: unknown
  probe?: unknown
  probeError?: unknown
  probePendingUntilCancelled?: boolean
  credentialStatus?: unknown
}

/**
 * Wrap the installed mock's `invoke` so the six todo-18 commands resolve locally.
 *
 * Runs as an `addInitScript` **after** the harness seeds its own config, but the wrapping
 * itself is deferred with a property setter: the shared mock replaces
 * `window.__TAURI_INTERNALS__` during the app's dynamic import, which happens after init
 * scripts run, so the wrap has to trigger on that assignment rather than before it.
 */
async function stubHostsIpc(page: Page, stub: HostsStub = {}): Promise<void> {
  await page.addInitScript((serialized: string) => {
    const config = JSON.parse(serialized) as HostsStub
    const calls: { command: string; args: Record<string, unknown> }[] = []
    const credentials = new Map<string, boolean>()
    let rejectProbe: ((reason?: unknown) => void) | undefined
    ;(window as unknown as Record<string, unknown>).__AGENTLENS_HOSTS_CALLS__ = calls

    const handle = async (command: string, args: Record<string, unknown>): Promise<unknown> => {
      const key = `${String(args.hostId)}:${String(args.kind)}`
      switch (command) {
        case 'local_machine_identity':
          if (config.identityError !== undefined) throw config.identityError
          return (
            config.identity ?? {
              hostId: 'local-host-000001',
              machineIdHash: 'a'.repeat(64),
              hostname: 'workstation',
            }
          )
        case 'ssh_probe':
          if (config.probeError !== undefined) throw config.probeError
          if (config.probePendingUntilCancelled === true) {
            return new Promise((_resolve, reject) => {
              rejectProbe = reject
            })
          }
          if (config.probe === undefined)
            throw { code: 'internal', message: 'no probe stub', fields: {} }
          return config.probe
        case 'ssh_probe_cancel':
          rejectProbe?.({
            code: 'refresh',
            message: '客户端已取消 SSH 采集',
            fields: { variant: 'clientCancelled', remediation: '可安全重新发起测试。' },
          })
          rejectProbe = undefined
          return null
        case 'credential_set':
          credentials.set(key, true)
          return { hostId: args.hostId, kind: args.kind, present: true }
        case 'credential_delete':
          credentials.set(key, false)
          return { hostId: args.hostId, kind: args.kind, present: false }
        case 'credential_status':
          return {
            hostId: args.hostId,
            kind: args.kind,
            present: credentials.get(key) ?? config.credentialStatus === true,
          }
        default:
          return undefined
      }
    }

    const wrap = (internals: Record<string, unknown>) => {
      const inner = internals.invoke as (
        command: string,
        args?: Record<string, unknown>,
      ) => Promise<unknown>
      internals.invoke = async (command: string, args: Record<string, unknown> = {}) => {
        const extra = [
          'local_machine_identity',
          'ssh_probe',
          'ssh_probe_cancel',
          'credential_set',
          'credential_status',
          'credential_delete',
        ]
        if (!extra.includes(command)) return inner(command, args)
        calls.push({ command, args: structuredClone(args) })
        return handle(command, args)
      }
      return internals
    }

    let current: Record<string, unknown> | undefined
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      get: () => current,
      set: (value: Record<string, unknown>) => {
        current = wrap(value)
      },
    })
  }, JSON.stringify(stub))
}

async function openHosts(page: Page, stub: HostsStub = {}, config = {}): Promise<void> {
  await stubHostsIpc(page, stub)
  await openShell(page, config)
  await page.getByTestId('nav-hosts').click()
  await expect(page.getByTestId('view-hosts')).toBeVisible()
}

interface RecordedCall {
  command: string
  args: Record<string, unknown>
}

function hostsCalls(page: Page, command: string): Promise<RecordedCall[]> {
  return page.evaluate((name: string) => {
    const calls = (window as unknown as Record<string, RecordedCall[]>).__AGENTLENS_HOSTS_CALLS__
    return calls.filter((call) => call.command === name)
  }, command)
}

/**
 * Everything except the machine id. Split out because the machine id is no longer operator
 * input: after a successful probe the field is auto-filled and read-only, so a spec that
 * exercises that path must not pre-seed it — otherwise the assertion cannot tell an
 * auto-filled hash from a typed one.
 */
async function fillSshTarget(
  page: Page,
  overrides: Partial<
    Record<'displayName' | 'target' | 'user' | 'identity' | 'dataDir', string>
  > = {},
) {
  await page.getByTestId('add-host-display-name').fill(overrides.displayName ?? 'build-box-2')
  await page.getByTestId('add-host-target').fill(overrides.target ?? 'build-box.internal')
  await page.getByTestId('add-host-user').fill(overrides.user ?? 'ci')
  if (overrides.identity !== undefined) {
    await page.getByTestId('add-host-identity').fill(overrides.identity)
  }
  if (overrides.dataDir !== undefined) {
    await page.getByTestId('add-host-data-dir').fill(overrides.dataDir)
  }
}

async function fillSshForm(
  page: Page,
  overrides: Partial<
    Record<'displayName' | 'target' | 'user' | 'identity' | 'dataDir' | 'machineId', string>
  > = {},
) {
  await fillSshTarget(page, overrides)
  await page.getByTestId('add-host-machine-id').fill(overrides.machineId ?? NEW_MACHINE_ID_HASH)
}

test('the local card auto-registers this machine and is visually distinct from ssh rows', async ({
  page,
}) => {
  await openHosts(page)

  const card = page.getByTestId('local-host-card')
  await expect(card).toHaveAttribute('data-local-state', 'registered')
  await expect(page.getByTestId('local-host-name')).toHaveText('workstation')
  await expect(page.getByTestId('local-host-id')).toHaveText('local-host-000001')

  // Already registered: no duplicate insert may be attempted.
  expect(await mockCalls(page, 'hosts_create')).toHaveLength(0)

  // The list still carries both hosts, with the ssh one distinguishable by attribute.
  await expect(page.getByTestId('host-row-local-host-000001')).toHaveAttribute(
    'data-host-kind',
    'local',
  )
  await expect(page.getByTestId('host-row-ssh-host-0000002')).toHaveAttribute(
    'data-host-kind',
    'ssh',
  )
})

test('a missing machine id renders the backend remediation instead of a blank card', async ({
  page,
}) => {
  await openHosts(page, {
    identityError: {
      code: 'database',
      message:
        'cannot read a stable machine id from any known source: /etc/machine-id (not found); on Linux run `systemd-machine-id-setup` or write a fresh 32-hex id to /etc/machine-id and restart AgentLens',
      fields: {},
    },
  })

  const card = page.getByTestId('local-host-card')
  await expect(card).toHaveAttribute('data-local-state', 'identityUnavailable')
  await expect(page.getByTestId('local-identity-error-message')).toContainText(
    'systemd-machine-id-setup',
  )
  expect(await mockCalls(page, 'hosts_create')).toHaveLength(0)
})

test('测试连接 success shows the remote architecture and the discovered data directory', async ({
  page,
}) => {
  await openHosts(page, { probe: PROBE_SUCCESS })
  await fillSshTarget(page, { identity: '~/.ssh/id_ed25519', dataDir: '' })

  await page.getByTestId('add-host-test').click()

  await expect(page.getByTestId('probe-success')).toBeVisible()
  await expect(page.getByTestId('probe-architecture')).toHaveText('x86_64')
  await expect(page.getByTestId('probe-data-dir')).toHaveText('/home/ci/.local/share/opencode')
  await expect(page.getByTestId('probe-xdg')).toHaveText('/home/ci/.local/share')
  await expect(page.getByTestId('probe-machine-id-source')).toHaveText('/etc/machine-id')
  await expect(page.getByTestId('probe-error')).toBeHidden()

  // The hash was never typed here: it can only have come from the probe response.
  const machineId = page.getByTestId('add-host-machine-id')
  await expect(machineId).toHaveValue(NEW_MACHINE_ID_HASH)
  await expect(machineId).toHaveAttribute('readonly', '')

  const probes = await hostsCalls(page, 'ssh_probe')
  expect(probes).toHaveLength(1)
  expect(probes[0].args).toMatchObject({
    input: {
      sshTarget: 'ci@build-box.internal',
      identityFile: '~/.ssh/id_ed25519',
      remoteDataDir: null,
    },
  })

  await qaScreenshot(page, 'hosts.png')
})

/**
 * The whole point of reading the hash off the remote: the operator types no identity at
 * all, and the value that reaches `hosts_create` is the one the probe reported.
 */
test('the probed machine id is submitted without the operator ever typing it', async ({ page }) => {
  await openHosts(page, { probe: PROBE_SUCCESS })
  await fillSshTarget(page)

  await page.getByTestId('add-host-test').click()
  await expect(page.getByTestId('probe-success')).toBeVisible()
  await page.getByTestId('add-host-submit').click()

  await expect.poll(async () => (await mockCalls(page, 'hosts_create')).length).toBe(1)
  expect(await mockCalls(page, 'hosts_create')).toMatchObject([
    {
      args: {
        input: {
          displayName: 'build-box-2',
          kind: 'ssh',
          machineIdHash: NEW_MACHINE_ID_HASH,
          sshTarget: 'ci@build-box.internal',
        },
      },
    },
  ])
})

/**
 * The double-counting guard: a hash read off machine A must never be registered against
 * machine B, so editing the ssh target drops it and forces a fresh probe.
 */
test('editing the ssh target after a probe drops the hash and re-blocks the submit', async ({
  page,
}) => {
  await openHosts(page, { probe: PROBE_SUCCESS })
  await fillSshTarget(page)

  await page.getByTestId('add-host-test').click()
  const machineId = page.getByTestId('add-host-machine-id')
  await expect(machineId).toHaveValue(NEW_MACHINE_ID_HASH)

  await page.getByTestId('add-host-target').fill('other-box.internal')

  await expect(machineId).toHaveValue('')
  await expect(machineId).not.toHaveAttribute('readonly', '')
  await expect(page.getByTestId('probe-success')).toBeHidden()

  await page.getByTestId('add-host-submit').click()
  await expect(page.getByTestId('add-host-validation')).toHaveText(
    zh.hosts.add.requireMachineIdHash,
  )
  expect(await mockCalls(page, 'hosts_create')).toHaveLength(0)
})

test('测试连接 AuthFailed renders the Chinese remediation and the key-file guidance', async ({
  page,
}) => {
  await openHosts(page, { probeError: AUTH_FAILED })
  await fillSshForm(page)

  await page.getByTestId('add-host-test').click()

  const panel = page.getByTestId('probe-error')
  await expect(panel).toBeVisible()
  await expect(page.getByTestId('probe-error-message')).toContainText('Permission denied')
  // The typed variant's `remediation()` must be rendered, not the variant name.
  await expect(page.getByTestId('probe-error-remediation')).toHaveText(
    '请检查 SSH 用户、密钥、agent 或钥匙串口令后重试。',
  )
  await expect(panel).not.toContainText('authFailed')
  // Windows without a usable ssh-agent lands on this variant, so the key-file guidance shows.
  await expect(page.getByTestId('ssh-agent-guidance')).toContainText('密钥路径')
  await expect(page.getByTestId('probe-success')).toBeHidden()

  await qaScreenshot(page, 'hosts-auth-failed.png')
})

test('测试连接 remains interactive while pending and cancels the matching request', async ({
  page,
}) => {
  await openHosts(page, { probePendingUntilCancelled: true })
  await fillSshForm(page)

  await page.getByTestId('add-host-test').click()
  await expect(page.getByTestId('probe-pending')).toContainText('最多运行 20 秒')
  await expect(page.getByTestId('probe-pending')).toContainText('可随时取消')
  await page.getByTestId('add-host-display-name').fill('界面仍可交互')
  await expect(page.getByTestId('add-host-display-name')).toHaveValue('界面仍可交互')
  await qaScreenshot(page, 'hosts-probe-pending.png')

  await page.getByTestId('add-host-test-cancel').click()
  await expect(page.getByTestId('probe-error-remediation')).toHaveText('可安全重新发起测试。')
  const probes = await hostsCalls(page, 'ssh_probe')
  const cancellations = await hostsCalls(page, 'ssh_probe_cancel')
  expect(cancellations).toHaveLength(1)
  expect(cancellations[0].args.requestId).toBe(probes[0].args.requestId)
  await qaScreenshot(page, 'hosts-probe-cancelled.png')
})

test('测试连接 timeout renders the typed process-tree remediation', async ({ page }) => {
  await openHosts(page, { probeError: TIMED_OUT })
  await fillSshForm(page)

  await page.getByTestId('add-host-test').click()

  await expect(page.getByTestId('probe-error-code')).toHaveText('refresh')
  await expect(page.getByTestId('probe-error-message')).toContainText('20000 毫秒硬超时')
  await expect(page.getByTestId('probe-error-remediation')).toContainText('已终止 SSH 进程树')
  await expect(page.getByTestId('probe-pending')).toBeHidden()
  await qaScreenshot(page, 'hosts-probe-timeout.png')
})

test('the seeded error-state ssh host renders its remediation and its last success', async ({
  page,
}) => {
  await openHosts(page)

  const row = page.getByTestId('host-row-ssh-host-0000002')
  await expect(row).toHaveAttribute('data-host-state', 'error')
  const failing = page.getByTestId('host-source-ssh-host-0000002-claude-code-error')
  await expect(failing).toContainText('Permission denied (publickey)')
  await expect(failing).toContainText('请检查密钥路径')
  // `last_success` survives the failure, so the row must not claim "从未成功".
  await expect(page.getByTestId('host-last-success-ssh-host-0000002')).toHaveText(
    '2026-01-05 00:00:00',
  )
  await expect(
    page.getByTestId('host-source-ssh-host-0000002-claude-code-interrupted'),
  ).toBeVisible()

  // A healthy host shows no error panel at all.
  await expect(page.getByTestId('host-row-local-host-000001')).toHaveAttribute(
    'data-host-state',
    'idle',
  )
  await expect(page.getByTestId('host-source-local-host-000001-opencode-error')).toBeHidden()
})

/**
 * 需求 4 的可见形态：调度器键是 (host_id, source)，因此一台主机的两个采集源必须分别显示状态。
 * 这条断言的核心是「OpenCode 空闲」与「Claude Code 出错」同时可见，且整行读作出错 ——
 * 只显示其中一个就等于把故障藏起来。
 */
test('one host renders one state per source, and a single failing source marks the whole row', async ({
  page,
}) => {
  await openHosts(page)

  const sources = page.getByTestId('host-sources-ssh-host-0000002')
  await expect(sources.getByTestId('host-source-ssh-host-0000002-opencode-name')).toHaveText(
    'opencode',
  )
  await expect(sources.getByTestId('host-source-ssh-host-0000002-claude-code-name')).toHaveText(
    'claude-code',
  )

  await expect(page.getByTestId('host-source-ssh-host-0000002-opencode')).toHaveAttribute(
    'data-source-state',
    'idle',
  )
  await expect(page.getByTestId('host-source-ssh-host-0000002-claude-code')).toHaveAttribute(
    'data-source-state',
    'error',
  )
  await expect(page.getByTestId('host-source-ssh-host-0000002-opencode-state')).toHaveText(
    zh.hosts.list.stateIdle,
  )
  await expect(page.getByTestId('host-source-ssh-host-0000002-claude-code-state')).toHaveText(
    zh.hosts.list.stateError,
  )

  // 出错优先：一个源坏了，整行不能显示成健康。
  await expect(page.getByTestId('host-row-ssh-host-0000002')).toHaveAttribute(
    'data-host-state',
    'error',
  )
  // 健康的那个源不得挂上错误面板。
  await expect(page.getByTestId('host-source-ssh-host-0000002-opencode-error')).toBeHidden()

  // 单源主机仍然只列一个源。
  const localSources = page.getByTestId('host-sources-local-host-000001')
  await expect(localSources.locator('li')).toHaveCount(1)
})

/**
 * 需求 2：本机走 LocalHostSource 直接读本地文件，压根不经过 SSH，钥匙串凭据对它永远不会被
 * 使用。因此本机行不得出现「凭据」按钮 —— 提供它等于诱导用户去配一个不起作用的东西。
 */
test('the local host offers no credentials button, while an ssh host does', async ({ page }) => {
  await openHosts(page)

  await expect(page.getByTestId('host-row-local-host-000001')).toBeVisible()
  await expect(page.getByTestId('host-credentials-local-host-000001')).toHaveCount(0)
  await expect(page.getByTestId('host-credentials-ssh-host-0000002')).toBeVisible()

  // 刷新与删除对两类主机都成立，被隐藏的只有凭据这一个按钮。
  await expect(page.getByTestId('host-refresh-local-host-000001')).toBeVisible()
  await expect(page.getByTestId('host-delete-local-host-000001')).toBeVisible()
})

/** 凭据卡必须说清「什么时候会用到」，以及 present=false 也可能是助手缺失。 */
test('the credential card explains when a stored secret is used and what absent covers', async ({
  page,
}) => {
  await openHosts(page)

  await expect(page.getByTestId('credential-when-used')).toContainText('采集时会自动完成认证')
  await expect(page.getByTestId('credential-when-used')).toContainText('本机')
  await expect(page.getByTestId('credential-absent-hint')).toContainText('口令助手缺失')
})

/**
 * 需求 4 的另一半：一键刷新对**全部主机的全部启用采集源**各起一轮。种子数据是
 * 1 个本机源 + 2 个远端源 = 3 轮，因此这条断言能证明它没有漏掉任何 (主机, 采集源) 组合。
 */
test('一键刷新 triggers one round per (host, source) across every host', async ({ page }) => {
  await openHosts(page)

  const before = (await mockCalls(page, 'trigger_refresh')).length
  await page.getByTestId('host-refresh-all').click()

  await expect(page.getByTestId('host-refresh-all-result')).toHaveText(
    zh.hosts.list.refreshAllDone(3),
  )
  // 每台主机一次 invoke，每次 invoke 内部按启用源展开。
  const calls = await mockCalls(page, 'trigger_refresh')
  expect(calls.length - before).toBe(2)
  expect(calls.slice(before).map((call) => call.args.hostId)).toEqual([
    'local-host-000001',
    'ssh-host-0000002',
  ])

  // 两台主机的每个源都被刷成空闲，包括原先出错的那个。
  await expect(page.getByTestId('host-source-ssh-host-0000002-claude-code')).toHaveAttribute(
    'data-source-state',
    'idle',
  )
  await expect(page.getByTestId('host-row-ssh-host-0000002')).toHaveAttribute(
    'data-host-state',
    'idle',
  )
})

/** 没有主机时按钮禁用，避免一次空转的 IPC 与一句无意义的「已触发 0 轮」。 */
test('一键刷新 is disabled when there is no host at all', async ({ page }) => {
  await openHosts(page, {}, { dataset: { hosts: [], refreshStatus: [] } })

  await expect(page.getByTestId('host-list-empty')).toBeVisible()
  await expect(page.getByTestId('host-refresh-all')).toBeDisabled()
})

test('a duplicate machine id surfaces the backend rejection text verbatim', async ({ page }) => {
  await openHosts(page, {}, { errors: { hosts_create: DUPLICATE_MACHINE } })
  await fillSshForm(page, { machineId: LOCAL_MACHINE_ID_HASH })

  await page.getByTestId('add-host-submit').click()

  await expect(page.getByTestId('add-host-error')).toBeVisible()
  await expect(page.getByTestId('add-host-error-code')).toHaveText('conflict')
  await expect(page.getByTestId('add-host-error-message')).toContainText(
    '同一台机器不能重复添加（否则用量会被双计）',
  )
})

test('a manual refresh renders alreadyRunning distinctly instead of silently doing nothing', async ({
  page,
}) => {
  await openHosts(
    page,
    {},
    {
      // 返回值现在是每个采集源一条；两个源都在跑，整行才读作「已在刷新中」。
      responses: {
        trigger_refresh: [
          {
            outcome: 'alreadyRunning',
            host_id: 'ssh-host-0000002',
            source: 'opencode',
            started_at_utc: Date.UTC(2026, 0, 7),
          },
          {
            outcome: 'alreadyRunning',
            host_id: 'ssh-host-0000002',
            source: 'claude-code',
            started_at_utc: Date.UTC(2026, 0, 7),
          },
        ],
      },
    },
  )

  await page.getByTestId('host-refresh-ssh-host-0000002').click()

  const outcome = page.getByTestId('host-refresh-outcome-ssh-host-0000002')
  await expect(outcome).toHaveText(zh.hosts.list.alreadyRunning)
  expect(await mockCalls(page, 'trigger_refresh')).toHaveLength(1)
})

/**
 * 只要有一个源真的开始了，这次点击就确实做了事，整行必须读作「已开始刷新」。
 * 否则用户会以为按钮没生效，而实际上一轮采集正在进行。
 */
test('a partially started refresh reads as started, not alreadyRunning', async ({ page }) => {
  await openHosts(
    page,
    {},
    {
      responses: {
        trigger_refresh: [
          {
            outcome: 'alreadyRunning',
            host_id: 'ssh-host-0000002',
            source: 'opencode',
            started_at_utc: Date.UTC(2026, 0, 7),
          },
          {
            outcome: 'started',
            host_id: 'ssh-host-0000002',
            source: 'claude-code',
            started_at_utc: Date.UTC(2026, 0, 7),
          },
        ],
      },
    },
  )

  await page.getByTestId('host-refresh-ssh-host-0000002').click()
  await expect(page.getByTestId('host-refresh-outcome-ssh-host-0000002')).toHaveText(
    zh.hosts.list.started,
  )
})

test('a started refresh reads differently from alreadyRunning', async ({ page }) => {
  await openHosts(page)

  const statusReads = (await mockCalls(page, 'get_refresh_status')).length
  await expect(page.getByTestId('host-row-ssh-host-0000002')).toHaveAttribute(
    'data-host-state',
    'error',
  )

  await page.getByTestId('host-refresh-ssh-host-0000002').click()

  await expect(page.getByTestId('host-refresh-outcome-ssh-host-0000002')).toHaveText('已开始刷新')
  await expect(page.getByTestId('host-row-ssh-host-0000002')).toHaveAttribute(
    'data-host-state',
    'idle',
  )
  expect(await mockCalls(page, 'get_refresh_status')).toHaveLength(statusReads)
  expect(await mockCalls(page, 'trigger_refresh')).toMatchObject([
    { args: { hostId: 'ssh-host-0000002', onEvent: expect.stringMatching(/^__CHANNEL__:/) } },
  ])
})

test('adding a host refetches the list and deleting one removes the row', async ({ page }) => {
  await openHosts(page, { probe: PROBE_SUCCESS })

  const before = (await mockCalls(page, 'hosts_list')).length
  await fillSshForm(page)
  await page.getByTestId('add-host-submit').click()

  // The mutation must invalidate the list query, so a fresh `hosts_list` fires.
  await expect
    .poll(async () => (await mockCalls(page, 'hosts_list')).length)
    .toBeGreaterThan(before)
  // The new host becomes the credential target, proving the create response was consumed.
  await expect(page.getByTestId('credential-password')).toBeVisible()

  await page.getByTestId('host-delete-ssh-host-0000002').click()
  await expect.poll(async () => (await mockCalls(page, 'hosts_delete')).length).toBe(1)
  expect(await mockCalls(page, 'hosts_delete')).toMatchObject([
    { args: { hostId: 'ssh-host-0000002' } },
  ])
})

test('a keyring write reports presence and never echoes the secret back', async ({ page }) => {
  await openHosts(page)

  await page.getByTestId('host-credentials-ssh-host-0000002').click()
  const row = page.getByTestId('credential-passphrase')
  await expect(row).toHaveAttribute('data-present', 'false')

  await page.getByTestId('credential-passphrase-value').fill('unlock-my-key')
  await page.getByTestId('credential-passphrase-save').click()

  await expect(row).toHaveAttribute('data-present', 'true')
  await expect(page.getByTestId('credential-passphrase-state')).toHaveText(
    zh.hosts.credentials.stored,
  )
  // The input is cleared on success and the plaintext appears nowhere in the DOM.
  await expect(page.getByTestId('credential-passphrase-value')).toHaveValue('')
  expect(await page.content()).not.toContain('unlock-my-key')

  await page.getByTestId('credential-passphrase-remove').click()
  await expect(row).toHaveAttribute('data-present', 'false')
})

test('a malformed host is rejected client-side before any IPC call', async ({ page }) => {
  await openHosts(page)

  // Empty display name and empty target.
  await page.getByTestId('add-host-submit').click()
  await expect(page.getByTestId('add-host-validation')).toHaveText(zh.hosts.add.requireHost)
  expect(await mockCalls(page, 'hosts_create')).toHaveLength(0)

  // An ssh host with a target but no display name.
  await page.getByTestId('add-host-target').fill('build-box.internal')
  await page.getByTestId('add-host-submit').click()
  await expect(page.getByTestId('add-host-validation')).toHaveText(zh.hosts.add.requireDisplayName)
  expect(await mockCalls(page, 'hosts_create')).toHaveLength(0)

  // A 500-character alias and a quoted key path with spaces must not break layout or
  // smuggle anything: the machine id is still validated, so no IPC fires.
  await page.getByTestId('add-host-display-name').fill('x'.repeat(500))
  await page.getByTestId('add-host-target').fill('a'.repeat(500))
  await page.getByTestId('add-host-identity').fill('/home/ci/my keys/"id rsa".pem')
  await page.getByTestId('add-host-machine-id').fill('not-a-hash')
  await page.getByTestId('add-host-submit').click()
  await expect(page.getByTestId('add-host-validation')).toHaveText(
    zh.hosts.add.requireMachineIdHash,
  )
  expect(await mockCalls(page, 'hosts_create')).toHaveLength(0)
  await expect(page.getByTestId('view-hosts')).toBeVisible()
})

test('an injection-shaped display name is stored and rendered literally', async ({ page }) => {
  const injected = `Robert'); DROP TABLE hosts;--`
  await openHosts(
    page,
    {},
    {
      dataset: {
        hosts: [
          {
            hostId: 'ssh-host-0000003',
            machineIdHash: 'd'.repeat(64),
            displayName: injected,
            kind: 'ssh',
            sshTarget: 'ci@evil.internal',
            remoteDataDir: null,
            lastSuccessUtc: null,
            enabledSources: ['opencode'],
          },
        ],
        refreshStatus: [],
      },
    },
  )

  await expect(page.getByTestId('host-name-ssh-host-0000003')).toHaveText(injected)
  await expect(page.getByTestId('host-last-success-ssh-host-0000003')).toHaveText('从未成功')
  // No status row exists for this host, so the state badge degrades to "状态未知" rather
  // than pretending the host is healthy.
  await expect(page.getByTestId('host-state-ssh-host-0000003')).toHaveText('状态未知')

  // This dataset does not contain the local machine, so the local card auto-registers it —
  // that is one legitimate `hosts_create` before the form's own submit. Count from a
  // baseline rather than asserting an absolute total.
  const before = (await mockCalls(page, 'hosts_create')).length
  await fillSshForm(page, { displayName: injected })
  await page.getByTestId('add-host-submit').click()
  await expect.poll(async () => (await mockCalls(page, 'hosts_create')).length).toBe(before + 1)
  const creates = await mockCalls(page, 'hosts_create')
  expect(creates[creates.length - 1].args).toMatchObject({
    input: { displayName: injected, kind: 'ssh' },
  })
})

test('an IPC failure renders the shared error state rather than a white screen', async ({
  page,
}) => {
  await openHosts(
    page,
    {},
    {
      errors: {
        hosts_list: { code: 'database', message: 'archive is locked', fields: {} },
      },
    },
  )

  await expect(page.getByTestId('error-state')).toBeVisible()
  await expect(page.getByTestId('error-code')).toHaveText('database')
  await expect(page.getByTestId('error-message')).toHaveText('archive is locked')
  await expect(page.getByTestId('host-list')).toBeHidden()
  await expect(page.getByTestId('view-hosts')).toBeVisible()
})

/**
 * 缺陷回归：四个适配器都实现了，但界面没有任何入口去启用它们 —— 三处写路径全传
 * `enabledSources: null`（后端语义是「保持库里已存的集合」），新建主机落到 DB 列默认值
 * `'opencode'`，于是 ~/.codex、~/.claude、~/.hermes 从不被扫描，总览里只剩 opencode。
 * 这条用例断言的正是那个缺失的入口：可勾选项来自后端导出，保存后立即出现在采集源列表里。
 */
test('每台主机都能勾选启用哪些采集源，可选项来自后端导出的清单', async ({ page }) => {
  await openHosts(page)

  const editor = page.getByTestId('host-sources-edit-local-host-000001')
  await expect(editor).toBeVisible()
  for (const source of ['opencode', 'claude-code', 'codex', 'hermes']) {
    await expect(page.getByTestId(`host-source-toggle-local-host-000001-${source}`)).toBeVisible()
  }
  // 种子数据只启用了 opencode，其余三个必须显式勾选 —— 这就是缺陷的成因。
  await expect(page.getByTestId('host-source-toggle-local-host-000001-opencode')).toBeChecked()
  await expect(page.getByTestId('host-source-toggle-local-host-000001-codex')).not.toBeChecked()
  await expect(page.getByTestId('host-sources-save-local-host-000001')).toBeDisabled()

  await page.getByTestId('host-source-toggle-local-host-000001-codex').check()
  await page.getByTestId('host-source-toggle-local-host-000001-hermes').check()
  await qaScreenshot(page, 'hosts-sources-picker.png')

  await page.getByTestId('host-sources-save-local-host-000001').click()

  await expect(page.getByTestId('host-sources-saved-local-host-000001')).toContainText(
    zh.hosts.list.sourcesSaved,
  )
  const updates = await mockCalls(page, 'hosts_update')
  expect(updates).toHaveLength(1)
  expect(updates[0].args).toMatchObject({
    input: {
      hostId: 'local-host-000001',
      // 显式数组，不是 null：null 会让后端保持原样，等于按钮什么都没做。
      enabledSources: ['opencode', 'codex', 'hermes'],
    },
  })

  // 写入真正落到了数据集上：刷新后的列表把两个新源列了出来。
  const sources = page.getByTestId('host-sources-local-host-000001')
  await expect(sources.getByTestId('host-source-local-host-000001-codex-name')).toHaveText('codex')
  await expect(sources.getByTestId('host-source-local-host-000001-hermes-name')).toHaveText(
    'hermes',
  )
  await expect(page.getByTestId('host-source-local-host-000001-codex-state')).toHaveText(
    zh.hosts.list.statusUnavailable,
  )
})

test('取消勾选全部采集源被前端拦下，一次 IPC 都不发', async ({ page }) => {
  await openHosts(page)

  await page.getByTestId('host-source-toggle-local-host-000001-opencode').uncheck()
  await page.getByTestId('host-sources-save-local-host-000001').click()

  await expect(page.getByTestId('host-sources-validation-local-host-000001')).toHaveText(
    zh.hosts.list.sourcesRequireOne,
  )
  expect(await mockCalls(page, 'hosts_update')).toHaveLength(0)
})

/** 后端对非法集合报「采集源配置无效」，界面必须把这句原样显示出来。 */
test('后端拒绝非法采集源集合时显示其原文', async ({ page }) => {
  await openHosts(
    page,
    {},
    {
      errors: {
        hosts_update: {
          code: 'invalidInput',
          message:
            'host local-host-000001 的采集源配置无效：采集源列表为空，主机将无法被采集；可用采集源为 opencode, claude-code, codex, hermes',
          fields: {},
        },
      },
    },
  )

  await page.getByTestId('host-source-toggle-local-host-000001-codex').check()
  await page.getByTestId('host-sources-save-local-host-000001').click()

  await expect(page.getByTestId('host-sources-error-local-host-000001-code')).toHaveText(
    'invalidInput',
  )
  await expect(page.getByTestId('host-sources-error-local-host-000001-message')).toContainText(
    '采集源配置无效',
  )
})
