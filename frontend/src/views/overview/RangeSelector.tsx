/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Range + granularity controls. All state lives in the shared `useReportRange()` store
 * (`@/app/reportRange`), never in local component state, so todos 16/17 observe the same
 * window. The calendar picks an INCLUSIVE end date for humans and dispatches the half-open
 * `endDateExclusive = end + 1 day` the backend contract requires.
 */
import { useEffect, useState } from 'react'
import { CalendarDays, ChevronLeft, ChevronRight } from 'lucide-react'
import { Popover } from 'radix-ui'

import { useReportRange } from '@/app/reportRange'
import { Button, buttonVariants } from '@/components/ui/button'
import type { Granularity } from '@/generated'
import { zh } from '@/i18n/zh'
import { RANGE_PRESETS, shiftIsoDate, type RangePreset } from '@/lib/localDate'
import { cn } from '@/lib/utils'
import { buildMonthGrid, isWithinInclusive, monthOf, shiftMonth } from '@/views/overview/monthGrid'

const PRESET_LABEL: Record<RangePreset, string> = {
  today: zh.common.range.today,
  last7Days: zh.common.range.last7Days,
  last30Days: zh.common.range.last30Days,
  custom: zh.common.range.custom,
}

const GRANULARITIES: Granularity[] = ['hour', 'day', 'week', 'month']

const GRANULARITY_LABEL: Record<Granularity, string> = {
  hour: zh.common.granularity.hour,
  day: zh.common.granularity.day,
  week: zh.common.granularity.week,
  month: zh.common.granularity.month,
}

function CalendarPanel({
  initialStart,
  initialEndInclusive,
  onApply,
}: {
  initialStart: string
  initialEndInclusive: string
  onApply: (startDate: string, endInclusive: string) => void
}) {
  const [month, setMonth] = useState(() => monthOf(initialEndInclusive))
  const [start, setStart] = useState<string | null>(initialStart)
  const [endInclusive, setEndInclusive] = useState<string | null>(initialEndInclusive)

  const grid = buildMonthGrid(month)
  const selected = start !== null && endInclusive !== null ? { start, endInclusive } : null

  function pick(day: string) {
    if (start === null || endInclusive !== null) {
      setStart(day)
      setEndInclusive(null)
      return
    }
    if (day < start) {
      setEndInclusive(start)
      setStart(day)
      return
    }
    setEndInclusive(day)
  }

  return (
    <div className="flex w-72 flex-col gap-3">
      <div className="flex items-center justify-between">
        <Button
          data-testid="calendar-prev-month"
          size="icon-sm"
          variant="ghost"
          aria-label={zh.overview.range.prevMonth}
          onClick={() => setMonth(shiftMonth(month, -1))}
        >
          <ChevronLeft aria-hidden />
        </Button>
        <span
          data-testid="calendar-month"
          className="font-heading text-sm font-medium tabular-nums"
        >
          {month}
        </span>
        <Button
          data-testid="calendar-next-month"
          size="icon-sm"
          variant="ghost"
          aria-label={zh.overview.range.nextMonth}
          onClick={() => setMonth(shiftMonth(month, 1))}
        >
          <ChevronRight aria-hidden />
        </Button>
      </div>

      <div className="grid grid-cols-7 gap-1 text-center text-[0.7rem] text-muted-foreground">
        {zh.overview.range.weekdays.map((weekday) => (
          <span key={weekday}>{weekday}</span>
        ))}
      </div>

      <div className="grid grid-cols-7 gap-1">
        {grid.days.map((day, index) =>
          day === null ? (
            <span key={`blank-${index}`} />
          ) : (
            <button
              key={day}
              type="button"
              data-testid={`calendar-day-${day}`}
              aria-pressed={
                selected !== null && isWithinInclusive(day, selected.start, selected.endInclusive)
              }
              onClick={() => pick(day)}
              className={cn(
                'h-8 rounded-md text-xs tabular-nums transition-colors',
                selected !== null && isWithinInclusive(day, selected.start, selected.endInclusive)
                  ? 'bg-primary text-primary-foreground'
                  : day === start
                    ? 'bg-primary/70 text-primary-foreground'
                    : 'hover:bg-muted',
              )}
            >
              {day.slice(8)}
            </button>
          ),
        )}
      </div>

      <p data-testid="calendar-hint" className="text-xs text-muted-foreground">
        {selected === null ? zh.overview.range.pickEndHint : zh.overview.range.halfOpenHint}
      </p>

      <div className="flex items-center justify-between gap-2">
        <Button
          size="sm"
          variant="ghost"
          data-testid="calendar-clear"
          onClick={() => {
            setStart(null)
            setEndInclusive(null)
          }}
        >
          {zh.overview.range.clear}
        </Button>
        <Button
          size="sm"
          data-testid="calendar-apply"
          disabled={selected === null}
          onClick={() => {
            if (selected !== null) onApply(selected.start, selected.endInclusive)
          }}
        >
          {zh.overview.range.apply}
        </Button>
      </div>
    </div>
  )
}

export function RangeSelector() {
  const { preset, range, timezone, granularity, granularityPinned, dispatch } = useReportRange()
  const [open, setOpen] = useState(false)
  const endInclusive = shiftIsoDate(range.endDateExclusive, -1)

  useEffect(() => {
    if (preset !== 'custom') setOpen(false)
  }, [preset])

  return (
    <div
      data-testid="range-selector"
      className="flex flex-wrap items-end justify-between gap-4 rounded-xl bg-card p-4 ring-1 ring-foreground/10"
    >
      <div className="flex flex-col gap-2">
        <span className="text-xs font-medium tracking-wide text-muted-foreground">
          {zh.overview.range.label}
        </span>
        <div className="flex flex-wrap items-center gap-2">
          <div
            role="group"
            aria-label={zh.overview.range.label}
            className="inline-flex rounded-lg bg-muted p-0.5"
          >
            {RANGE_PRESETS.filter((candidate) => candidate !== 'custom').map((candidate) => (
              <Button
                key={candidate}
                data-testid={`range-preset-${candidate}`}
                size="sm"
                variant={candidate === preset ? 'default' : 'ghost'}
                aria-pressed={candidate === preset}
                onClick={() =>
                  dispatch({
                    type: 'selectPreset',
                    preset: candidate as Exclude<RangePreset, 'custom'>,
                  })
                }
              >
                {PRESET_LABEL[candidate]}
              </Button>
            ))}
          </div>

          <Popover.Root open={open} onOpenChange={setOpen}>
            {/* Not `asChild` + <Button>: the shadcn Button is a plain function component with
                no forwarded ref, so Radix Popper would never receive an anchor and the
                content would stay parked at `translate(0, -200%)`. */}
            <Popover.Trigger
              data-testid="range-preset-custom"
              data-slot="button"
              aria-pressed={preset === 'custom'}
              className={buttonVariants({
                size: 'sm',
                variant: preset === 'custom' ? 'default' : 'outline',
              })}
            >
              <CalendarDays aria-hidden />
              {PRESET_LABEL.custom}
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                data-testid="range-calendar"
                sideOffset={8}
                align="start"
                className="z-50 rounded-xl border border-border bg-popover p-4 shadow-xl"
              >
                <CalendarPanel
                  initialStart={range.startDate}
                  initialEndInclusive={endInclusive}
                  onApply={(startDate, pickedEnd) => {
                    dispatch({
                      type: 'selectCustomRange',
                      startDate,
                      endDateExclusive: shiftIsoDate(pickedEnd, 1),
                    })
                    setOpen(false)
                  }}
                />
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>
        </div>

        <p className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <span data-testid="range-window" className="font-medium tabular-nums text-foreground">
            [{range.startDate}, {range.endDateExclusive})
          </span>
          <span>{zh.overview.range.halfOpenHint}</span>
          <span data-testid="range-timezone">
            {zh.common.range.timezone}: {timezone}
          </span>
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <span className="text-xs font-medium tracking-wide text-muted-foreground">
          {zh.overview.granularity.label}
        </span>
        <div
          role="group"
          aria-label={zh.overview.granularity.label}
          className="inline-flex rounded-lg bg-muted p-0.5"
        >
          <Button
            data-testid="granularity-auto"
            size="sm"
            variant={granularityPinned ? 'ghost' : 'default'}
            aria-pressed={!granularityPinned}
            onClick={() => dispatch({ type: 'resetGranularity' })}
          >
            {zh.overview.granularity.auto}
          </Button>
          {GRANULARITIES.map((candidate) => (
            <Button
              key={candidate}
              data-testid={`granularity-${candidate}`}
              size="sm"
              variant={granularityPinned && candidate === granularity ? 'default' : 'ghost'}
              aria-pressed={granularityPinned && candidate === granularity}
              onClick={() => dispatch({ type: 'setGranularity', granularity: candidate })}
            >
              {GRANULARITY_LABEL[candidate]}
            </Button>
          ))}
        </div>
        <p data-testid="granularity-hint" className="text-xs text-muted-foreground">
          <span data-testid="granularity-effective" className="font-medium text-foreground">
            {GRANULARITY_LABEL[granularity]}
          </span>
          {' · '}
          {granularityPinned
            ? zh.overview.granularity.pinnedHint
            : zh.overview.granularity.autoHint}
        </p>
      </div>
    </div>
  )
}
