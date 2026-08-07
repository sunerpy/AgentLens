/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Add-SSH-host form plus the "测试连接" probe.
 *
 * Two backend calls, deliberately separate:
 * - `ssh_probe` runs the transport's STAGE1 and reports the remote architecture, the
 *   discovered data directory, and the SHA-256 of the remote machine id. That hash is
 *   auto-filled into the form and the field turns read-only: it is a fact read off the
 *   remote, not something an operator can know or should retype. Failures come back as
 *   typed variants whose Chinese `remediation()` travels in `IpcError.fields.remediation`,
 *   which is what gets rendered — never the bare variant name, which would be useless to a
 *   user.
 * - `hosts_create` inserts the row. A second host on the same machine is rejected by the
 *   backend, and that rejection text is surfaced verbatim rather than re-worded here,
 *   because it explains the actual consequence (double-counted usage).
 */
import { useRef, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { Host, SshProbeResult } from '@/generated'
import { zh } from '@/i18n/zh'
import { hostsCreate } from '@/lib/ipc'

import { HostBadge } from './HostBadge'
import { CONTROL_CLASS, HostField } from './HostField'
import { HostsErrorPanel } from './HostsErrorPanel'
import { newSshProbeRequestId, sshProbe, sshProbeCancel } from './hostsIpc'
import { composeSshTarget, formatKib, isMachineIdHash, needsKeyFileGuidance } from './hostsModel'
import { HOSTS_QUERY_KEY, REFRESH_STATUS_QUERY_KEY } from './queryKeys'

interface FormState {
  displayName: string
  host: string
  user: string
  identityFile: string
  dataDir: string
  machineIdHash: string
}

const EMPTY_FORM: FormState = {
  displayName: '',
  host: '',
  user: '',
  identityFile: '',
  dataDir: '',
  machineIdHash: '',
}

function optional(value: string): string | null {
  const trimmed = value.trim()
  return trimmed === '' ? null : trimmed
}

export function AddSshHostForm({ onCreated }: { onCreated: (host: Host) => void }) {
  const queryClient = useQueryClient()
  const [form, setForm] = useState<FormState>(EMPTY_FORM)
  const [probedTarget, setProbedTarget] = useState<string | null>(null)
  const [validation, setValidation] = useState<string | null>(null)
  const [isCancellingProbe, setIsCancellingProbe] = useState(false)
  const probeRequestId = useRef<string | null>(null)

  const patch = (key: keyof FormState) => (value: string) =>
    setForm((current) => ({ ...current, [key]: value }))

  /**
   * The probe is the only writer of `machineIdHash`: the remote computes SHA-256 over its
   * own machine id and `parse_probe` already constrains it to 64 lowercase hex, so the
   * operator never retypes it. `probedTarget` records which ssh target it was read from.
   */
  const probe = useMutation<SshProbeResult, unknown, void>({
    mutationFn: () => {
      const requestId = newSshProbeRequestId()
      probeRequestId.current = requestId
      return sshProbe(
        {
          sshTarget: composeSshTarget(form.user, form.host),
          identityFile: optional(form.identityFile),
          remoteDataDir: optional(form.dataDir),
        },
        requestId,
      )
    },
    onSuccess: (result) => {
      setProbedTarget(composeSshTarget(form.user, form.host))
      setForm((current) => ({ ...current, machineIdHash: result.machineIdHash }))
      setValidation(null)
    },
    onSettled: () => {
      probeRequestId.current = null
      setIsCancellingProbe(false)
    },
  })

  async function cancelProbe() {
    const requestId = probeRequestId.current
    if (requestId === null || isCancellingProbe) return
    setIsCancellingProbe(true)
    try {
      await sshProbeCancel(requestId)
    } catch {
      setIsCancellingProbe(false)
    }
  }

  const create = useMutation({
    mutationFn: hostsCreate,
    onSuccess: async (host) => {
      setForm(EMPTY_FORM)
      setProbedTarget(null)
      probe.reset()
      await queryClient.invalidateQueries({ queryKey: HOSTS_QUERY_KEY })
      await queryClient.invalidateQueries({ queryKey: REFRESH_STATUS_QUERY_KEY })
      onCreated(host)
    },
  })

  function validate(requireIdentity: boolean): boolean {
    if (form.host.trim() === '') {
      setValidation(zh.hosts.add.requireHost)
      return false
    }
    if (requireIdentity && form.displayName.trim() === '') {
      setValidation(zh.hosts.add.requireDisplayName)
      return false
    }
    if (requireIdentity && !isMachineIdHash(form.machineIdHash)) {
      setValidation(zh.hosts.add.requireMachineIdHash)
      return false
    }
    setValidation(null)
    return true
  }

  /**
   * `host` and `user` are the only two fields that decide WHICH machine gets probed, so
   * editing either can turn an auto-filled hash into another machine's identity — and
   * registering that would file this host's usage under the wrong machine. Both go through
   * here so there is exactly one path that can invalidate the hash.
   */
  const patchTarget = (key: 'host' | 'user') => (value: string) => {
    const next = { ...form, [key]: value }
    const stale = probedTarget !== null && composeSshTarget(next.user, next.host) !== probedTarget
    setForm(stale ? { ...next, machineIdHash: '' } : next)
    if (stale) {
      setProbedTarget(null)
      probe.reset()
    }
  }

  return (
    <Card data-testid="add-ssh-host">
      <CardHeader>
        <CardTitle>{zh.hosts.add.title}</CardTitle>
        <CardDescription>{zh.hosts.add.description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <HostField id="add-host-display-name" label={zh.hosts.add.displayName}>
            <input
              id="add-host-display-name"
              data-testid="add-host-display-name"
              className={CONTROL_CLASS}
              placeholder={zh.hosts.add.displayNamePlaceholder}
              value={form.displayName}
              onChange={(event) => patch('displayName')(event.target.value)}
            />
          </HostField>
          <HostField id="add-host-target" label={zh.hosts.add.host}>
            <input
              id="add-host-target"
              data-testid="add-host-target"
              className={CONTROL_CLASS}
              placeholder={zh.hosts.add.hostPlaceholder}
              value={form.host}
              onChange={(event) => patchTarget('host')(event.target.value)}
            />
          </HostField>
          <HostField id="add-host-user" label={zh.hosts.add.user}>
            <input
              id="add-host-user"
              data-testid="add-host-user"
              className={CONTROL_CLASS}
              placeholder={zh.hosts.add.userPlaceholder}
              value={form.user}
              onChange={(event) => patchTarget('user')(event.target.value)}
            />
          </HostField>
          <HostField id="add-host-identity" label={zh.hosts.add.identityFile}>
            <input
              id="add-host-identity"
              data-testid="add-host-identity"
              className={CONTROL_CLASS}
              placeholder={zh.hosts.add.identityFilePlaceholder}
              value={form.identityFile}
              onChange={(event) => patch('identityFile')(event.target.value)}
            />
          </HostField>
          <HostField id="add-host-data-dir" label={zh.hosts.add.dataDir}>
            <input
              id="add-host-data-dir"
              data-testid="add-host-data-dir"
              className={CONTROL_CLASS}
              placeholder={zh.hosts.add.dataDirPlaceholder}
              value={form.dataDir}
              onChange={(event) => patch('dataDir')(event.target.value)}
            />
          </HostField>
          {/*
            跨满两栏。这个字段承载 64 位十六进制摘要，在 text-sm 等宽字体下约需 550px，
            超过两栏网格分给单列的宽度 —— 900px 视口下实测末尾字符被输入框右边缘裁掉，
            而它的用途正是让操作者核对机器身份，看不全等于核对不了。其余字段是短输入
            （显示名、用户名、路径），并排合适；这一个是探测结果展示，性质不同。
            包一层 div 而不给 HostField 加 className prop：HostField 被 6 处共用，
            为一个字段改共用组件的 API 不划算。
          */}
          <div className="sm:col-span-2">
            <HostField
              id="add-host-machine-id"
              label={zh.hosts.add.machineIdHash}
              hint={
                probedTarget === null
                  ? zh.hosts.add.machineIdHashHint
                  : zh.hosts.add.machineIdHashFilled
              }
            >
              <input
                id="add-host-machine-id"
                data-testid="add-host-machine-id"
                className={`${CONTROL_CLASS} font-mono read-only:bg-muted read-only:text-muted-foreground`}
                value={form.machineIdHash}
                readOnly={probedTarget !== null}
                onChange={(event) => patch('machineIdHash')(event.target.value)}
              />
            </HostField>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            data-testid="add-host-test"
            disabled={probe.isPending}
            onClick={() => {
              if (validate(false)) probe.mutate()
            }}
          >
            {probe.isPending ? zh.hosts.add.testing : zh.hosts.add.test}
          </Button>
          {probe.isPending ? (
            <Button
              size="sm"
              variant="destructive"
              data-testid="add-host-test-cancel"
              disabled={isCancellingProbe}
              onClick={() => void cancelProbe()}
            >
              {isCancellingProbe ? zh.hosts.add.cancelling : zh.hosts.add.cancelTest}
            </Button>
          ) : null}
          <Button
            size="sm"
            data-testid="add-host-submit"
            disabled={create.isPending}
            onClick={() => {
              if (!validate(true)) return
              create.mutate({
                displayName: form.displayName.trim(),
                kind: 'ssh',
                machineIdHash: form.machineIdHash.trim().toLowerCase(),
                sshTarget: composeSshTarget(form.user, form.host),
                remoteDataDir: optional(form.dataDir),
              })
            }}
          >
            {create.isPending ? zh.hosts.add.submitting : zh.hosts.add.submit}
          </Button>
        </div>

        {probe.isPending ? (
          <p data-testid="probe-pending" role="status" className="text-sm text-muted-foreground">
            {zh.hosts.add.testingHint}
          </p>
        ) : null}

        {validation !== null ? (
          <p data-testid="add-host-validation" role="alert" className="text-sm text-destructive">
            {validation}
          </p>
        ) : null}

        {probe.isSuccess ? (
          <div
            data-testid="probe-success"
            className="flex flex-col gap-2 rounded-lg border border-border bg-muted/40 p-4"
          >
            <div className="flex items-center gap-2">
              <span className="text-sm font-semibold">{zh.hosts.probe.successTitle}</span>
              <HostBadge tone="accent" data-testid="probe-architecture">
                {probe.data.architecture}
              </HostBadge>
            </div>
            <dl className="grid grid-cols-1 gap-2 text-sm sm:grid-cols-2">
              <div className="flex flex-col gap-0.5">
                <dt className="text-xs text-muted-foreground">{zh.hosts.probe.dataDir}</dt>
                <dd className="font-mono text-xs" data-testid="probe-data-dir">
                  {probe.data.dataDir}
                </dd>
              </div>
              <div className="flex flex-col gap-0.5">
                <dt className="text-xs text-muted-foreground">{zh.hosts.probe.xdgDataHome}</dt>
                <dd className="font-mono text-xs" data-testid="probe-xdg">
                  {probe.data.xdgDataHome ?? zh.hosts.probe.xdgUnset}
                </dd>
              </div>
              <div className="flex flex-col gap-0.5">
                <dt className="text-xs text-muted-foreground">{zh.hosts.probe.availableSpace}</dt>
                <dd className="text-xs" data-testid="probe-available">
                  {formatKib(probe.data.availableKib)}
                </dd>
              </div>
              <div className="flex flex-col gap-0.5">
                <dt className="text-xs text-muted-foreground">{zh.hosts.probe.machineIdSource}</dt>
                <dd className="font-mono text-xs" data-testid="probe-machine-id-source">
                  {probe.data.machineIdSource}
                </dd>
              </div>
            </dl>
          </div>
        ) : null}

        {probe.isError ? (
          <div className="flex flex-col gap-2">
            <HostsErrorPanel
              testId="probe-error"
              title={zh.hosts.probe.failureTitle}
              error={probe.error}
              remediationLabel={zh.hosts.probe.remediationLabel}
            />
            {needsKeyFileGuidance(probe.error) ? (
              <p data-testid="ssh-agent-guidance" className="text-sm text-muted-foreground">
                {zh.hosts.add.agentUnavailable}
              </p>
            ) : null}
          </div>
        ) : null}

        {create.isError ? <HostsErrorPanel testId="add-host-error" error={create.error} /> : null}
      </CardContent>
    </Card>
  )
}
