/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * The local host card. It auto-registers this machine on first load.
 *
 * Why the identity has to come from Rust: `machineIdHash` is SHA-256 over the trimmed
 * contents of `/etc/machine-id`, and registering the wrong value would split one machine
 * into two hosts and double-count its usage.
 *
 * When the machine id cannot be read at all — `/etc/machine-id` is genuinely absent in
 * some containers — the backend error already carries its own remediation, so the card
 * renders that text instead of a blank panel.
 */
import { useEffect, useRef } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { Host } from '@/generated'
import { zh } from '@/i18n/zh'
import { hostsCreate } from '@/lib/ipc'

import { HostBadge } from './HostBadge'
import { HostsErrorPanel } from './HostsErrorPanel'
import { localMachineIdentity } from './hostsIpc'
import { HOSTS_QUERY_KEY, LOCAL_IDENTITY_QUERY_KEY, REFRESH_STATUS_QUERY_KEY } from './queryKeys'

export function LocalHostCard({
  hosts,
  hostsLoaded,
}: {
  hosts: readonly Host[]
  hostsLoaded: boolean
}) {
  const queryClient = useQueryClient()
  // The machine id is this machine's identity: it cannot change while the process runs, so
  // any refetch is pure waste. Without this the query re-ran on every mount of the card,
  // which is what made navigating back to the hosts view feel slow.
  const identityQuery = useQuery({
    queryKey: LOCAL_IDENTITY_QUERY_KEY,
    queryFn: localMachineIdentity,
    staleTime: Infinity,
    gcTime: Infinity,
  })
  const identity = identityQuery.data

  const registration = useMutation({
    mutationFn: hostsCreate,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: HOSTS_QUERY_KEY })
      await queryClient.invalidateQueries({ queryKey: REFRESH_STATUS_QUERY_KEY })
    },
  })

  const registered =
    identity === undefined
      ? undefined
      : hosts.find((host) => host.machineIdHash === identity.machineIdHash)

  // Auto-registration fires at most once per mount. A ref rather than a state flag so a
  // re-render triggered by the mutation itself cannot queue a second insert, which the
  // backend would reject as a duplicate machine id.
  const attempted = useRef(false)
  useEffect(() => {
    if (identity === undefined || !hostsLoaded || registered !== undefined) return
    if (attempted.current) return
    attempted.current = true
    registration.mutate({
      displayName: identity.hostname ?? zh.hosts.local.defaultDisplayName,
      kind: 'local',
      machineIdHash: identity.machineIdHash,
      sshTarget: null,
      remoteDataDir: null,
      enabledSources: null,
    })
  }, [identity, hostsLoaded, registered, registration])

  if (identityQuery.isPending) {
    return (
      <Card data-testid="local-host-card" data-local-state="loading">
        <CardHeader>
          <CardTitle>{zh.hosts.local.title}</CardTitle>
          <CardDescription>{zh.common.loading}</CardDescription>
        </CardHeader>
      </Card>
    )
  }

  if (identityQuery.isError) {
    return (
      <Card data-testid="local-host-card" data-local-state="identityUnavailable">
        <CardHeader>
          <CardTitle>{zh.hosts.local.title}</CardTitle>
          <CardDescription>{zh.hosts.local.identityHint}</CardDescription>
        </CardHeader>
        <CardContent>
          <HostsErrorPanel
            testId="local-identity-error"
            title={zh.hosts.local.identityUnavailable}
            error={identityQuery.error}
            onRetry={() => void identityQuery.refetch()}
          />
        </CardContent>
      </Card>
    )
  }

  const state =
    registered !== undefined
      ? 'registered'
      : registration.isPending
        ? 'registering'
        : 'unregistered'

  return (
    <Card data-testid="local-host-card" data-local-state={state}>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <span data-testid="local-host-name">
            {registered?.displayName ?? identity?.hostname ?? zh.hosts.local.defaultDisplayName}
          </span>
          <HostBadge tone="accent">{zh.hosts.local.badge}</HostBadge>
          <HostBadge
            tone={state === 'registered' ? 'muted' : 'neutral'}
            data-testid="local-host-state"
          >
            {state === 'registered'
              ? zh.hosts.local.registered
              : state === 'registering'
                ? zh.hosts.local.registering
                : zh.hosts.local.unregistered}
          </HostBadge>
        </CardTitle>
        <CardDescription>{zh.hosts.local.identityHint}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <dl className="grid grid-cols-1 gap-2 text-sm sm:grid-cols-2">
          <div className="flex flex-col gap-0.5">
            <dt className="text-xs text-muted-foreground">{zh.hosts.local.hostIdLabel}</dt>
            <dd className="font-mono text-xs" data-testid="local-host-id">
              {identity?.hostId}
            </dd>
          </div>
          <div className="flex min-w-0 flex-col gap-0.5">
            <dt className="text-xs text-muted-foreground">{zh.hosts.local.identityLabel}</dt>
            <dd className="truncate font-mono text-xs" title={identity?.machineIdHash}>
              {identity?.machineIdHash}
            </dd>
          </div>
        </dl>
        {registration.isError ? (
          <HostsErrorPanel
            testId="local-register-error"
            title={zh.hosts.local.unregistered}
            error={registration.error}
          />
        ) : null}
        {state === 'unregistered' && !registration.isError ? (
          <Button
            size="sm"
            variant="outline"
            data-testid="local-register"
            onClick={() => {
              if (identity === undefined) return
              registration.mutate({
                displayName: identity.hostname ?? zh.hosts.local.defaultDisplayName,
                kind: 'local',
                machineIdHash: identity.machineIdHash,
                sshTarget: null,
                remoteDataDir: null,
                enabledSources: null,
              })
            }}
          >
            {zh.hosts.local.register}
          </Button>
        ) : null}
      </CardContent>
    </Card>
  )
}
