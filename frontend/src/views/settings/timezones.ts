/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * IANA timezone option list for the report-timezone dropdown.
 *
 * The picker is a native `<select>`, so an invalid IANA name cannot be typed in — the plan's
 * failure scenario ("非法 IANA 字符串被下拉约束") is enforced by the control itself rather than by
 * validation after the fact.
 */

type SupportedValuesOf = (key: 'timeZone') => string[]

/**
 * Used when the runtime does not expose `Intl.supportedValuesOf`, so the dropdown still offers
 * a usable spread of zones (including half-hour and 45-minute offsets and both DST hemispheres)
 * instead of collapsing to a single entry.
 */
const FALLBACK_TIMEZONES = [
  'UTC',
  'Africa/Cairo',
  'America/Chicago',
  'America/Los_Angeles',
  'America/New_York',
  'America/Sao_Paulo',
  'Asia/Kathmandu',
  'Asia/Kolkata',
  'Asia/Shanghai',
  'Asia/Singapore',
  'Asia/Tokyo',
  'Australia/Lord_Howe',
  'Australia/Sydney',
  'Europe/Berlin',
  'Europe/London',
  'Europe/Moscow',
  'Pacific/Auckland',
] as const

/** Sorted IANA zone names, always including `UTC` and the currently persisted value. */
export function timezoneOptions(current: string): string[] {
  const supportedValuesOf = (Intl as unknown as { supportedValuesOf?: SupportedValuesOf })
    .supportedValuesOf
  let names: string[] = []
  if (typeof supportedValuesOf === 'function') {
    try {
      names = supportedValuesOf('timeZone')
    } catch {
      names = []
    }
  }
  const unique = new Set(names.length > 0 ? names : FALLBACK_TIMEZONES)
  unique.add('UTC')
  if (current !== '') unique.add(current)
  return [...unique].sort((left, right) => left.localeCompare(right))
}
