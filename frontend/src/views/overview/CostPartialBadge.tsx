/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * The `.cost-badge-partial` class name is part of the plan's acceptance criteria and is
 * asserted verbatim by `e2e/overview.spec.ts`. Do not rename it.
 */
import { AlertTriangle } from 'lucide-react'

import { zh } from '@/i18n/zh'
import { cn } from '@/lib/utils'

export function CostPartialBadge({ className }: { className?: string }) {
  return (
    <span
      data-testid="cost-badge-partial"
      className={cn(
        'cost-badge-partial inline-flex items-center gap-1 rounded-full bg-destructive/10 px-2 py-0.5 text-[0.7rem] font-medium text-destructive ring-1 ring-destructive/20',
        className,
      )}
    >
      <AlertTriangle aria-hidden className="size-3" />
      {zh.common.cost.partial}
    </span>
  )
}
