/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**` and the `zh.hosts` dictionary
 * section. No other worker edits this directory; this worker edits no shell file.
 *
 * Layout: the auto-registered local card, the add-SSH-host form with its connection probe,
 * the keyring credential card for the selected host, then the host list.
 *
 * Both reads live here rather than inside the children so a single failure renders one
 * shared error panel instead of several partial ones, and so the list and the scheduler
 * status can never disagree about which hosts exist.
 */
import { useCallback, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { ErrorState, LoadingState } from '@/components/app-state'
import type { RefreshEvent, SourceStatus } from '@/generated'
import { zh } from '@/i18n/zh'
import { getRefreshStatus, hostsList } from '@/lib/ipc'

import { AddSshHostForm } from './AddSshHostForm'
import { CredentialCard } from './CredentialCard'
import { HostList } from './HostList'
import { joinHostStatus } from './hostsModel'
import { LocalHostCard } from './LocalHostCard'
import { HOSTS_QUERY_KEY, REFRESH_STATUS_QUERY_KEY } from './queryKeys'

function upsertStatus(statuses: SourceStatus[], status: SourceStatus): SourceStatus[] {
  const existing = statuses.findIndex((candidate) => candidate.hostId === status.hostId)
  if (existing === -1) return [...statuses, status]
  return statuses.map((candidate, index) => (index === existing ? status : candidate))
}

function applyRefreshEvent(statuses: SourceStatus[], event: RefreshEvent): SourceStatus[] {
  switch (event.event) {
    case 'started':
      return upsertStatus(statuses, event.data.status)
    case 'finished':
      return event.data.status === null
        ? statuses.filter((status) => status.hostId !== event.data.hostId)
        : upsertStatus(statuses, event.data.status)
  }
}

export function HostsView() {
  const [selectedHostId, setSelectedHostId] = useState<string | null>(null)
  const queryClient = useQueryClient()

  const hosts = useQuery({ queryKey: HOSTS_QUERY_KEY, queryFn: hostsList })
  const statuses = useQuery({
    queryKey: REFRESH_STATUS_QUERY_KEY,
    queryFn: getRefreshStatus,
    staleTime: Infinity,
    gcTime: Infinity,
  })
  const onRefreshEvent = useCallback(
    (event: RefreshEvent) => {
      queryClient.setQueryData<SourceStatus[]>(REFRESH_STATUS_QUERY_KEY, (current) =>
        applyRefreshEvent(current ?? [], event),
      )
    },
    [queryClient],
  )

  const failure = hosts.error ?? statuses.error
  const rows = joinHostStatus(hosts.data ?? [], statuses.data ?? [])

  return (
    <section data-testid="view-hosts" className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 className="text-2xl font-semibold tracking-tight">{zh.hosts.title}</h2>
        <p className="text-sm text-muted-foreground">{zh.hosts.subtitle}</p>
      </div>

      {failure !== null ? (
        <ErrorState
          error={failure}
          onRetry={() => {
            void hosts.refetch()
            void statuses.refetch()
          }}
        />
      ) : (
        <>
          <LocalHostCard hosts={hosts.data ?? []} hostsLoaded={hosts.isSuccess} />
          <AddSshHostForm onCreated={(host) => setSelectedHostId(host.hostId)} />
          <CredentialCard hostId={selectedHostId} />
          {hosts.isPending || statuses.isPending ? (
            <LoadingState />
          ) : (
            <HostList
              rows={rows}
              selectedHostId={selectedHostId}
              onSelect={setSelectedHostId}
              onRefreshEvent={onRefreshEvent}
            />
          )}
        </>
      )}
    </section>
  )
}
