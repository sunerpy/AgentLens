/**
 * EXCLUSIVE FILE BOUNDARY — todo 17 owns `src/views/detail/**`.
 *
 * Filter bar: host / agent / model / is_incomplete.
 *
 * Option lists come from real IPC (`hosts_list` for hosts, `get_breakdown` for the agents and
 * models actually present in the current report window) rather than from the visible page, which
 * would make the choices change as the user pages.
 */
import { useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'

import { useReportRange } from '@/app/reportRange'
import { Button } from '@/components/ui/button'
import { zh } from '@/i18n/zh'
import { getBreakdown, hostsList } from '@/lib/ipc'
import { cn } from '@/lib/utils'

import type { DetailFilterState, DetailPageAction } from './useDetailPage'

const SELECT_CLASS =
  'h-8 min-w-36 rounded-lg border border-border bg-background px-2 text-sm text-foreground outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50'

interface Option {
  value: string
  label: string
}

function FilterField({
  id,
  label,
  testId,
  value,
  options,
  onChange,
}: {
  id: string
  label: string
  testId: string
  value: string
  options: Option[]
  onChange: (value: string) => void
}) {
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className="text-xs font-medium text-muted-foreground">
        {label}
      </label>
      <select
        id={id}
        data-testid={testId}
        className={SELECT_CLASS}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="">{zh.detail.filters.any}</option>
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    </div>
  )
}

function incompleteToSelectValue(value: boolean | null): string {
  if (value === null) return ''
  return value ? 'true' : 'false'
}

export function DetailFilterBar({
  filters,
  dispatch,
}: {
  filters: DetailFilterState
  dispatch: (action: DetailPageAction) => void
}) {
  const { range, timezone } = useReportRange()

  const hosts = useQuery({ queryKey: ['detail', 'hosts'] as const, queryFn: hostsList })
  const breakdown = useQuery({
    queryKey: ['detail', 'breakdown', range, timezone] as const,
    queryFn: () =>
      getBreakdown(range, {
        timezone,
        filters: { hostId: null, source: null, agentKey: null, providerId: null, modelId: null },
        expandVariant: false,
      }),
  })

  const hostOptions = useMemo<Option[]>(
    () => (hosts.data ?? []).map((host) => ({ value: host.hostId, label: host.displayName })),
    [hosts.data],
  )

  const agentOptions = useMemo<Option[]>(() => {
    const seen = new Map<string, string>()
    for (const row of breakdown.data ?? []) {
      if (!seen.has(row.agentKey)) seen.set(row.agentKey, row.agentRaw || row.agentKey)
    }
    return [...seen].map(([value, label]) => ({ value, label }))
  }, [breakdown.data])

  const modelOptions = useMemo<Option[]>(() => {
    const seen = new Set<string>()
    for (const row of breakdown.data ?? []) seen.add(row.modelId)
    return [...seen].map((value) => ({ value, label: value }))
  }, [breakdown.data])

  const hasFilter =
    filters.hostId !== null ||
    filters.agentKey !== null ||
    filters.modelId !== null ||
    filters.isIncomplete !== null

  return (
    <fieldset
      data-testid="detail-filters"
      className="flex flex-wrap items-end gap-3 rounded-xl border border-border p-4"
    >
      <legend className="px-1 text-xs font-medium text-muted-foreground">
        {zh.detail.filters.legend}
      </legend>

      <FilterField
        id="detail-filter-host"
        testId="detail-filter-host"
        label={zh.detail.filters.host}
        value={filters.hostId ?? ''}
        options={hostOptions}
        onChange={(value) =>
          dispatch({ type: 'setFilter', patch: { hostId: value === '' ? null : value } })
        }
      />
      <FilterField
        id="detail-filter-agent"
        testId="detail-filter-agent"
        label={zh.detail.filters.agent}
        value={filters.agentKey ?? ''}
        options={agentOptions}
        onChange={(value) =>
          dispatch({ type: 'setFilter', patch: { agentKey: value === '' ? null : value } })
        }
      />
      <FilterField
        id="detail-filter-model"
        testId="detail-filter-model"
        label={zh.detail.filters.model}
        value={filters.modelId ?? ''}
        options={modelOptions}
        onChange={(value) =>
          dispatch({ type: 'setFilter', patch: { modelId: value === '' ? null : value } })
        }
      />

      <div className="flex flex-col gap-1">
        <label
          htmlFor="detail-filter-incomplete"
          className="text-xs font-medium text-muted-foreground"
        >
          {zh.detail.filters.incomplete}
        </label>
        <select
          id="detail-filter-incomplete"
          data-testid="detail-filter-incomplete"
          className={SELECT_CLASS}
          value={incompleteToSelectValue(filters.isIncomplete)}
          onChange={(event) => {
            const raw = event.target.value
            dispatch({
              type: 'setFilter',
              patch: { isIncomplete: raw === '' ? null : raw === 'true' },
            })
          }}
        >
          <option value="">{zh.detail.filters.any}</option>
          <option value="true">{zh.detail.filters.incompleteOnly}</option>
          <option value="false">{zh.detail.filters.completeOnly}</option>
        </select>
      </div>

      <Button
        type="button"
        size="sm"
        variant="outline"
        data-testid="detail-filter-reset"
        className={cn(!hasFilter && 'invisible')}
        onClick={() => dispatch({ type: 'resetFilters' })}
      >
        {zh.detail.filters.reset}
      </Button>
    </fieldset>
  )
}
