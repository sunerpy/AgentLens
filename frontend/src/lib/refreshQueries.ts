import type { QueryClient } from '@tanstack/react-query'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import { REFRESH_STATUS_QUERY_KEY } from './queryKeys'

export const REFRESH_COMPLETED_EVENT = 'agentlens://refresh-completed'

const pendingInvalidations = new WeakMap<QueryClient, Promise<void>>()

export function invalidateRefreshStatusQueries(client: QueryClient): Promise<void> {
  const pending = pendingInvalidations.get(client)
  if (pending !== undefined) return pending

  const invalidation = client
    .invalidateQueries({ queryKey: REFRESH_STATUS_QUERY_KEY })
    .finally(() => pendingInvalidations.delete(client))
  pendingInvalidations.set(client, invalidation)
  return invalidation
}

export async function subscribeRefreshCompletions(client: QueryClient): Promise<UnlistenFn | null> {
  try {
    return await listen(REFRESH_COMPLETED_EVENT, () => {
      void invalidateRefreshStatusQueries(client)
    })
  } catch {
    return null
  }
}
