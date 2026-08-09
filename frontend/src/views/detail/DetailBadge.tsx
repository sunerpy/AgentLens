/**
 * EXCLUSIVE FILE BOUNDARY — todo 17 owns `src/views/detail/**`.
 *
 * Local badge primitive. No shadcn `badge` is installed and adding an npm dependency is not
 * allowed while sibling workers hold `package.json`, so this is built from the same design
 * tokens the shadcn primitives use (`muted`, `primary`, `destructive`, `border`).
 */
import type { ReactNode } from 'react'

import { cn } from '@/lib/utils'

export type BadgeTone = 'neutral' | 'muted' | 'accent' | 'warning'

const TONE_CLASS: Record<BadgeTone, string> = {
  neutral: 'border-border bg-background text-foreground',
  muted: 'border-border bg-muted text-muted-foreground',
  accent: 'border-primary/30 bg-primary/10 text-foreground',
  warning: 'border-destructive/40 bg-destructive/10 text-destructive',
}

/**
 * `clip` needs both halves of the flexbox truncation pattern to work, which is why it is a prop
 * rather than a caller-supplied class: `overflow-hidden` on the outer span is what lets the badge
 * shrink at all (a flex item's `min-width: auto` resolves to `min-content` while its overflow is
 * visible), and `min-w-0 truncate` on an inner element is what produces the ellipsis, because a
 * bare text child of a flex container is an anonymous item that cannot carry `text-overflow`.
 */
export function DetailBadge({
  tone = 'neutral',
  title,
  className,
  clip = false,
  children,
  ...rest
}: {
  tone?: BadgeTone
  title?: string
  className?: string
  clip?: boolean
  children: ReactNode
} & Record<`data-${string}`, string | undefined>) {
  return (
    <span
      title={title}
      className={cn(
        'inline-flex items-center whitespace-nowrap rounded-md border px-1.5 py-0.5 text-[0.6875rem] leading-4 font-medium',
        clip && 'max-w-full overflow-hidden',
        TONE_CLASS[tone],
        className,
      )}
      {...rest}
    >
      {clip ? <span className="min-w-0 truncate">{children}</span> : children}
    </span>
  )
}
