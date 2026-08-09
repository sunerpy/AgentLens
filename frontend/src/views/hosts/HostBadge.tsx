/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Local badge primitive, mirroring `views/detail/DetailBadge` so the app reads as one
 * product. Same tone contract as the other views: **red (`warning`) is reserved for
 * genuine problems** — a failed refresh or a rejected connection — never for ordinary
 * informational states.
 */
import type { ReactNode } from 'react'

import { cn } from '@/lib/utils'

export type HostBadgeTone = 'neutral' | 'muted' | 'accent' | 'warning'

const TONE_CLASS: Record<HostBadgeTone, string> = {
  neutral: 'border-border bg-background text-foreground',
  muted: 'border-border bg-muted text-muted-foreground',
  accent: 'border-primary/30 bg-primary/10 text-foreground',
  warning: 'border-destructive/40 bg-destructive/10 text-destructive',
}

export function HostBadge({
  tone = 'neutral',
  title,
  className,
  children,
  ...rest
}: {
  tone?: HostBadgeTone
  title?: string
  className?: string
  children: ReactNode
} & Record<`data-${string}`, string | undefined>) {
  return (
    <span
      title={title}
      className={cn(
        'inline-flex items-center whitespace-nowrap rounded-md border px-1.5 py-0.5 text-[0.6875rem] leading-4 font-medium',
        TONE_CLASS[tone],
        className,
      )}
      {...rest}
    >
      {children}
    </span>
  )
}
