/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**` and the `zh.settings` dictionary
 * section. No other worker edits this directory; this worker edits no shell file.
 *
 * Shared infrastructure to build on (do not reimplement):
 * - `@/lib/ipc` — typed `invoke` wrappers + `toIpcError`
 * - `@/app/reportRange` — `useReportRange()` for the shared range / timezone / granularity
 * - `@/components/app-state` — `LoadingState` / `EmptyState` / `ErrorState`
 * - `@/i18n/zh` — every user-visible string (`scripts/check-i18n.mjs` enforces this)
 */
import { ErrorState, LoadingState } from '@/components/app-state'
import { zh } from '@/i18n/zh'

import { AppearanceCard } from './AppearanceCard'
import { ArchiveLocationCard } from './ArchiveLocationCard'
import { PriceOverrideEditor } from './PriceOverrideEditor'
import { ReportSettingsCard } from './ReportSettingsCard'
import { usePriceOverrides } from './usePriceOverrides'
import { useSettingsForm } from './useSettingsForm'

export function SettingsView() {
  const form = useSettingsForm()
  const overrides = usePriceOverrides()

  return (
    <section data-testid="view-settings" className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 className="text-2xl font-semibold tracking-tight">{zh.settings.title}</h2>
        <p className="text-sm text-muted-foreground">{zh.settings.subtitle}</p>
      </div>

      <AppearanceCard />

      {form.error !== null ? (
        <ErrorState error={form.error} onRetry={form.refetch} />
      ) : form.isPending ? (
        <LoadingState />
      ) : (
        <ReportSettingsCard form={form} />
      )}

      <PriceOverrideEditor overrides={overrides} />
      <ArchiveLocationCard path={form.archivePath} />
    </section>
  )
}
