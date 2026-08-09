/**
 * Shell navigation: a left rail with three states, one item per view.
 *
 * Owner: W8 prep (shell/infrastructure). Labels come from the `zh.nav` block, which the
 * shell owns; view workers add strings under their own `zh.<view>` section instead.
 *
 * Replaces the former top tab strip (`AppNav`). The tab ARIA role is kept rather than swapped
 * for `navigation` + `aria-current`: these really are sibling panels in one window with no URL
 * behind them, which is what `tablist` describes. Two additions on top of the stock pattern:
 * every item stays in the Tab order (a rail is chrome the user tabs *into*, and a roving
 * tabindex would make five of the six items unreachable that way), and Arrow/Home/End move
 * between items for anyone who expects the canonical tabs keys.
 *
 * Surface tokens are `--card` / `--border` / `--muted`, NOT the shadcn `--sidebar-*` set.
 * Those eight preset tokens were mapped in `@theme` but only ever declared under `:root` and
 * `.dark`, so the four custom palettes silently inherited the light values — a `bg-sidebar`
 * rail would have painted white inside 深海蓝 and 夜紫. The never-rendered preset branch is
 * deleted in `index.css`; see the note there.
 */
import { useCallback, useEffect, useRef, type KeyboardEvent, type PointerEvent } from 'react'
import {
  ChartColumnBig,
  EyeOff,
  LayoutDashboard,
  PanelLeftClose,
  PanelLeftOpen,
  Pin,
  PinOff,
  ScrollText,
  Server,
  Settings,
  TableProperties,
  type LucideIcon,
} from 'lucide-react'

import { useShellLayout } from '@/app/layout/ShellLayoutContext'
import {
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_RECALL_WIDTH,
  SIDEBAR_WIDTH_STEP,
  sidebarWidthPx,
} from '@/app/layout/shellLayout'
import { VIEW_KEYS, type ViewKey } from '@/app/views'
import { zh } from '@/i18n/zh'
import { cn } from '@/lib/utils'

/**
 * One icon per view, so the collapsed rail stays distinguishable. Icons are the *only* label
 * in that state, which is why each item also carries `aria-label` and `title`.
 */
const VIEW_ICONS: Record<ViewKey, LucideIcon> = {
  overview: LayoutDashboard,
  drilldown: ChartColumnBig,
  detail: TableProperties,
  hosts: Server,
  settings: Settings,
  diagnostics: ScrollText,
}

const CONTROL_CLASS = cn(
  'inline-flex h-7 items-center justify-center gap-1.5 rounded-md px-2',
  'text-xs font-medium text-muted-foreground transition-colors outline-none',
  'hover:bg-muted hover:text-foreground',
  'focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1',
  'focus-visible:ring-offset-card',
  '[&_svg]:pointer-events-none [&_svg]:size-3.5 [&_svg]:shrink-0',
)

export function AppSidebar({
  active,
  onSelect,
}: {
  active: ViewKey
  onSelect: (view: ViewKey) => void
}) {
  const {
    layout,
    state,
    widthPx,
    peeking,
    toggleCollapsed,
    setHidden,
    togglePinned,
    previewWidth,
    commitWidth,
    setPeeking,
  } = useShellLayout()

  const itemRefs = useRef<(HTMLButtonElement | null)[]>([])
  const dragStart = useRef<{ x: number; width: number } | null>(null)

  /*
    Read off `layout`, NOT off `state`. `state` collapses the two flags into one answer and reports
    `'hidden'` for a hidden-and-collapsed rail, so deriving `collapsed` from it made the hover
    preview render EXPANDED content — full labels and a horizontal footer — inside a 64px box.
    The labels were clipped away silently, but the footer's flex row wrapped 收起侧栏 into a
    vertical stack of single characters.
  */
  const collapsed = layout.collapsed
  const hidden = layout.hidden
  /** Hidden-but-peeking behaves like an unpinned rail: it floats instead of taking px. */
  const floating = hidden ? peeking : !layout.pinned
  /**
   * A hidden rail measures 0, which is correct for the layout but wrong for the hover preview:
   * rendering the preview at that width produced a 1px sliver (only the border had any width),
   * so the preview is measured as if it were not hidden.
   */
  const shownWidth = hidden ? sidebarWidthPx({ ...layout, hidden: false }) : widthPx

  /**
   * Retracting the preview cannot ride `onMouseLeave` on the rail. The rail appears *underneath*
   * a stationary cursor, so the browser never dispatched a matching `mouseenter`, and the first
   * move away therefore produced no `mouseleave` either — the preview stayed open indefinitely.
   * Watching the pointer's x against the preview's own edge is independent of that bookkeeping.
   */
  useEffect(() => {
    if (!hidden || !peeking) return undefined
    const onMove = (event: globalThis.PointerEvent) => {
      if (event.clientX > shownWidth) setPeeking(false)
    }
    document.addEventListener('pointermove', onMove)
    return () => document.removeEventListener('pointermove', onMove)
  }, [hidden, peeking, setPeeking, shownWidth])

  const moveFocus = useCallback(
    (index: number) => {
      const wrapped = (index + VIEW_KEYS.length) % VIEW_KEYS.length
      const target = itemRefs.current[wrapped]
      target?.focus()
      onSelect(VIEW_KEYS[wrapped])
    },
    [onSelect],
  )

  const onNavKeyDown = useCallback(
    (event: KeyboardEvent<HTMLElement>, index: number) => {
      // Enter / Space are left to the native button activation; only the tabs-pattern
      // navigation keys are intercepted, and only when they would otherwise scroll.
      switch (event.key) {
        case 'ArrowDown':
          event.preventDefault()
          moveFocus(index + 1)
          break
        case 'ArrowUp':
          event.preventDefault()
          moveFocus(index - 1)
          break
        case 'Home':
          event.preventDefault()
          moveFocus(0)
          break
        case 'End':
          event.preventDefault()
          moveFocus(VIEW_KEYS.length - 1)
          break
        default:
          break
      }
    },
    [moveFocus],
  )

  const onResizePointerDown = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      // Pointer capture, not a window listener: it keeps the stream flowing when the pointer
      // outruns the 200..320 clamp and leaves the handle, which a plain mousemove would drop.
      event.currentTarget.setPointerCapture(event.pointerId)
      dragStart.current = { x: event.clientX, width: layout.width }
    },
    [layout.width],
  )

  const onResizePointerMove = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      const start = dragStart.current
      if (start === null) return
      previewWidth(start.width + (event.clientX - start.x))
    },
    [previewWidth],
  )

  const onResizePointerUp = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      if (dragStart.current === null) return
      dragStart.current = null
      event.currentTarget.releasePointerCapture(event.pointerId)
      // One write per drag, on release — a write per pointer-move would be ~60 IPC calls/s.
      commitWidth(layout.width)
    },
    [commitWidth, layout.width],
  )

  const onResizeKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      let delta = 0
      if (event.key === 'ArrowRight') delta = SIDEBAR_WIDTH_STEP
      if (event.key === 'ArrowLeft') delta = -SIDEBAR_WIDTH_STEP
      if (delta === 0) return
      event.preventDefault()
      const next = layout.width + delta
      previewWidth(next)
      commitWidth(next)
    },
    [commitWidth, layout.width, previewWidth],
  )

  return (
    <>
      {hidden ? (
        <button
          type="button"
          data-testid="sidebar-recall"
          aria-label={zh.sidebar.show}
          title={zh.sidebar.show}
          onMouseEnter={() => setPeeking(true)}
          onFocus={() => setPeeking(true)}
          onClick={() => setHidden(false)}
          style={{ width: `${String(SIDEBAR_RECALL_WIDTH)}px` }}
          className={cn(
            'group/recall fixed start-0 top-titlebar bottom-0 z-40 flex cursor-e-resize',
            'items-stretch bg-transparent transition-colors hover:bg-primary/20',
            'focus-visible:bg-primary/20 focus-visible:outline-2 focus-visible:outline-ring',
          )}
        >
          {/*
            A visible hairline inside the 12px hit area. work-kit's strip is fully transparent,
            which makes the only way back from the hidden state something the user has to find by
            accident. The hairline is the affordance; the transparent margin around it is the
            target. Two separate widths on purpose — a 2px target would be unhittable.
          */}
          <span
            aria-hidden
            data-testid="sidebar-recall-hint"
            className={cn(
              'block h-full w-0.5 rounded-e-full bg-border transition-colors',
              'group-hover/recall:bg-primary group-focus-visible/recall:bg-primary',
            )}
          />
        </button>
      ) : null}

      {hidden && !peeking ? null : (
        <aside
          data-testid="app-sidebar"
          data-state={state}
          data-pinned={layout.pinned}
          data-floating={floating}
          /*
            `minWidth` / `maxWidth` are not redundant with `width`. A flex item defaults to
            `min-width: auto`, i.e. it refuses to go below its own min-content width, so the
            footer control row alone held the collapsed rail at 173px and `width: 64px` was
            quietly ignored. Pinning all three makes the rendered width exactly the model's.
          */
          style={{
            width: `${String(shownWidth)}px`,
            minWidth: `${String(shownWidth)}px`,
            maxWidth: `${String(shownWidth)}px`,
          }}
          className={cn(
            'z-40 flex shrink-0 flex-col overflow-hidden border-e border-border bg-card',
            'transition-[width] duration-200 ease-out',
            floating
              ? 'fixed start-0 top-titlebar bottom-0 shadow-raised'
              : 'sticky top-titlebar h-[calc(100dvh-var(--titlebar-height))]',
          )}
        >
          <nav
            role="tablist"
            aria-orientation="vertical"
            aria-label={zh.sidebar.label}
            className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto p-2"
          >
            {VIEW_KEYS.map((view, index) => {
              const Icon = VIEW_ICONS[view]
              const isActive = view === active
              return (
                <button
                  key={view}
                  ref={(node) => {
                    itemRefs.current[index] = node
                  }}
                  type="button"
                  role="tab"
                  aria-selected={isActive}
                  aria-label={zh.nav[view]}
                  title={zh.nav[view]}
                  data-testid={`nav-${view}`}
                  onClick={() => onSelect(view)}
                  onKeyDown={(event) => onNavKeyDown(event, index)}
                  className={cn(
                    'relative flex h-10 shrink-0 items-center rounded-lg text-sm font-medium',
                    'transition-colors outline-none',
                    'focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1',
                    'focus-visible:ring-offset-card',
                    '[&_svg]:pointer-events-none [&_svg]:size-4.5 [&_svg]:shrink-0',
                    collapsed ? 'justify-center px-0' : 'justify-start gap-2.5 px-3',
                    isActive
                      ? 'bg-primary text-primary-foreground shadow-panel'
                      : 'text-muted-foreground hover:bg-muted hover:text-foreground',
                  )}
                >
                  {/*
                    Non-textual selection marker. The collapsed rail has no label to highlight,
                    so the filled background alone would be the only signal; this keeps a second
                    one that survives a theme whose primary fill is low-contrast.
                  */}
                  {isActive ? (
                    <span
                      aria-hidden
                      data-testid={`nav-marker-${view}`}
                      className="absolute inset-y-1.5 start-0.5 w-0.5 rounded-full bg-primary-foreground/70"
                    />
                  ) : null}
                  <Icon aria-hidden />
                  {collapsed ? null : <span className="truncate">{zh.nav[view]}</span>}
                </button>
              )
            })}
          </nav>

          <div
            className={cn(
              'flex shrink-0 items-center gap-1 border-t border-border p-2',
              collapsed ? 'flex-col' : 'justify-between',
            )}
          >
            <button
              type="button"
              data-testid="sidebar-toggle-collapsed"
              aria-label={collapsed ? zh.sidebar.expand : zh.sidebar.collapse}
              aria-expanded={!collapsed}
              title={collapsed ? zh.sidebar.expand : zh.sidebar.collapse}
              onClick={toggleCollapsed}
              className={CONTROL_CLASS}
            >
              {collapsed ? <PanelLeftOpen aria-hidden /> : <PanelLeftClose aria-hidden />}
              {collapsed ? null : <span>{zh.sidebar.collapse}</span>}
            </button>
            <div className={cn('flex items-center gap-1', collapsed && 'flex-col')}>
              <button
                type="button"
                data-testid="sidebar-toggle-pinned"
                aria-label={layout.pinned ? zh.sidebar.unpin : zh.sidebar.pin}
                aria-pressed={layout.pinned}
                title={layout.pinned ? zh.sidebar.unpin : zh.sidebar.pin}
                onClick={togglePinned}
                className={CONTROL_CLASS}
              >
                {layout.pinned ? <Pin aria-hidden /> : <PinOff aria-hidden />}
              </button>
              {/*
                A toggle, not a one-way hide. While the hover preview is open it is the only way
                back: the preview renders at full width over the 12px edge strip, so a mouse
                click can never reach the strip again once hovering it has opened the preview.
              */}
              <button
                type="button"
                data-testid="sidebar-toggle-hidden"
                aria-label={hidden ? zh.sidebar.show : zh.sidebar.hide}
                aria-pressed={hidden}
                title={hidden ? zh.sidebar.show : zh.sidebar.hide}
                onClick={() => setHidden(!hidden)}
                className={CONTROL_CLASS}
              >
                {hidden ? <PanelLeftOpen aria-hidden /> : <EyeOff aria-hidden />}
              </button>
            </div>
          </div>

          {collapsed ? null : (
            <div
              data-testid="sidebar-resize"
              role="separator"
              aria-orientation="vertical"
              aria-label={zh.sidebar.resize}
              aria-valuenow={layout.width}
              aria-valuemin={SIDEBAR_MIN_WIDTH}
              aria-valuemax={SIDEBAR_MAX_WIDTH}
              tabIndex={0}
              onPointerDown={onResizePointerDown}
              onPointerMove={onResizePointerMove}
              onPointerUp={onResizePointerUp}
              onKeyDown={onResizeKeyDown}
              className={cn(
                'absolute inset-y-0 end-0 w-1.5 cursor-col-resize touch-none',
                'transition-colors hover:bg-primary/30',
                'focus-visible:bg-primary/30 focus-visible:outline-2 focus-visible:outline-ring',
              )}
            />
          )}
        </aside>
      )}
    </>
  )
}
