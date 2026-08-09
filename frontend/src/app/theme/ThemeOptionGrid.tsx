import { Check } from 'lucide-react'

import { zh } from '@/i18n/zh'
import { cn } from '@/lib/utils'

import { useTheme } from './ThemeContext'
import { THEME_KEYS, THEME_SWATCH, type ThemeKey } from './themes'

function Swatch({ theme }: { theme: ThemeKey }) {
  const [background, edge, accent] = THEME_SWATCH[theme]
  return (
    <span
      aria-hidden
      className="flex size-6 shrink-0 items-center justify-center rounded-full ring-1 ring-inset"
      style={{ background, boxShadow: `inset 0 0 0 1px ${edge}` }}
    >
      <span className="size-3 rounded-full" style={{ background: accent }} />
    </span>
  )
}

/**
 * The theme options, shared verbatim by the header menu and the settings card so the two can
 * never drift into offering different palettes.
 *
 * `role="radio"` rather than a `<select>`: each row carries a colour preview, which a native
 * option element cannot render.
 */
export function ThemeOptionGrid({
  columns = 1,
  onPicked,
}: {
  columns?: 1 | 2
  onPicked?: () => void
}) {
  const { theme, setTheme } = useTheme()

  return (
    <div
      role="radiogroup"
      aria-label={zh.theme.label}
      data-testid="theme-options"
      className={cn('grid gap-1', columns === 2 ? 'sm:grid-cols-2' : 'grid-cols-1')}
    >
      {THEME_KEYS.map((candidate) => {
        const active = candidate === theme
        return (
          <button
            key={candidate}
            type="button"
            role="radio"
            aria-checked={active}
            data-testid={`theme-option-${candidate}`}
            onClick={() => {
              setTheme(candidate)
              onPicked?.()
            }}
            className={cn(
              'flex min-w-0 items-center gap-3 rounded-lg px-2.5 py-2 text-left transition-colors',
              'outline-none focus-visible:ring-2 focus-visible:ring-ring/60',
              active ? 'bg-accent text-accent-foreground' : 'hover:bg-muted',
            )}
          >
            <Swatch theme={candidate} />
            <span className="flex min-w-0 flex-col">
              <span className="truncate text-sm font-medium">{zh.theme.names[candidate]}</span>
              <span className="truncate text-[0.7rem] text-muted-foreground">
                {zh.theme.modes[candidate]}
              </span>
            </span>
            <Check
              aria-hidden
              className={cn('ml-auto size-4 shrink-0', active ? 'opacity-100' : 'opacity-0')}
            />
          </button>
        )
      })}
    </div>
  )
}
