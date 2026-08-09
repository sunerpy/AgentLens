/**
 * Log tail and diagnostics queries.
 *
 * Both are `staleTime: 0` and not polled. Logs are read on demand: a background poll would
 * re-read the file every few seconds forever for a view the user is usually not looking at,
 * and it would fight with their scroll position while they read a stack of records.
 */
import { useQuery } from '@tanstack/react-query'

import { diagnosticsReport, logsTail } from '@/lib/ipc'

export const LOGS_QUERY_KEY = ['diagnostics', 'logs'] as const
export const DIAGNOSTICS_QUERY_KEY = ['diagnostics', 'report'] as const

/** Records fetched per read. Well under the Rust-side clamp of 2000. */
export const LOG_TAIL_LIMIT = 500

export function useLogTail() {
  return useQuery({
    queryKey: LOGS_QUERY_KEY,
    queryFn: () => logsTail(LOG_TAIL_LIMIT),
    staleTime: 0,
  })
}

export function useDiagnosticsReport() {
  return useQuery({
    queryKey: DIAGNOSTICS_QUERY_KEY,
    queryFn: () => diagnosticsReport(),
    staleTime: Infinity,
  })
}
