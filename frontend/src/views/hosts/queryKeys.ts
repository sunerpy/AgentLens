/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * TanStack Query keys shared by the hosts view's components, so a write in one place
 * invalidates the reads in the others instead of leaving a stale row on screen.
 */
export const HOSTS_QUERY_KEY = ['hosts', 'list'] as const
export const REFRESH_STATUS_QUERY_KEY = ['hosts', 'refreshStatus'] as const
export const LOCAL_IDENTITY_QUERY_KEY = ['hosts', 'localIdentity'] as const
