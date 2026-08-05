/**
 * The **archive query family** — one cache identity shared by every read that answers
 * "what is in the archive right now".
 *
 * Why this file exists (F3 DEFECT-2): `get_summary` / `get_trend` / `get_breakdown` /
 * `query_messages` each owned a private, unrelated query key, so nothing could invalidate
 * them as a group. Combined with `staleTime: 30_000` in `main.tsx`, a refresh round could
 * commit 155k rows while the overview kept serving the cached pre-collection result — zeros
 * on a fresh install, stale numbers on a warm archive — and only a range change (a brand-new
 * query key) recovered it.
 *
 * The fix has two halves and both live here:
 *  1. Every aggregate query key is built with {@link archiveQueryKey}, so they all start with
 *     the same root segment and {@link invalidateArchiveQueries} reaches all of them at once.
 *     Do NOT hand-write the root literal in a view.
 *  2. {@link subscribeArchiveCommits} listens for the Tauri event the Rust refresh runtime
 *     emits when a round **commits new rows**, so the automatic scheduler tick invalidates the
 *     family too — not just the manual 立即刷新 button. Polling would not be correctness here:
 *     nothing polls while the user sits on 总览.
 */
import type { QueryClient } from '@tanstack/react-query'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

/** First segment of every archive-derived query key. */
export const ARCHIVE_QUERY_KEY_ROOT = 'archive'

/** Prefix that invalidates the whole family; TanStack matches query keys by prefix. */
export const ARCHIVE_QUERY_KEY = [ARCHIVE_QUERY_KEY_ROOT] as const

/**
 * Event emitted by `src-tauri/src/state.rs` after a refresh round commits new rows.
 *
 * The name is duplicated in Rust (`state::EVENT_ARCHIVE_COMMITTED`) because an event name is
 * a wire string, not a generated DTO; both sides carry a comment pointing at the other.
 */
export const ARCHIVE_COMMITTED_EVENT = 'agentlens://archive-committed'

/** Build an archive-family query key. `segments` identify the specific read. */
export function archiveQueryKey(...segments: readonly unknown[]): readonly unknown[] {
  return [ARCHIVE_QUERY_KEY_ROOT, ...segments]
}

/**
 * Mark every archive-derived query stale, refetching the ones currently mounted.
 *
 * Views that are not mounted are only marked stale, so returning to them refetches instead of
 * replaying the pre-refresh cache — which is exactly the "leave 总览 and come back" case that
 * used to keep showing zeros.
 */
export function invalidateArchiveQueries(client: QueryClient): Promise<void> {
  return client.invalidateQueries({ queryKey: ARCHIVE_QUERY_KEY })
}

/**
 * Subscribe to {@link ARCHIVE_COMMITTED_EVENT} for the lifetime of the process.
 *
 * Returns `null` when no Tauri event bridge is reachable — a plain `vite dev` browser tab has
 * no `__TAURI_INTERNALS__` at all — so a browser-only preview still renders instead of failing
 * to boot. Inside the desktop shell the listener always installs.
 */
export async function subscribeArchiveCommits(client: QueryClient): Promise<UnlistenFn | null> {
  try {
    return await listen(ARCHIVE_COMMITTED_EVENT, () => {
      void invalidateArchiveQueries(client)
    })
  } catch {
    return null
  }
}
