/**
 * Presentation-level date/time helpers — **the one place the whole UI formats a moment**.
 *
 * Owner: W8 prep (shell/infrastructure). Shared by todos 15-19.
 *
 * Deliberately dependency-free (no date-fns / dayjs / moment — hard plan constraint).
 * `Intl` is the platform's own tzdb binding, not a dependency.
 *
 * Two kinds of value reach the UI and they must never be confused:
 *
 * 1. **A raw instant** — UTC epoch milliseconds (`Host.lastSuccessUtc`,
 *    `MessageRow.timeCreatedUtc`) or an offset-bearing RFC 3339 stamp (`LogEntry.timestamp`,
 *    written by Rust `chrono::Local`). These carry no display timezone of their own, so the
 *    frontend renders them here, in the **report timezone**, via {@link formatInstantInZone} /
 *    {@link formatOffsetStampInZone}. Every such surface goes through these two functions so
 *    "报表时区: Asia/Shanghai" is true of every clock on screen, not just some of them.
 *
 * 2. **A string the backend already formatted in the report timezone** — `TimeBucket.label`
 *    and `DateRange.startDate` / `endDateExclusive`. Rust computed those with `chrono_tz`
 *    against the same IANA name. Passing them through anything here would convert an
 *    already-converted value and shift the clock twice, so they are rendered verbatim.
 *    There is still exactly one bucketing engine and it is in Rust.
 *
 * Calendar *arithmetic* likewise stays split: bucket boundaries, DST folds/gaps and week
 * starts are Rust's; the only arithmetic below is the whole-day `YYYY-MM-DD` shifting that
 * `DateRange` needs, anchored at UTC noon so no DST transition can perturb it.
 */
import type { DateRange, WeekStart } from '@/generated'

/**
 * `sv-SE` is used purely for its CLDR short-date pattern (`YYYY-MM-DD HH:mm:ss`): sortable by
 * eye, locale-neutral, and it needs no part reassembly. The locale carries no visible words.
 */
const INSTANT_FORMAT_OPTIONS: Intl.DateTimeFormatOptions = {
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
  hour12: false,
}

/**
 * `Intl.DateTimeFormat` construction is not free and these run once per row per render, so
 * formatters are memoised per timezone rather than rebuilt.
 */
const INSTANT_FORMATS = new Map<string, Intl.DateTimeFormat>()

function instantFormatFor(timezone: string): Intl.DateTimeFormat {
  const cached = INSTANT_FORMATS.get(timezone)
  if (cached !== undefined) return cached
  let built: Intl.DateTimeFormat
  try {
    built = new Intl.DateTimeFormat('sv-SE', { ...INSTANT_FORMAT_OPTIONS, timeZone: timezone })
  } catch {
    built = new Intl.DateTimeFormat('sv-SE', { ...INSTANT_FORMAT_OPTIONS, timeZone: 'UTC' })
  }
  INSTANT_FORMATS.set(timezone, built)
  return built
}

/**
 * UTC epoch milliseconds → `YYYY-MM-DD HH:mm:ss` **in the report timezone**, or `null` when
 * there is no instant to show.
 *
 * Presentation only: it renders one instant and never derives a new one, so it cannot
 * disagree with the bucket that instant falls into — both sides resolve the same IANA name
 * against the same tzdb. Unknown timezone names fall back to UTC rather than throwing, which
 * is what keeps a stale persisted setting from blanking every clock in the app.
 */
export function formatInstantInZone(
  epochMs: number | null | undefined,
  timezone: string,
): string | null {
  if (typeof epochMs !== 'number' || !Number.isFinite(epochMs)) return null
  return instantFormatFor(timezone).format(new Date(epochMs))
}

/**
 * Strict RFC 3339 with a mandatory zone designator.
 *
 * Strict on purpose: `Date.parse` also accepts `'2026-08-07 09:58:05'` and interprets it as
 * the *machine's* local time, which would silently shift a log line by the difference between
 * the OS zone and the report zone. Anything this pattern rejects is left as written instead.
 */
const RFC3339_WITH_ZONE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/

/** Epoch milliseconds for an offset-bearing RFC 3339 stamp, or `null` if it is not one. */
export function parseOffsetStamp(stamp: string): number | null {
  if (!RFC3339_WITH_ZONE.test(stamp)) return null
  const epochMs = Date.parse(stamp)
  return Number.isFinite(epochMs) ? epochMs : null
}

/**
 * An offset-bearing RFC 3339 stamp → `YYYY-MM-DD HH:mm:ss` **in the report timezone**.
 *
 * The offset in the input is what makes this lossless: the stamp names an instant, so
 * re-rendering it in the report timezone is a change of presentation, not of meaning. Rust
 * writes log records with the *machine's* local offset (`chrono::Local`), which is a third
 * zone the user never chose — reading it verbatim was the one clock in the app that did not
 * answer to 报表时区.
 *
 * Anything not shaped like an offset-bearing stamp is passed through untouched rather than
 * mangled: a record whose timestamp the parser cannot vouch for is more useful shown as
 * written than converted on a guess.
 */
export function formatOffsetStampInZone(stamp: string, timezone: string): string {
  const epochMs = parseOffsetStamp(stamp)
  if (epochMs === null) return stamp
  return formatInstantInZone(epochMs, timezone) ?? stamp
}

/**
 * The range presets the UI offers. `custom` means "the user picked explicit dates".
 *
 * `thisQuarter` / `thisYear` are calendar-aligned, not rolling: "this quarter" means Q3, not
 * "the last 92 days". Both are period-**to-date** — extending past today would add buckets the
 * archive cannot cover, and the trend chart must draw those as breaks, which reads as data loss.
 */
export const RANGE_PRESETS = [
  'today',
  'last7Days',
  'last30Days',
  'thisQuarter',
  'thisYear',
  'custom',
] as const

export type RangePreset = (typeof RANGE_PRESETS)[number]

/** The system IANA timezone, or `UTC` when the runtime refuses to report one. */
export function systemTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

/**
 * The current local calendar date in `timezone`, formatted `YYYY-MM-DD`.
 *
 * Uses `en-CA`, whose short date format is already ISO-ordered, so no manual part
 * reassembly (and no off-by-one from `toISOString`) is involved.
 */
export function todayInTimezone(timezone: string, now: Date = new Date()): string {
  try {
    return new Intl.DateTimeFormat('en-CA', {
      timeZone: timezone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
    }).format(now)
  } catch {
    return new Intl.DateTimeFormat('en-CA', {
      timeZone: 'UTC',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
    }).format(now)
  }
}

/**
 * Shift a `YYYY-MM-DD` string by whole days.
 *
 * Anchored at UTC noon so the arithmetic can never be perturbed by a DST transition:
 * the result is a pure calendar-date shift, not an instant shift.
 */
export function shiftIsoDate(isoDate: string, days: number): string {
  const [year, month, day] = isoDate.split('-').map(Number)
  const anchor = Date.UTC(year, month - 1, day, 12)
  const shifted = new Date(anchor + days * 86_400_000)
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${shifted.getUTCFullYear()}-${pad(shifted.getUTCMonth() + 1)}-${pad(shifted.getUTCDate())}`
}

/** Number of whole days a rolling-window preset spans. */
const PRESET_SPAN_DAYS = {
  today: 1,
  last7Days: 7,
  last30Days: 30,
} as const

type RollingPreset = keyof typeof PRESET_SPAN_DAYS

function isRollingPreset(preset: RangePreset): preset is RollingPreset {
  return preset in PRESET_SPAN_DAYS
}

/** First day of the calendar quarter containing `isoDate`: Jan / Apr / Jul / Oct the 1st. */
export function quarterStartOf(isoDate: string): string {
  const [year, month] = isoDate.split('-').map(Number)
  const quarterFirstMonth = Math.floor((month - 1) / 3) * 3 + 1
  return `${String(year).padStart(4, '0')}-${String(quarterFirstMonth).padStart(2, '0')}-01`
}

export function yearStartOf(isoDate: string): string {
  return `${isoDate.slice(0, 4)}-01-01`
}

/**
 * Build the half-open `[startDate, endDateExclusive)` range for a preset.
 *
 * `custom` has no canonical span, so it falls back to `today`; callers that select
 * `custom` are expected to supply explicit dates.
 */
export function rangeForPreset(
  preset: RangePreset,
  timezone: string,
  weekStart: WeekStart,
  now: Date = new Date(),
): DateRange {
  const today = todayInTimezone(timezone, now)
  const endDateExclusive = shiftIsoDate(today, 1)
  if (preset === 'thisQuarter') {
    return { startDate: quarterStartOf(today), endDateExclusive, weekStart }
  }
  if (preset === 'thisYear') {
    return { startDate: yearStartOf(today), endDateExclusive, weekStart }
  }
  const span = isRollingPreset(preset) ? PRESET_SPAN_DAYS[preset] : PRESET_SPAN_DAYS.today
  return { startDate: shiftIsoDate(today, -(span - 1)), endDateExclusive, weekStart }
}

/** Whole-day span of a half-open range; used to pick a default granularity. */
export function rangeSpanDays(range: DateRange): number {
  const toUtcNoon = (isoDate: string) => {
    const [year, month, day] = isoDate.split('-').map(Number)
    return Date.UTC(year, month - 1, day, 12)
  }
  const span = (toUtcNoon(range.endDateExclusive) - toUtcNoon(range.startDate)) / 86_400_000
  return Math.max(0, Math.round(span))
}
