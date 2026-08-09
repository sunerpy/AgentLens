/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Keyring credential sub-card.
 *
 * The input is cleared the moment the write succeeds and the UI never asks the backend for
 * the plaintext back — `credential_status` returns only a boolean. That is why a saved
 * entry shows as "已存入钥匙串" instead of a masked value: there is nothing to mask,
 * because the secret exists only in the OS keyring.
 */
import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { Button } from '@/components/ui/button'
import type { CredentialKind } from '@/generated'
import { zh } from '@/i18n/zh'

import { HostBadge } from './HostBadge'
import { CONTROL_CLASS, HostField } from './HostField'
import { HostsErrorPanel } from './HostsErrorPanel'
import { credentialDelete, credentialSet, credentialStatus } from './hostsIpc'

function credentialQueryKey(hostId: string, kind: CredentialKind) {
  return ['hosts', 'credential', hostId, kind] as const
}

function CredentialRow({
  hostId,
  kind,
  label,
  testId,
}: {
  hostId: string
  kind: CredentialKind
  label: string
  testId: string
}) {
  const queryClient = useQueryClient()
  const [secret, setSecret] = useState('')
  const queryKey = credentialQueryKey(hostId, kind)
  const status = useQuery({
    queryKey,
    queryFn: () => credentialStatus(hostId, kind),
  })

  const save = useMutation({
    mutationFn: (value: string) => credentialSet(hostId, kind, value),
    onSuccess: async () => {
      setSecret('')
      await queryClient.invalidateQueries({ queryKey })
    },
  })
  const remove = useMutation({
    mutationFn: () => credentialDelete(hostId, kind),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey })
    },
  })

  const present = status.data?.present === true
  const inputId = `${testId}-input`

  return (
    <div className="flex flex-col gap-2" data-testid={testId} data-present={String(present)}>
      <HostField id={inputId} label={label}>
        <div className="flex flex-wrap items-center gap-2">
          <input
            id={inputId}
            data-testid={`${testId}-value`}
            type="password"
            autoComplete="off"
            className={`${CONTROL_CLASS} min-w-56 flex-1`}
            placeholder={zh.hosts.credentials.placeholder}
            value={secret}
            onChange={(event) => setSecret(event.target.value)}
          />
          <Button
            size="sm"
            variant="outline"
            data-testid={`${testId}-save`}
            disabled={secret.trim() === '' || save.isPending}
            onClick={() => save.mutate(secret)}
          >
            {save.isPending ? zh.hosts.credentials.saving : zh.hosts.credentials.save}
          </Button>
          {present ? (
            <Button
              size="sm"
              variant="ghost"
              data-testid={`${testId}-remove`}
              disabled={remove.isPending}
              onClick={() => remove.mutate()}
            >
              {zh.hosts.credentials.remove}
            </Button>
          ) : null}
          <HostBadge tone="muted" data-testid={`${testId}-state`}>
            {present ? zh.hosts.credentials.stored : zh.hosts.credentials.absent}
          </HostBadge>
        </div>
      </HostField>
      {save.isError ? <HostsErrorPanel testId={`${testId}-error`} error={save.error} /> : null}
      {remove.isError ? (
        <HostsErrorPanel testId={`${testId}-remove-error`} error={remove.error} />
      ) : null}
    </div>
  )
}

export function CredentialCard({ hostId }: { hostId: string | null }) {
  return (
    <div
      className="flex flex-col gap-3 rounded-lg border border-border p-4"
      data-testid="credential-card"
    >
      <div className="flex flex-col gap-0.5">
        <span className="text-sm font-semibold">{zh.hosts.credentials.title}</span>
        <span className="text-xs text-muted-foreground">{zh.hosts.credentials.description}</span>
        <span className="text-xs text-muted-foreground" data-testid="credential-when-used">
          {zh.hosts.credentials.whenUsed}
        </span>
        <span className="text-xs text-muted-foreground">{zh.hosts.credentials.neverEchoed}</span>
        <span className="text-xs text-muted-foreground" data-testid="credential-absent-hint">
          {zh.hosts.credentials.absentHint}
        </span>
        {hostId === null ? null : (
          <span className="text-xs text-muted-foreground">
            {zh.hosts.credentials.forHost}：
            <code className="font-mono" data-testid="credential-host-id">
              {hostId}
            </code>
          </span>
        )}
      </div>
      {hostId === null ? (
        <p data-testid="credential-requires-host" className="text-sm text-muted-foreground">
          {zh.hosts.credentials.requireHost}
        </p>
      ) : (
        <>
          <CredentialRow
            hostId={hostId}
            kind="password"
            label={zh.hosts.credentials.password}
            testId="credential-password"
          />
          <CredentialRow
            hostId={hostId}
            kind="passphrase"
            label={zh.hosts.credentials.passphrase}
            testId="credential-passphrase"
          />
        </>
      )}
    </div>
  )
}
