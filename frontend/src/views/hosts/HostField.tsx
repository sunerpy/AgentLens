/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Field chrome for the hosts forms. Class strings are copied from
 * `views/settings/SettingsField` (which in turn mirrors `views/detail`'s filter bar) so all
 * five views read as one product rather than five separately styled pages.
 */
import type { ReactNode } from 'react'

export const CONTROL_CLASS =
  'h-8 rounded-lg border border-border bg-background px-2 text-sm text-foreground outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-50'

export function HostField({
  id,
  label,
  hint,
  children,
}: {
  id: string
  label: string
  hint?: ReactNode
  children: ReactNode
}) {
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className="text-xs font-medium text-muted-foreground">
        {label}
      </label>
      {children}
      {hint === undefined ? null : <span className="text-xs text-muted-foreground">{hint}</span>}
    </div>
  )
}
