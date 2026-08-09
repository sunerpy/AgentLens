import { useState } from 'react'
import { Palette } from 'lucide-react'
import { Popover } from 'radix-ui'

import { buttonVariants } from '@/components/ui/button'
import { zh } from '@/i18n/zh'

import { ThemeOptionGrid } from './ThemeOptionGrid'
import { useTheme } from './ThemeContext'

export function ThemeMenu() {
  const { theme } = useTheme()
  const [open, setOpen] = useState(false)

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      {/* Not `asChild` + <Button>: the shadcn Button forwards no ref, so Radix Popper would
          never get an anchor — the same constraint RangeSelector documents. */}
      <Popover.Trigger
        data-testid="theme-menu-trigger"
        data-slot="button"
        aria-label={zh.theme.label}
        className={buttonVariants({ size: 'sm', variant: 'ghost' })}
      >
        <Palette aria-hidden />
        <span data-testid="theme-menu-current">{zh.theme.names[theme]}</span>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          data-testid="theme-menu"
          sideOffset={8}
          align="end"
          className="z-50 w-60 rounded-xl border border-border bg-popover p-2 shadow-raised"
        >
          <p className="px-2.5 pt-1 pb-2 text-xs font-medium tracking-wide text-muted-foreground">
            {zh.theme.label}
          </p>
          <ThemeOptionGrid onPicked={() => setOpen(false)} />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  )
}
