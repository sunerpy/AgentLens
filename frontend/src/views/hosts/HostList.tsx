/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Host rows: per-source scheduler state, last success in the report timezone, the failure's
 * Chinese remediation, a per-host refresh button and a list-wide refresh-all.
 *
 * `trigger_refresh` returns one tagged outcome **per enabled source**, and `alreadyRunning` is
 * rendered as its own visible state rather than swallowed: a button that silently does nothing
 * reads as a bug, and the user needs to know a round is already in flight.
 *
 * Tone discipline matches the other four views — red is reserved for a genuine failure.
 * `alreadyRunning`, `idle` and "never succeeded" are ordinary states and stay neutral.
 *
 * ### Why every row is memoised
 *
 * A refresh round emits one `RefreshEvent` per `(host_id, source)` slot, and each event rewrites
 * the refresh-status cache entry — after which `joinHostStatus` hands this list brand-new wrapper
 * objects and brand-new status arrays for **every** host, even the ones the event never touched.
 * Rendered plainly that means one slot going `running` re-renders every row's whole subtree
 * (headline badges, per-source list, and the source picker with its two `useMutation` hooks), so
 * scrolling during a refresh competed with React reconciliation for the main thread. That is the
 * "刷新时滚动条卡顿" report: the collection itself already runs on a Rust worker thread.
 *
 * So rows take `host` + `statuses` rather than the wrapper, and {@link sameRowProps} compares the
 * status array **element-wise by identity**: the join's fresh array is transparent, while the one
 * `SourceStatus` the scheduler actually replaced is not. Row counts here are single digits to a
 * few dozen, so this is the whole fix — no virtualisation, which would add a dependency (banned)
 * and buy nothing at this scale.
 *
 * Two consequences worth stating, because breaking either silently undoes the fix:
 *  1. Every prop a row receives must be reference-stable across renders. `HostsView` owns that
 *     for `supportedSources` / `onSelect` / `onRefreshEvent`; do not inline a fresh array or
 *     lambda at a call site.
 *  2. `rowStateKey` / `hostStateKey` are called **directly, never through `useMemo`**. Both are
 *     a `.some()` over a handful of statuses — memoising them would cost more than it saves —
 *     and `HostList.render.test.tsx` counts renders through them, which a memo hit would hide.
 */
import { memo, useCallback, useMemo, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'

import { useReportRange } from '@/app/reportRange'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import type { Host, RefreshEvent, SourceStatus, TriggerRefreshResult } from '@/generated'
import { zh } from '@/i18n/zh'
import { invalidateArchiveQueries } from '@/lib/archiveQueries'
import { hostsDelete, hostsUpdate, triggerRefresh } from '@/lib/ipc'

import { HostBadge } from './HostBadge'
import { HostsErrorPanel } from './HostsErrorPanel'
import {
  formatTimestampInZone,
  hostErrorText,
  hostLastSuccessUtc,
  hostStateKey,
  hostStateLabel,
  rowStateKey,
  type HostRowModel,
} from './hostsModel'
import { HOSTS_QUERY_KEY, REFRESH_STATUS_QUERY_KEY } from './queryKeys'

/**
 * Fans out over hosts and resolves once every round has been dispatched.
 *
 * Sequential rather than concurrent: one remote round already starts six `ssh`/`scp`
 * processes, so firing every host at once would multiply that by the host count. A rejected
 * host is collected instead of aborting the fan-out — one unreachable box must not stop the
 * others from refreshing.
 */
async function refreshEveryHost(
  hosts: readonly Host[],
  onEvent: (event: RefreshEvent) => void,
): Promise<{ rounds: number; failures: unknown[] }> {
  const failures: unknown[] = []
  let rounds = 0
  for (const host of hosts) {
    try {
      rounds += (await triggerRefresh(host.hostId, onEvent)).length
    } catch (error) {
      failures.push(error)
    }
  }
  return { rounds, failures }
}

/**
 * One scheduler slot: its source name, state and — when it failed — the backend remediation.
 *
 * Takes the already-resolved `status` instead of scanning `row.statuses` itself, so the slot the
 * refresh event replaced is the only one whose props change. `undefined` stays a legal value: a
 * source the host has enabled but the scheduler has not registered yet renders as 状态未知.
 */
const SourceRow = memo(function SourceRow({
  hostId,
  source,
  status,
}: {
  hostId: string
  source: string
  status: SourceStatus | undefined
}) {
  const state = hostStateKey(status)
  const errorText = hostErrorText(status)
  const testId = `host-source-${hostId}-${source}`

  return (
    <li
      data-testid={testId}
      data-source-state={state}
      className="flex flex-col gap-1 rounded-md bg-muted/40 px-2 py-1.5"
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-xs select-text" data-testid={`${testId}-name`}>
          {source}
        </span>
        <HostBadge tone={state === 'error' ? 'warning' : 'neutral'} data-testid={`${testId}-state`}>
          {hostStateLabel(state)}
        </HostBadge>
        {status !== undefined ? (
          <HostBadge tone="muted">
            {status.trigger === 'auto' ? zh.hosts.list.triggerAuto : zh.hosts.list.triggerManual}
          </HostBadge>
        ) : null}
        {status?.interrupted === true ? (
          <HostBadge tone="muted" data-testid={`${testId}-interrupted`}>
            {zh.hosts.list.interrupted}
          </HostBadge>
        ) : null}
      </div>
      {errorText !== null ? (
        <div
          data-testid={`${testId}-error`}
          role="alert"
          className="flex flex-col gap-1 rounded-md border border-destructive/40 bg-destructive/5 p-2"
        >
          <span className="text-xs font-semibold text-destructive">{zh.hosts.list.errorTitle}</span>
          <span className="text-sm text-foreground select-text">{errorText}</span>
        </div>
      ) : null}
    </li>
  )
})

/**
 * The per-host source picker.
 *
 * Without it `enabled_sources` was unreachable from the UI: every write path sent `null`, which
 * the backend documents as "keep the stored set", and a fresh row falls back to the column
 * default of `'opencode'` alone. Codex, Claude Code and Hermes were therefore implemented but
 * never collected, which read as "总览里只有 opencode".
 *
 * `available` comes from `hosts_supported_sources`, never from a list written here. A source the
 * host has enabled but that the export no longer contains is still rendered — dropping it would
 * silently disable it on the next save.
 */
const SourceEditor = memo(function SourceEditor({
  host,
  available,
  onRefreshEvent,
}: {
  host: Host
  available: readonly string[]
  onRefreshEvent: (event: RefreshEvent) => void
}) {
  const queryClient = useQueryClient()
  const [draft, setDraft] = useState<readonly string[] | null>(null)
  const [validation, setValidation] = useState<string | null>(null)

  const selected = draft ?? host.enabledSources
  // Two `Set` allocations per render otherwise, on a list that only changes when the host does.
  const options = useMemo(
    () => [...new Set([...available, ...host.enabledSources])],
    [available, host.enabledSources],
  )

  const firstScan = useMutation<TriggerRefreshResult[], unknown, void>({
    mutationFn: () => triggerRefresh(host.hostId, onRefreshEvent),
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: REFRESH_STATUS_QUERY_KEY })
      await invalidateArchiveQueries(queryClient)
    },
  })

  const save = useMutation({
    mutationFn: (sources: readonly string[]) =>
      hostsUpdate({
        hostId: host.hostId,
        displayName: host.displayName,
        kind: host.kind,
        sshTarget: host.sshTarget,
        remoteDataDir: host.remoteDataDir,
        enabledSources: [...sources],
      }),
    // `sources` is the submitted set and `host.enabledSources` the closure's pre-save value, so
    // the difference is exactly what this save switched on. Only a newly enabled source needs a
    // round kicked off; a save that merely turned something off must not start scanning.
    onSuccess: async (_saved, sources) => {
      const added = sources.filter((source) => !host.enabledSources.includes(source))
      setDraft(null)
      await queryClient.invalidateQueries({ queryKey: HOSTS_QUERY_KEY })
      await queryClient.invalidateQueries({ queryKey: REFRESH_STATUS_QUERY_KEY })
      if (added.length > 0) firstScan.mutate()
    },
  })
  const { mutate: saveMutate, reset: saveReset } = save

  const scanOutcomes = firstScan.data
  const scanStatus = firstScan.isPending
    ? zh.hosts.list.sourcesScanning
    : scanOutcomes === undefined || scanOutcomes.length === 0
      ? null
      : scanOutcomes.every((result) => result.outcome === 'alreadyRunning')
        ? zh.hosts.list.sourcesScanAlreadyRunning
        : zh.hosts.list.sourcesScanStarted

  const dirty =
    draft !== null &&
    (draft.length !== host.enabledSources.length ||
      draft.some((source) => !host.enabledSources.includes(source)))

  const toggle = useCallback(
    (source: string, checked: boolean) => {
      const next = checked ? [...selected, source] : selected.filter((each) => each !== source)
      setDraft(options.filter((candidate) => next.includes(candidate)))
      setValidation(null)
      saveReset()
    },
    [options, saveReset, selected],
  )

  const submit = useCallback(() => {
    if (selected.length === 0) {
      setValidation(zh.hosts.list.sourcesRequireOne)
      return
    }
    setValidation(null)
    saveMutate(selected)
  }, [saveMutate, selected])

  if (options.length === 0) return null

  return (
    <div className="flex flex-col gap-2 rounded-md border border-border bg-muted/20 p-2">
      <div className="flex flex-col gap-0.5">
        <span className="text-xs font-medium select-none">{zh.hosts.list.sourcesEditTitle}</span>
        <span className="text-xs text-muted-foreground">{zh.hosts.list.sourcesEditHint}</span>
      </div>
      <div
        data-testid={`host-sources-edit-${host.hostId}`}
        className="flex flex-wrap items-center gap-x-4 gap-y-2"
      >
        {options.map((source) => {
          const inputId = `host-source-toggle-${host.hostId}-${source}`
          return (
            <label key={source} htmlFor={inputId} className="flex items-center gap-1.5 text-xs">
              <input
                id={inputId}
                type="checkbox"
                data-testid={inputId}
                className="size-3.5 accent-primary"
                checked={selected.includes(source)}
                onChange={(event) => toggle(source, event.target.checked)}
              />
              <span className="font-mono">{source}</span>
            </label>
          )
        })}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          variant="outline"
          data-testid={`host-sources-save-${host.hostId}`}
          disabled={!dirty || save.isPending}
          onClick={submit}
        >
          {save.isPending ? zh.hosts.list.sourcesSaving : zh.hosts.list.sourcesSave}
        </Button>
        {save.isSuccess && !dirty ? (
          <span
            data-testid={`host-sources-saved-${host.hostId}`}
            className="text-xs text-muted-foreground"
          >
            {zh.hosts.list.sourcesSaved}
          </span>
        ) : (
          <span className="text-xs text-muted-foreground select-none">
            {zh.hosts.list.sourcesFirstScanHint}
          </span>
        )}
        {scanStatus === null ? null : (
          <span
            data-testid={`host-sources-scan-${host.hostId}`}
            className="text-xs text-muted-foreground"
          >
            {scanStatus}
          </span>
        )}
        {validation !== null ? (
          <span
            data-testid={`host-sources-validation-${host.hostId}`}
            role="alert"
            className="text-xs text-destructive"
          >
            {validation}
          </span>
        ) : null}
      </div>
      {save.isError ? (
        <HostsErrorPanel testId={`host-sources-error-${host.hostId}`} error={save.error} />
      ) : null}
      {firstScan.isError ? (
        <HostsErrorPanel
          testId={`host-sources-scan-error-${host.hostId}`}
          error={firstScan.error}
        />
      ) : null}
    </div>
  )
})

interface HostRowProps {
  host: Host
  statuses: readonly SourceStatus[]
  supportedSources: readonly string[]
  selected: boolean
  timezone: string
  onSelect: (hostId: string) => void
  onRefreshEvent: (event: RefreshEvent) => void
}

/**
 * Element-wise on `statuses` and identity on everything else.
 *
 * `joinHostStatus` allocates a fresh array per host on every refresh event, so the default
 * shallow comparison would report a change for every row while only one slot actually moved.
 * The individual `SourceStatus` objects are the honest signal: the scheduler replaces exactly the
 * one it updated and TanStack's structural sharing preserves the rest.
 */
function sameRowProps(previous: HostRowProps, next: HostRowProps): boolean {
  return (
    previous.host === next.host &&
    previous.supportedSources === next.supportedSources &&
    previous.selected === next.selected &&
    previous.timezone === next.timezone &&
    previous.onSelect === next.onSelect &&
    previous.onRefreshEvent === next.onRefreshEvent &&
    previous.statuses.length === next.statuses.length &&
    previous.statuses.every((status, index) => status === next.statuses[index])
  )
}

const HostRow = memo(function HostRow({
  host,
  statuses,
  supportedSources,
  selected,
  timezone,
  onSelect,
  onRefreshEvent,
}: HostRowProps) {
  const queryClient = useQueryClient()
  const state = rowStateKey(statuses)
  const lastSuccess = useMemo(
    () => formatTimestampInZone(hostLastSuccessUtc({ host, statuses }), timezone),
    [host, statuses, timezone],
  )

  /**
   * `enabledSources` is what the host is configured to collect; the scheduler may not have
   * registered a slot yet. Rendering the union means a configured-but-unregistered source
   * shows as "状态未知" instead of vanishing.
   *
   * Resolved to `(source, status)` pairs here rather than per-`SourceRow` so the lookup happens
   * once per row instead of once per slot, and so each slot's props change only when its own
   * status does.
   */
  const slots = useMemo(
    () =>
      [...new Set([...host.enabledSources, ...statuses.map((status) => status.source)])].map(
        (source) => ({
          source,
          status: statuses.find((candidate) => candidate.source === source),
        }),
      ),
    [host.enabledSources, statuses],
  )

  const invalidate = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: HOSTS_QUERY_KEY })
    await queryClient.invalidateQueries({ queryKey: REFRESH_STATUS_QUERY_KEY })
  }, [queryClient])

  const refresh = useMutation<TriggerRefreshResult[], unknown, void>({
    mutationFn: () => triggerRefresh(host.hostId, onRefreshEvent),
    onSuccess: async () => {
      await invalidateArchiveQueries(queryClient)
    },
  })
  const remove = useMutation({
    mutationFn: () => hostsDelete(host.hostId),
    onSuccess: invalidate,
  })
  const { mutate: refreshMutate } = refresh
  const { mutate: removeMutate } = remove

  /**
   * `mutate` — not the whole mutation result — is the dependency: TanStack keeps `mutate` stable
   * across renders while the result object is rebuilt every time, so depending on the latter
   * would hand every child a fresh callback and defeat the memoisation above.
   */
  const handleSelect = useCallback(() => onSelect(host.hostId), [host.hostId, onSelect])
  const handleRefresh = useCallback(() => refreshMutate(), [refreshMutate])
  const handleDelete = useCallback(() => removeMutate(), [removeMutate])

  /**
   * `alreadyRunning` only when *every* dispatched round said so. Any started round means the
   * click did something, and reporting "已在刷新中" then would be a lie.
   */
  const outcomes = refresh.data
  const outcome =
    outcomes === undefined || outcomes.length === 0
      ? undefined
      : outcomes.every((result) => result.outcome === 'alreadyRunning')
        ? 'alreadyRunning'
        : 'started'

  return (
    <li
      data-testid={`host-row-${host.hostId}`}
      data-host-kind={host.kind}
      data-host-state={state}
      data-selected={String(selected)}
      className="flex flex-col gap-2 border-t border-border px-4 py-3 first:border-t-0"
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium select-text" data-testid={`host-name-${host.hostId}`}>
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
        {host.sshTarget !== null ? (
          <span className="font-mono text-xs text-muted-foreground select-text">
            {host.sshTarget}
          </span>
        ) : null}
        <span className="ml-auto flex items-center gap-2">
          {/* Local hosts read their files directly through `LocalHostSource`; no SSH is
              involved, so a keyring credential could never be used. Offering the button
              would invite the user to configure something that does nothing. */}
          {host.kind === 'local' ? null : (
            <Button
              size="sm"
              variant="ghost"
              data-testid={`host-credentials-${host.hostId}`}
              onClick={handleSelect}
            >
              {zh.hosts.list.manageCredentials}
            </Button>
          )}
          <Button
            size="sm"
            variant="outline"
            data-testid={`host-refresh-${host.hostId}`}
            disabled={refresh.isPending}
            onClick={handleRefresh}
          >
            {refresh.isPending ? zh.hosts.list.refreshing : zh.hosts.list.refresh}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            data-testid={`host-delete-${host.hostId}`}
            disabled={remove.isPending}
            onClick={handleDelete}
          >
            {remove.isPending ? zh.hosts.list.deleting : zh.hosts.list.delete}
          </Button>
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span className="select-none">{zh.hosts.list.columnLastSuccess}：</span>
        <span data-testid={`host-last-success-${host.hostId}`} className="tabular-nums select-text">
          {lastSuccess ?? zh.hosts.list.never}
        </span>
        {outcome !== undefined ? (
          <HostBadge tone="muted" data-testid={`host-refresh-outcome-${host.hostId}`}>
            {outcome === 'alreadyRunning' ? zh.hosts.list.alreadyRunning : zh.hosts.list.started}
          </HostBadge>
        ) : null}
      </div>

      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted-foreground select-none">
          {zh.hosts.list.columnSources}
        </span>
        {slots.length === 0 ? (
          <span
            data-testid={`host-sources-empty-${host.hostId}`}
            className="text-xs text-muted-foreground"
          >
            {zh.hosts.list.sourcesUnavailable}
          </span>
        ) : (
          <ul data-testid={`host-sources-${host.hostId}`} className="flex flex-col gap-1">
            {slots.map((slot) => (
              <SourceRow
                key={slot.source}
                hostId={host.hostId}
                source={slot.source}
                status={slot.status}
              />
            ))}
          </ul>
        )}
        <SourceEditor host={host} available={supportedSources} onRefreshEvent={onRefreshEvent} />
      </div>

      {refresh.isError ? (
        <HostsErrorPanel testId={`host-refresh-error-${host.hostId}`} error={refresh.error} />
      ) : null}
      {remove.isError ? (
        <HostsErrorPanel testId={`host-delete-error-${host.hostId}`} error={remove.error} />
      ) : null}
    </li>
  )
}, sameRowProps)

export function HostList({
  rows,
  supportedSources,
  selectedHostId,
  onSelect,
  onRefreshEvent,
}: {
  rows: readonly HostRowModel[]
  supportedSources: readonly string[]
  selectedHostId: string | null
  onSelect: (hostId: string) => void
  onRefreshEvent: (event: RefreshEvent) => void
}) {
  const queryClient = useQueryClient()
  const { timezone } = useReportRange()

  const refreshAll = useMutation({
    mutationFn: () =>
      refreshEveryHost(
        rows.map((row) => row.host),
        onRefreshEvent,
      ),
    onSuccess: async () => {
      await invalidateArchiveQueries(queryClient)
    },
  })
  const { mutate: refreshAllMutate } = refreshAll
  const handleRefreshAll = useCallback(() => refreshAllMutate(), [refreshAllMutate])

  const result = refreshAll.data

  return (
    <Card data-testid="host-list">
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2">
        <CardTitle>{zh.hosts.list.title}</CardTitle>
        <div className="flex flex-wrap items-center gap-2">
          {result !== undefined ? (
            <span data-testid="host-refresh-all-result" className="text-xs text-muted-foreground">
              {rows.length === 0
                ? zh.hosts.list.refreshAllNoHosts
                : zh.hosts.list.refreshAllDone(result.rounds)}
            </span>
          ) : (
            <span className="text-xs text-muted-foreground select-none">
              {zh.hosts.list.refreshAllHint}
            </span>
          )}
          <Button
            size="sm"
            data-testid="host-refresh-all"
            disabled={refreshAll.isPending || rows.length === 0}
            onClick={handleRefreshAll}
          >
            {refreshAll.isPending ? zh.hosts.list.refreshingAll : zh.hosts.list.refreshAll}
          </Button>
        </div>
      </CardHeader>
      <CardContent className="p-0 pb-4">
        {result !== undefined && result.failures.length > 0 ? (
          <div className="px-4 pb-3">
            <HostsErrorPanel testId="host-refresh-all-error" error={result.failures[0]} />
          </div>
        ) : null}
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
                host={row.host}
                statuses={row.statuses}
                supportedSources={supportedSources}
                selected={row.host.hostId === selectedHostId}
                timezone={timezone}
                onSelect={onSelect}
                onRefreshEvent={onRefreshEvent}
              />
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  )
}
