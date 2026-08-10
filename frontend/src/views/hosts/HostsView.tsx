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
import { useCallback, useMemo, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { ErrorState, LoadingState } from '@/components/app-state'
import type { Host, RefreshEvent, SourceStatus } from '@/generated'
import { zh } from '@/i18n/zh'
import { getRefreshStatus, hostsList, hostsSupportedSources } from '@/lib/ipc'

import { AddSshHostForm } from './AddSshHostForm'
import { CredentialCard } from './CredentialCard'
import { HostList } from './HostList'
import { joinHostStatus } from './hostsModel'
import { LocalHostCard } from './LocalHostCard'
import { HOSTS_QUERY_KEY, REFRESH_STATUS_QUERY_KEY, SUPPORTED_SOURCES_QUERY_KEY } from './queryKeys'

/**
 * The scheduler's identity is `(host_id, source)`, so both halves must match. Keying on
 * `hostId` alone would let a finished OpenCode round overwrite a running Claude Code round on
 * the same host.
 */
function upsertStatus(statuses: SourceStatus[], status: SourceStatus): SourceStatus[] {
  const existing = statuses.findIndex(
    (candidate) => candidate.hostId === status.hostId && candidate.source === status.source,
  )
  if (existing === -1) return [...statuses, status]
  return statuses.map((candidate, index) => (index === existing ? status : candidate))
}

/**
 * Frozen empty fallbacks.
 *
 * `?? []` would allocate a new array on every render, and `HostList`'s rows are memoised on the
 * identity of exactly these values — a fresh literal per render silently re-renders every row
 * while a refresh round is in flight, which is the scroll stutter this file is guarding against.
 */
const NO_HOSTS: readonly Host[] = []
const NO_STATUSES: readonly SourceStatus[] = []
const NO_SOURCES: readonly string[] = []

/**
 * `upsertStatus` preserves the identity of every entry it does not replace, so this rebuild only
 * invalidates the one slot the event describes. `HostRow`'s comparator reads exactly that.
 */
function applyRefreshEvent(statuses: SourceStatus[], event: RefreshEvent): SourceStatus[] {
  switch (event.event) {
    case 'started':
      return upsertStatus(statuses, event.data.status)
    case 'finished':
      return event.data.status === null
        ? statuses.filter(
            (status) =>
              !(status.hostId === event.data.hostId && status.source === event.data.source),
          )
        : upsertStatus(statuses, event.data.status)
  }
}

export function HostsView() {
  const [selectedHostId, setSelectedHostId] = useState<string | null>(null)
  const queryClient = useQueryClient()

  const hosts = useQuery({ queryKey: HOSTS_QUERY_KEY, queryFn: hostsList })
  /**
   * The legal source keys, read from `SUPPORTED_SOURCES` rather than written here. Cached
   * forever because it is a compile-time constant, and deliberately kept out of `failure`
   * below: if this one read fails the source picker simply does not render, which is a far
   * better degradation than replacing the whole hosts view with an error panel.
   */
  const supportedSources = useQuery({
    queryKey: SUPPORTED_SOURCES_QUERY_KEY,
    queryFn: hostsSupportedSources,
    staleTime: Infinity,
    gcTime: Infinity,
  })
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
  const rows = useMemo(
    () => joinHostStatus(hosts.data ?? NO_HOSTS, statuses.data ?? NO_STATUSES),
    [hosts.data, statuses.data],
  )

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
              supportedSources={supportedSources.data ?? NO_SOURCES}
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
