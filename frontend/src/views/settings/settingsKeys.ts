/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * `app_settings` keys and the interval clamp rule. The two report keys are imported from
 * `@/app/reportRange` rather than redeclared, so there is exactly one spelling of each key.
 */

/** Configured local refresh interval, in milliseconds, as stored in `app_settings`. */
export const SETTING_KEY_LOCAL_INTERVAL_MS = 'refresh.localIntervalMs'
/** Configured remote refresh interval, in milliseconds, as stored in `app_settings`. */
export const SETTING_KEY_REMOTE_INTERVAL_MS = 'refresh.remoteIntervalMs'
/** Archive database path, written by the desktop shell at boot; read-only for the UI. */
export const SETTING_KEY_ARCHIVE_PATH = 'archive.path'

/**
 * Hard floor for both refresh intervals.
 *
 * A full scan of a real archive measured 23 s, so a 60 s poll was rejected in review as
 * guaranteeing overlapping scans. The Rust shell enforces the same floor when it applies the
 * persisted value to the scheduler, so a value below it can never reach a refresh round.
 */
export const MIN_INTERVAL_SECONDS = 300
export const DEFAULT_LOCAL_INTERVAL_SECONDS = 300
export const DEFAULT_REMOTE_INTERVAL_SECONDS = 900

export interface ClampedInterval {
  seconds: number
  clamped: boolean
}

/**
 * Parses a user-entered interval and clamps it to [`MIN_INTERVAL_SECONDS`].
 *
 * Zero, negatives and anything unparseable land on the floor with `clamped` set, so the UI can
 * explain the correction instead of silently persisting an unusable interval.
 */
export function clampIntervalSeconds(raw: string): ClampedInterval {
  const parsed = Number.parseFloat(raw.trim())
  if (!Number.isFinite(parsed) || parsed < MIN_INTERVAL_SECONDS) {
    return { seconds: MIN_INTERVAL_SECONDS, clamped: true }
  }
  return { seconds: Math.floor(parsed), clamped: false }
}

/** Reads a millisecond interval out of `app_settings` and renders it as whole seconds. */
export function intervalSecondsFromSettings(
  values: Readonly<Record<string, string | undefined>>,
  key: string,
  fallbackSeconds: number,
): number {
  const parsed = Number.parseInt(values[key] ?? '', 10)
  if (!Number.isFinite(parsed) || parsed <= 0) return fallbackSeconds
  return Math.floor(parsed / 1000)
}
