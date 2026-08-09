/**
 * EXCLUSIVE FILE BOUNDARY — todo 17 owns `src/views/detail/**`.
 *
 * Paging + filter state for the detail view.
 *
 * Paging is **server-side and only server-side**: the real archive holds ~153,800 rows, so every
 * page turn issues a fresh `query_messages` with a new `offset`. Nothing here ever slices a
 * larger client-side result set, and `queryMessages` is never called without a `limit`.
 */
import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { useMemo, useReducer } from 'react'

import { useReportRange } from '@/app/reportRange'
import type { MessageFilters, MessagePage } from '@/generated'
import { archiveQueryKey } from '@/lib/archiveQueries'
import { queryMessages } from '@/lib/ipc'

/** One page. The backend clamps `limit` to `MAX_DETAIL_LIMIT = 200`; 50 is well inside that. */
export const DETAIL_PAGE_SIZE = 50

export interface DetailFilterState {
  hostId: string | null
  agentKey: string | null
  modelId: string | null
  isIncomplete: boolean | null
}

export const EMPTY_DETAIL_FILTERS: DetailFilterState = {
  hostId: null,
  agentKey: null,
  modelId: null,
  isIncomplete: null,
}

interface DetailPageState {
  filters: DetailFilterState
  offset: number
  /** Identity of the shared report window, so a range change can reset paging (see below). */
  rangeKey: string
}

export type DetailPageAction =
  | { type: 'setFilter'; patch: Partial<DetailFilterState> }
  | { type: 'resetFilters' }
  | { type: 'nextPage'; totalCount: number }
  | { type: 'previousPage' }
  | { type: 'rangeChanged'; rangeKey: string }

function reducer(state: DetailPageState, action: DetailPageAction): DetailPageState {
  switch (action.type) {
    // Any filter change must return to the first page: keeping the old offset would silently
    // show page 2 of a shorter result set, or an empty page for a result set that has rows.
    case 'setFilter':
      return { ...state, filters: { ...state.filters, ...action.patch }, offset: 0 }
    case 'resetFilters':
      return { ...state, filters: EMPTY_DETAIL_FILTERS, offset: 0 }
    case 'rangeChanged':
      return { ...state, rangeKey: action.rangeKey, offset: 0 }
    case 'nextPage': {
      const next = state.offset + DETAIL_PAGE_SIZE
      return next >= action.totalCount ? state : { ...state, offset: next }
    }
    case 'previousPage':
      return { ...state, offset: Math.max(0, state.offset - DETAIL_PAGE_SIZE) }
  }
}

export interface DetailPageResult {
  filters: DetailFilterState
  offset: number
  pageSize: number
  page: MessagePage | undefined
  isPending: boolean
  isFetching: boolean
  error: unknown
  refetch: () => void
  dispatch: (action: DetailPageAction) => void
  timezone: string
}

export function useDetailPage(): DetailPageResult {
  const { range, timezone } = useReportRange()
  const rangeKey = `${range.startDate}|${range.endDateExclusive}|${range.weekStart}|${timezone}`

  const [state, dispatch] = useReducer(reducer, {
    filters: EMPTY_DETAIL_FILTERS,
    offset: 0,
    rangeKey,
  })

  // Reset during render rather than in an effect: an effect would let one `query_messages` fire
  // at the stale offset for the new range before the reset landed.
  if (state.rangeKey !== rangeKey) {
    dispatch({ type: 'rangeChanged', rangeKey })
  }
  const offset = state.rangeKey === rangeKey ? state.offset : 0

  const messageFilters: MessageFilters = useMemo(
    () => ({
      range,
      timezone,
      hostId: state.filters.hostId,
      source: null,
      agentKey: state.filters.agentKey,
      providerId: null,
      modelId: state.filters.modelId,
      isIncomplete: state.filters.isIncomplete,
    }),
    [range, timezone, state.filters],
  )

  const query = useQuery({
    queryKey: archiveQueryKey('detail', 'messages', rangeKey, state.filters, offset),
    queryFn: () => queryMessages(messageFilters, DETAIL_PAGE_SIZE, offset),
    placeholderData: keepPreviousData,
  })

  return {
    filters: state.filters,
    offset,
    pageSize: DETAIL_PAGE_SIZE,
    page: query.data,
    isPending: query.isPending,
    isFetching: query.isFetching,
    error: query.error,
    refetch: () => {
      void query.refetch()
    },
    dispatch,
    timezone,
  }
}
