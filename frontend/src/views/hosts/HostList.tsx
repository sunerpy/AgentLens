/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Host rows: scheduler state, last success, the failure's Chinese remediation, and a
 * manual refresh button.
 *
 * `trigger_refresh` returns a tagged outcome, and `alreadyRunning` is rendered as its own
 * visible state rather than swallowed: a button that silently does nothing reads as a bug,
 * and the user needs to know a round is already in flight.
 *
 * Tone discipline matches the other four views — red is reserved for a genuine failure.
 * `alreadyRunning`, `idle` and "never succeeded" are ordinary states and stay neutral.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import type { TriggerRefreshResult } from '@/generated'
import { zh } from '@/i18n/zh'
import { invalidateArchiveQueries } from '@/lib/archiveQueries'
import { hostsDelete, triggerRefresh } from '@/lib/ipc'

import { HostBadge } from './HostBadge'
import { HostsErrorPanel } from './HostsErrorPanel'
import {
  formatUtcTimestamp,
  hostErrorText,
  hostLastSuccessUtc,
  hostStateKey,
  hostStateLabel,
  type HostRowModel,
} from './hostsModel'
import { HOSTS_QUERY_KEY, REFRESH_STATUS_QUERY_KEY } from './queryKeys'

function HostRow({
  row,
  selected,
  onSelect,
}: {
  row: HostRowModel
  selected: boolean
  onSelect: (hostId: string) => void
}) {
  const queryClient = useQueryClient()
  const { host, status } = row
  const state = hostStateKey(status)
  const errorText = hostErrorText(status)
  const lastSuccess = formatUtcTimestamp(hostLastSuccessUtc(row))

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: HOSTS_QUERY_KEY })
    await queryClient.invalidateQueries({ queryKey: REFRESH_STATUS_QUERY_KEY })
  }

  // `started` means in flight, not committed, so this is only the optimistic half of the
  // dashboard refresh; the authoritative one is the `archive-committed` event `main.tsx`
  // subscribes to. Host removal stays out: it deletes the hosts row, never archived usage.
  const refresh = useMutation<TriggerRefreshResult, unknown, void>({
    mutationFn: () => triggerRefresh(host.hostId),
    onSuccess: async () => {
      await invalidate()
      await invalidateArchiveQueries(queryClient)
    },
  })
  const remove = useMutation({
    mutationFn: () => hostsDelete(host.hostId),
    onSuccess: invalidate,
  })

  const outcome = refresh.data?.outcome

  return (
    <li
      data-testid={`host-row-${host.hostId}`}
      data-host-kind={host.kind}
      data-host-state={state}
      data-selected={String(selected)}
      className="flex flex-col gap-2 border-t border-border px-4 py-3 first:border-t-0"
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium" data-testid={`host-name-${host.hostId}`}>
          {host.displayName}
        </span>
        <HostBadge tone="muted">
          {host.kind === 'local' ? zh.hosts.list.kindLocal : zh.hosts.list.kindSsh}
        </HostBadge>
        <HostBadge
          tone={state === 'error' ? 'warning' : 'neutral'}
          data-testid={`host-state-${host.hostId}`}
        >
          {hostStateLabel(state)}
        </HostBadge>
        {status !== undefined ? (
          <HostBadge tone="muted">
            {status.trigger === 'auto' ? zh.hosts.list.triggerAuto : zh.hosts.list.triggerManual}
          </HostBadge>
        ) : null}
        {status?.interrupted === true ? (
          <HostBadge tone="muted" data-testid={`host-interrupted-${host.hostId}`}>
            {zh.hosts.list.interrupted}
          </HostBadge>
        ) : null}
        {host.sshTarget !== null ? (
          <span className="font-mono text-xs text-muted-foreground">{host.sshTarget}</span>
        ) : null}
        <span className="ml-auto flex items-center gap-2">
          <Button
            size="sm"
            variant="ghost"
            data-testid={`host-credentials-${host.hostId}`}
            onClick={() => onSelect(host.hostId)}
          >
            {zh.hosts.list.manageCredentials}
          </Button>
          <Button
            size="sm"
            variant="outline"
            data-testid={`host-refresh-${host.hostId}`}
            disabled={refresh.isPending}
            onClick={() => refresh.mutate()}
          >
            {refresh.isPending ? zh.hosts.list.refreshing : zh.hosts.list.refresh}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            data-testid={`host-delete-${host.hostId}`}
            disabled={remove.isPending}
            onClick={() => remove.mutate()}
          >
            {remove.isPending ? zh.hosts.list.deleting : zh.hosts.list.delete}
          </Button>
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span>{zh.hosts.list.columnLastSuccess}：</span>
        <span data-testid={`host-last-success-${host.hostId}`}>
          {lastSuccess ?? zh.hosts.list.never}
        </span>
        {outcome !== undefined ? (
          <HostBadge tone="muted" data-testid={`host-refresh-outcome-${host.hostId}`}>
            {outcome === 'alreadyRunning' ? zh.hosts.list.alreadyRunning : zh.hosts.list.started}
          </HostBadge>
        ) : null}
      </div>

      {errorText !== null ? (
        <div
          data-testid={`host-error-${host.hostId}`}
          role="alert"
          className="flex flex-col gap-1 rounded-lg border border-destructive/40 bg-destructive/5 p-3"
        >
          <span className="text-xs font-semibold text-destructive">{zh.hosts.list.errorTitle}</span>
          <span className="text-sm text-foreground">{errorText}</span>
        </div>
      ) : null}

      {refresh.isError ? (
        <HostsErrorPanel testId={`host-refresh-error-${host.hostId}`} error={refresh.error} />
      ) : null}
      {remove.isError ? (
        <HostsErrorPanel testId={`host-delete-error-${host.hostId}`} error={remove.error} />
      ) : null}
    </li>
  )
}

export function HostList({
  rows,
  selectedHostId,
  onSelect,
}: {
  rows: readonly HostRowModel[]
  selectedHostId: string | null
  onSelect: (hostId: string) => void
}) {
  return (
    <Card data-testid="host-list">
      <CardHeader>
        <CardTitle>{zh.hosts.list.title}</CardTitle>
      </CardHeader>
      <CardContent className="p-0 pb-4">
        {rows.length === 0 ? (
          <div
            data-testid="host-list-empty"
            className="flex flex-col items-center gap-1 px-4 py-8 text-sm text-muted-foreground"
          >
            <span>{zh.hosts.list.empty}</span>
            <span className="text-xs">{zh.hosts.list.emptyHint}</span>
          </div>
        ) : (
          <ul data-testid="host-rows" className="flex flex-col">
            {rows.map((row) => (
              <HostRow
                key={row.host.hostId}
                row={row}
                selected={row.host.hostId === selectedHostId}
                onSelect={onSelect}
              />
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  )
}
