/**
 * Log-level filtering and display formatting.
 *
 * Kept separate from the components so the ordering and the timestamp slicing are unit-testable
 * without a DOM: the level order decides what a filter selection includes, and getting it wrong
 * silently hides errors — the one thing this whole view exists to surface.
 */
import type { LogEntry, LogLevel } from '@/generated'

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
 * Trims the RFC 3339 stamp to `YYYY-MM-DD HH:MM:SS` for display.
 *
 * Pure string slicing on purpose: the Rust side already wrote local wall-clock time with its
 * offset, so there is nothing to convert, and the project deliberately ships no date library.
 * Anything not shaped like a stamp is passed through untouched rather than mangled.
 */
export function formatTimestamp(timestamp: string): string {
  if (timestamp.length < 19 || timestamp[10] !== 'T') return timestamp
  return `${timestamp.slice(0, 10)} ${timestamp.slice(11, 19)}`
}

/** Plain-text rendering of the visible list, for the clipboard. */
export function entriesToText(entries: readonly LogEntry[]): string {
  return entries
    .map(
      (entry) =>
        `${formatTimestamp(entry.timestamp)} ${entry.level.toUpperCase()} ${entry.target} ${entry.message}`,
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
