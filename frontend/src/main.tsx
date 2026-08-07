import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'

import './index.css'
import App from './App.tsx'
import { applyCachedTheme } from '@/app/theme/applyTheme'
import { subscribeArchiveCommits } from '@/lib/archiveQueries'
import { installContextMenuGuard } from '@/lib/contextMenuGuard'
import { isIpcError } from '@/lib/ipc'
import { subscribeRefreshCompletions } from '@/lib/refreshQueries'

/**
 * Desktop-tuned TanStack Query defaults.
 *
 * - `refetchOnWindowFocus: false` — a desktop window regains focus constantly; refetching
 *   the whole dashboard on every alt-tab would hammer SQLite while a refresh round writes.
 * - `staleTime: 30s` — archive data only changes when a scheduled refresh round commits, so
 *   sub-30s refetches are pure waste; tab switches reuse the cache. This is only safe because
 *   a committing round now invalidates the archive query family (see `@/lib/archiveQueries`);
 *   the stale window is never used as a substitute for that invalidation.
 * - `retry` returns `false` for a structured `IpcError` (an invalid range or timezone will
 *   fail identically three times, and the user needs the error now) and retries once for
 *   anything unstructured, which is where a transient `SQLITE_BUSY` would surface.
 */
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 30_000,
      retry: (failureCount, error) => !isIpcError(error) && failureCount < 1,
    },
    mutations: {
      retry: false,
    },
  },
})

async function bootstrap() {
  // Installed before the first paint so Chromium's page menu ("Reload" discards the whole
  // React tree mid-refresh) is never reachable. Editable fields keep their native menu —
  // `@/lib/contextMenuGuard` explains why that exemption is mandatory.
  installContextMenuGuard()

  // Before the first paint, so a dark theme does not flash the light palette while
  // `get_settings` is in flight. `app_settings` stays authoritative — ThemeProvider
  // overwrites both the DOM and this cache as soon as the real value arrives.
  applyCachedTheme()

  // Dev-only: `?mockIpc=1` swaps in the mock IPC layer before the first render so
  // Playwright specs run against vite dev with no Tauri process. Statically dead in
  // production builds, so `mockIpc.ts` never reaches the shipped bundle.
  if (import.meta.env.DEV && new URLSearchParams(window.location.search).has('mockIpc')) {
    const { installMockIpc } = await import('@/lib/mockIpc')
    installMockIpc()
  }

  // App-level and never torn down: the automatic scheduler tick commits while no view is
  // watching, so a per-view subscription would miss it — which is what DEFECT-2 was.
  void subscribeArchiveCommits(queryClient)
  void subscribeRefreshCompletions(queryClient)

  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </StrictMode>,
  )
}

void bootstrap()
