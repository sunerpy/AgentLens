/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Range + granularity controls. All state lives in the shared `useReportRange()` store
 * (`@/app/reportRange`), never in local component state, so todos 16/17 observe the same
 * window. The calendar picks an INCLUSIVE end date for humans and dispatches the half-open
 * `endDateExclusive = end + 1 day` the backend contract requires.
 */
import { useEffect, useState } from 'react'
import { CalendarDays } from 'lucide-react'
import { Popover } from 'radix-ui'

import { useReportRange } from '@/app/reportRange'
import { Button, buttonVariants } from '@/components/ui/button'
import type { Granularity } from '@/generated'
import { zh } from '@/i18n/zh'
import { RANGE_PRESETS, shiftIsoDate, type RangePreset } from '@/lib/localDate'

const PRESET_LABEL: Record<RangePreset, string> = {
  today: zh.common.range.today,
  last7Days: zh.common.range.last7Days,
  last30Days: zh.common.range.last30Days,
  thisQuarter: zh.common.range.thisQuarter,
  thisYear: zh.common.range.thisYear,
  custom: zh.common.range.custom,
}

const GRANULARITIES: Granularity[] = ['hour', 'day', 'week', 'month']

const GRANULARITY_LABEL: Record<Granularity, string> = {
  hour: zh.common.granularity.hour,
  day: zh.common.granularity.day,
  week: zh.common.granularity.week,
  month: zh.common.granularity.month,
}

const DATE_INPUT_CLASS =
  'h-9 rounded-md border border-input bg-background px-2 text-sm tabular-nums select-text ' +
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'

/**
 * Two independent `<input type="date">` fields — one start, one end — rather than a single
 * month grid the user has to page back and forth in to reach both endpoints.
 *
 * `type="date"` is the platform's own picker, so this adds no date library (a hard plan
 * constraint) and keeps the native context menu the editable-field exemption relies on.
 * The end field is **inclusive** because that is what a human means by "截止日期"; the
 * half-open `endDateExclusive` the backend contract requires is derived on apply.
 */
function DateRangePanel({
  initialStart,
  initialEndInclusive,
  onApply,
}: {
  initialStart: string
  initialEndInclusive: string
  onApply: (startDate: string, endInclusive: string) => void
}) {
  const [start, setStart] = useState(initialStart)
  const [endInclusive, setEndInclusive] = useState(initialEndInclusive)

  const missing = start === '' || endInclusive === ''
  const inverted = !missing && endInclusive < start
  const issue = missing
    ? zh.overview.range.requireBothDates
    : inverted
      ? zh.overview.range.invalidOrder
      : null

  return (
    <div className="flex w-72 flex-col gap-3">
      <span className="text-sm font-medium">{zh.overview.range.customTitle}</span>

      <div className="flex flex-col gap-1">
        <label htmlFor="range-start-date" className="text-xs text-muted-foreground select-none">
          {zh.overview.range.startDate}
        </label>
        <input
          id="range-start-date"
          data-testid="range-start-date"
          type="date"
          className={DATE_INPUT_CLASS}
          value={start}
          max={endInclusive === '' ? undefined : endInclusive}
          onChange={(event) => setStart(event.target.value)}
        />
      </div>

      <div className="flex flex-col gap-1">
        <label htmlFor="range-end-date" className="text-xs text-muted-foreground select-none">
          {zh.overview.range.endDate}
        </label>
        <input
          id="range-end-date"
          data-testid="range-end-date"
          type="date"
          className={DATE_INPUT_CLASS}
          value={endInclusive}
          min={start === '' ? undefined : start}
          onChange={(event) => setEndInclusive(event.target.value)}
        />
      </div>

      <p data-testid="range-custom-hint" className="text-xs text-muted-foreground">
        {issue ?? zh.overview.range.endDateInclusiveHint}
      </p>

      <div className="flex items-center justify-between gap-2">
        <Button
          size="sm"
          variant="ghost"
          data-testid="range-custom-clear"
          onClick={() => {
            setStart('')
            setEndInclusive('')
          }}
        >
          {zh.overview.range.clear}
        </Button>
        <Button
          size="sm"
          data-testid="range-custom-apply"
          disabled={issue !== null}
          onClick={() => onApply(start, endInclusive)}
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
                data-testid="range-custom-panel"
                sideOffset={8}
                align="start"
                className="z-50 rounded-xl border border-border bg-popover p-4 shadow-xl"
              >
                <DateRangePanel
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
