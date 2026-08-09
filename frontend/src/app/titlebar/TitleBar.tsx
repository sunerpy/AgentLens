import { Minus, Copy, Square, X } from 'lucide-react'

import { zh } from '@/i18n/zh'
import { cn } from '@/lib/utils'

import { useWindowChrome } from './useWindowChrome'

const CONTROL_BASE = cn(
  'inline-flex h-full w-11 items-center justify-center',
  'text-titlebar-foreground transition-colors outline-none',
  'hover:bg-titlebar-hover focus-visible:bg-titlebar-hover',
  'focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-inset',
  '[&_svg]:pointer-events-none [&_svg]:size-3.5',
)

export function TitleBar() {
  const {
    platform,
    controls,
    isMaximized,
    minimize,
    toggleMaximize,
    close,
    dragOnNonMousePointer,
  } = useWindowChrome()

  // macOS keeps its native traffic lights (`titleBarStyle: "Overlay"` in
  // tauri.macos.conf.json), so drawing a second set would duplicate them; the bar only
  // reserves the leading inset there. Without a Tauri window handle the buttons would be
  // inert, so they are omitted rather than shown broken.
  const showControls = platform !== 'macos' && controls !== null

  return (
    <div
      data-tauri-drag-region="deep"
      data-testid="titlebar"
      data-platform={platform}
      onPointerDown={(event) => dragOnNonMousePointer(event.pointerType)}
      className={cn(
        'sticky top-0 z-50 flex h-titlebar shrink-0 items-center gap-2 select-none',
        'border-b border-titlebar-border bg-titlebar text-titlebar-foreground',
        'ps-titlebar-inset',
      )}
    >
      <div className="flex min-w-0 flex-1 items-center gap-2 px-3">
        <span aria-hidden="true" className="size-2 shrink-0 rounded-full bg-chart-2" />
        <span
          data-testid="titlebar-title"
          className="truncate text-xs font-medium tracking-wide text-titlebar-foreground"
        >
          {zh.appName}
        </span>
      </div>

      {showControls ? (
        <div data-testid="titlebar-controls" className="flex h-full items-stretch">
          <button
            type="button"
            data-testid="titlebar-minimize"
            aria-label={zh.titlebar.minimize}
            title={zh.titlebar.minimize}
            onClick={minimize}
            className={cn(CONTROL_BASE)}
          >
            <Minus />
          </button>
          <button
            type="button"
            data-testid="titlebar-maximize"
            data-state={isMaximized ? 'maximized' : 'normal'}
            aria-label={isMaximized ? zh.titlebar.restore : zh.titlebar.maximize}
            title={isMaximized ? zh.titlebar.restore : zh.titlebar.maximize}
            onClick={toggleMaximize}
            className={cn(CONTROL_BASE)}
          >
            {isMaximized ? <Copy /> : <Square />}
          </button>
          <button
            type="button"
            data-testid="titlebar-close"
            aria-label={zh.titlebar.close}
            title={zh.titlebar.close}
            onClick={close}
            className={cn(
              CONTROL_BASE,
              'hover:bg-titlebar-danger hover:text-titlebar-danger-foreground',
              'focus-visible:bg-titlebar-danger focus-visible:text-titlebar-danger-foreground',
            )}
          >
            <X />
          </button>
        </div>
      ) : null}
    </div>
  )
}
