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
 * Whether timer-driven refresh runs at all. Absent means enabled, matching the Rust
 * `resolve_auto_refresh_enabled`, so an installation predating the toggle keeps refreshing.
 */
export const SETTING_KEY_AUTO_REFRESH_ENABLED = 'refresh.autoRefreshEnabled'
/**
 * Whether a checked update may be installed automatically. Absent means enabled, matching Rust's
 * `resolve_auto_update_enabled`, so installations predating the setting retain the default policy.
 */
export const SETTING_KEY_AUTO_UPDATE_ENABLED = 'update.autoInstallEnabled'

/**
 * Hard floor for both refresh intervals, mirroring `MIN_AUTO_REFRESH_INTERVAL_MS`.
 *
 * One remote round starts six `ssh`/`scp` processes and a full scan of a real archive measured
 * 23 s, so a shorter poll across several hosts risks overlapping rounds.
 *
 * The backend **rejects** a sub-floor write rather than clamping it, and this module matches
 * that: a user who typed 60 and was silently given 600 would keep believing the app polls every
 * minute. So the UI reports an error and refuses to save instead of correcting the value.
 */
export const MIN_INTERVAL_SECONDS = 600
export const DEFAULT_LOCAL_INTERVAL_SECONDS = 600
export const DEFAULT_REMOTE_INTERVAL_SECONDS = 900

export type IntervalIssue = 'malformed' | 'belowFloor'

export interface ParsedInterval {
  seconds: number | null
  issue: IntervalIssue | null
}

/**
 * Parses a user-entered interval in whole seconds.
 *
 * Blank, non-numeric, non-integer, zero and negative inputs are `malformed`; a well-formed
 * value under the floor is `belowFloor`. Both leave `seconds` null so no caller can accidentally
 * persist a refused value.
 */
export function parseIntervalSeconds(raw: string): ParsedInterval {
  const trimmed = raw.trim()
  if (!/^\d+$/.test(trimmed)) return { seconds: null, issue: 'malformed' }
  const parsed = Number.parseInt(trimmed, 10)
  if (!Number.isSafeInteger(parsed) || parsed <= 0) return { seconds: null, issue: 'malformed' }
  if (parsed < MIN_INTERVAL_SECONDS) return { seconds: null, issue: 'belowFloor' }
  return { seconds: parsed, issue: null }
}

/** Reads the auto-refresh toggle exactly as `resolve_auto_refresh_enabled` does in Rust. */
export function autoRefreshEnabledFromSettings(
  values: Readonly<Record<string, string | undefined>>,
): boolean {
  const raw = values[SETTING_KEY_AUTO_REFRESH_ENABLED]
  if (raw === undefined) return true
  return !['false', '0', 'off', 'no'].includes(raw.trim())
}

/** Reads the automatic-update toggle exactly as `resolve_auto_update_enabled` does in Rust. */
export function autoUpdateEnabledFromSettings(
  values: Readonly<Record<string, string | undefined>>,
): boolean {
  const raw = values[SETTING_KEY_AUTO_UPDATE_ENABLED]
  if (raw === undefined) return true
  return !['false', '0', 'off', 'no'].includes(raw.trim())
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
