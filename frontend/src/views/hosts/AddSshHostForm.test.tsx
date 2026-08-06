import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SshProbeResult } from '@/generated'
import { zh } from '@/i18n/zh'
import { hostsCreate } from '@/lib/ipc'

import { AddSshHostForm } from './AddSshHostForm'
import { newSshProbeRequestId, sshProbe, sshProbeCancel } from './hostsIpc'

vi.mock('@/lib/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/ipc')>()),
  hostsCreate: vi.fn(),
}))

vi.mock('./hostsIpc', () => ({
  newSshProbeRequestId: vi.fn(),
  sshProbe: vi.fn(),
  sshProbeCancel: vi.fn(),
}))

/** `parse_probe` guarantees 64 lowercase hex, so the fixture must be a valid hash. */
const REMOTE_MACHINE_ID_HASH = 'c'.repeat(64)

const PROBE_SUCCESS: SshProbeResult = {
  architecture: 'x86_64',
  xdgDataHome: '/home/ci/.local/share',
  dataDir: '/home/ci/.local/share/opencode',
  availableKib: 8_388_608,
  machineIdSource: '/etc/machine-id',
  machineIdHash: REMOTE_MACHINE_ID_HASH,
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

function renderForm() {
  const queryClient = new QueryClient({
    defaultOptions: { mutations: { retry: false }, queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <AddSshHostForm onCreated={vi.fn()} />
    </QueryClientProvider>,
  )
}

function enterHostAndTest() {
  fireEvent.change(screen.getByTestId('add-host-target'), { target: { value: 'build-box' } })
  fireEvent.click(screen.getByTestId('add-host-test'))
}

describe('AddSshHostForm SSH probe', () => {
  beforeEach(() => {
    vi.mocked(newSshProbeRequestId).mockReturnValue('probe_test_01')
    vi.mocked(sshProbeCancel).mockResolvedValue(undefined)
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('shows bounded progress while the request remains pending', async () => {
    const pending = deferred<never>()
    vi.mocked(sshProbe).mockReturnValue(pending.promise)
    renderForm()

    enterHostAndTest()

    expect((await screen.findByTestId('probe-pending')).textContent).toContain(
      zh.hosts.add.testingHint,
    )
    expect(sshProbe).toHaveBeenCalledWith(
      { sshTarget: 'build-box', identityFile: null, remoteDataDir: null },
      'probe_test_01',
    )
    expect(screen.getByTestId('add-host-test-cancel').getAttribute('disabled')).toBeNull()
    fireEvent.change(screen.getByTestId('add-host-display-name'), {
      target: { value: '界面仍可交互' },
    })
    expect(screen.getByTestId('add-host-display-name')).toHaveProperty('value', '界面仍可交互')

    pending.reject({
      code: 'refresh',
      message: '客户端已取消 SSH 采集',
      fields: { variant: 'clientCancelled', remediation: '可安全重新发起测试。' },
    })
    await screen.findByTestId('probe-error-remediation')
  })

  it('cancels the active request by requestId', async () => {
    const pending = deferred<never>()
    vi.mocked(sshProbe).mockReturnValue(pending.promise)
    renderForm()

    enterHostAndTest()
    await screen.findByTestId('probe-pending')
    fireEvent.click(screen.getByTestId('add-host-test-cancel'))
    expect(screen.getByTestId('add-host-test-cancel').getAttribute('disabled')).not.toBeNull()
    expect(screen.getByTestId('add-host-test-cancel').textContent).toContain(
      zh.hosts.add.cancelling,
    )
    await waitFor(() => expect(sshProbeCancel).toHaveBeenCalledWith('probe_test_01'))

    pending.reject({
      code: 'refresh',
      message: '客户端已取消 SSH 采集',
      fields: { variant: 'clientCancelled', remediation: '可安全重新发起测试。' },
    })
    expect((await screen.findByTestId('probe-error-remediation')).textContent).toContain(
      '可安全重新发起测试。',
    )
  })

  it('renders the typed timeout and process-tree remediation', async () => {
    vi.mocked(sshProbe).mockRejectedValue({
      code: 'refresh',
      message: 'Stage1 超过 20000 毫秒硬超时',
      fields: {
        variant: 'timedOut',
        remediation: '连接测试已终止 SSH 进程树；请检查网络、代理、远端 sshd 与认证交互后重试。',
      },
    })
    renderForm()

    enterHostAndTest()

    expect((await screen.findByTestId('probe-error-code')).textContent).toContain('refresh')
    expect(screen.getByTestId('probe-error-message').textContent).toContain('20000 毫秒硬超时')
    expect(screen.getByTestId('probe-error-remediation').textContent).toContain('已终止 SSH 进程树')
    expect(screen.queryByTestId('probe-pending')).toBeNull()
  })
})

describe('AddSshHostForm machine-id auto-fill', () => {
  beforeEach(() => {
    vi.mocked(newSshProbeRequestId).mockReturnValue('probe_test_01')
    vi.mocked(sshProbeCancel).mockResolvedValue(undefined)
    vi.mocked(sshProbe).mockResolvedValue(PROBE_SUCCESS)
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  function machineIdInput(): HTMLInputElement {
    return screen.getByTestId('add-host-machine-id') as HTMLInputElement
  }

  it('fills the remote hash from a successful probe and locks the field', async () => {
    renderForm()

    expect(machineIdInput().readOnly).toBe(false)
    expect(screen.getByText(zh.hosts.add.machineIdHashHint)).toBeTruthy()

    enterHostAndTest()
    await screen.findByTestId('probe-success')

    expect(machineIdInput().value).toBe(REMOTE_MACHINE_ID_HASH)
    expect(machineIdInput().readOnly).toBe(true)
    expect(screen.getByText(zh.hosts.add.machineIdHashFilled)).toBeTruthy()
  })

  it('submits the auto-filled hash without the operator ever typing it', async () => {
    renderForm()

    fireEvent.change(screen.getByTestId('add-host-display-name'), { target: { value: '构建机' } })
    enterHostAndTest()
    await screen.findByTestId('probe-success')
    fireEvent.click(screen.getByTestId('add-host-submit'))

    await waitFor(() => expect(hostsCreate).toHaveBeenCalledTimes(1))
    expect(vi.mocked(hostsCreate).mock.calls[0][0]).toEqual({
      displayName: '构建机',
      kind: 'ssh',
      machineIdHash: REMOTE_MACHINE_ID_HASH,
      sshTarget: 'build-box',
      remoteDataDir: null,
    })
  })

  it('blocks a submit that never probed and points at 测试连接 instead of asking for hex', async () => {
    renderForm()

    fireEvent.change(screen.getByTestId('add-host-display-name'), { target: { value: '构建机' } })
    fireEvent.change(screen.getByTestId('add-host-target'), { target: { value: 'build-box' } })
    fireEvent.click(screen.getByTestId('add-host-submit'))

    expect(screen.getByTestId('add-host-validation').textContent).toBe(
      zh.hosts.add.requireMachineIdHash,
    )
    expect(hostsCreate).not.toHaveBeenCalled()
  })

  /**
   * The double-counting guard: a hash read from machine A must never be registered against
   * machine B, so editing the ssh target drops it and forces a fresh probe.
   */
  it('drops the hash when the ssh target changes and re-blocks the submit', async () => {
    renderForm()

    fireEvent.change(screen.getByTestId('add-host-display-name'), { target: { value: '构建机' } })
    enterHostAndTest()
    await screen.findByTestId('probe-success')
    expect(machineIdInput().value).toBe(REMOTE_MACHINE_ID_HASH)

    fireEvent.change(screen.getByTestId('add-host-target'), { target: { value: 'other-box' } })

    expect(machineIdInput().value).toBe('')
    expect(machineIdInput().readOnly).toBe(false)
    expect(screen.queryByTestId('probe-success')).toBeNull()

    fireEvent.click(screen.getByTestId('add-host-submit'))
    expect(screen.getByTestId('add-host-validation').textContent).toBe(
      zh.hosts.add.requireMachineIdHash,
    )
    expect(hostsCreate).not.toHaveBeenCalled()
  })

  it('keeps the hash when a field that cannot change the machine is edited', async () => {
    renderForm()

    enterHostAndTest()
    await screen.findByTestId('probe-success')

    fireEvent.change(screen.getByTestId('add-host-data-dir'), {
      target: { value: '/srv/opencode' },
    })
    fireEvent.change(screen.getByTestId('add-host-identity'), {
      target: { value: '~/.ssh/id_ed25519' },
    })

    expect(machineIdInput().value).toBe(REMOTE_MACHINE_ID_HASH)
    expect(machineIdInput().readOnly).toBe(true)
  })
})
