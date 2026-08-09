/**
 * Log-level filtering and display formatting.
 *
 * Kept separate from the components so the ordering and the timestamp slicing are unit-testable
 * without a DOM: the level order decides what a filter selection includes, and getting it wrong
 * silently hides errors — the one thing this whole view exists to surface.
 */
import type { LogEntry, LogLevel } from '@/generated'
import { formatOffsetStampInZone, parseOffsetStamp } from '@/lib/localDate'

/** Most to least severe, matching `tracing::Level`. */
export const LOG_LEVELS: readonly LogLevel[] = ['error', 'warn', 'info', 'debug', 'trace'] as const

export type LevelFilter = LogLevel | 'all'

/**
 * Keeps entries at or above `filter` in severity.
 *
 * "At or above" rather than "exactly": picking `warn` to hunt a problem and being shown warnings
 * while the errors are hidden would be the opposite of useful.
 */
export function filterEntries(entries: readonly LogEntry[], filter: LevelFilter): LogEntry[] {
  if (filter === 'all') return [...entries]
  const ceiling = LOG_LEVELS.indexOf(filter)
  return entries.filter((entry) => LOG_LEVELS.indexOf(entry.level) <= ceiling)
}

/**
 * Renders the RFC 3339 stamp as `YYYY-MM-DD HH:MM:SS` **in the report timezone**.
 *
 * Rust writes each record with the *machine's* local offset (`chrono::Local`), so the stamp
 * names an unambiguous instant but in a third zone the user never chose. Slicing it — which is
 * what this used to do — dropped the offset and left the one clock in the app that did not
 * answer to 报表时区: the same round could read `09:58` here and `01:58` on 主机.
 *
 * UTC was the alternative and was rejected: these records come from the local desktop shell
 * only, never from the remote hosts, so there is no cross-machine correlation to preserve —
 * the offset is already in the payload for anyone who needs the raw value. What UTC would buy
 * is nothing, and what it would cost is a second time standard on screen.
 *
 * A stamp the strict parser cannot vouch for is passed through untouched rather than converted
 * on a guess. See `formatOffsetStampInZone` for why the parse is strict.
 */
export function formatTimestamp(timestamp: string, timezone: string): string {
  if (parseOffsetStamp(timestamp) !== null) return formatOffsetStampInZone(timestamp, timezone)
  if (timestamp.length < 19 || timestamp[10] !== 'T') return timestamp
  return `${timestamp.slice(0, 10)} ${timestamp.slice(11, 19)}`
}

/**
 * Plain-text rendering of the visible list, for the clipboard.
 *
 * Takes the same `timezone` as the on-screen list so a pasted line reads identically to the
 * row it was copied from; a clipboard that silently used a different zone would make two
 * people comparing the same log disagree about when something happened.
 */
export function entriesToText(entries: readonly LogEntry[], timezone: string): string {
  return entries
    .map(
      (entry) =>
        `${formatTimestamp(entry.timestamp, timezone)} ${entry.level.toUpperCase()} ${entry.target} ${entry.message}`,
    )
    .join('\n')
}

/** Tailwind classes per level, so severity is readable at a glance rather than by reading. */
export const LEVEL_CLASS: Record<LogLevel, string> = {
  error: 'bg-destructive/15 text-destructive',
  warn: 'bg-chart-4/20 text-foreground',
  info: 'bg-primary/12 text-foreground',
  debug: 'bg-muted text-muted-foreground',
  trace: 'bg-muted text-muted-foreground',
}
