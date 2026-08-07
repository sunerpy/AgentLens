/**
 * Typed Tauri IPC client — the single place the UI is allowed to call `invoke`.
 *
 * Owner: W8 prep (shell/infrastructure). Views (todos 15-19) MUST import from here
 * and MUST NOT call `invoke` directly, so that command names and argument shapes
 * live in exactly one file.
 *
 * Contract notes:
 * - Every payload / return type is imported from `@/generated`, which is produced by
 *   `cargo test -p agentlens-tauri --features ts-export bindings_export`. Never hand-write
 *   a DTO here; if a shape is missing, the Rust side is the place to change.
 * - Tauri v2 exposes Rust command parameters as a single argument object with **camelCase**
 *   keys (`host_id` on the Rust side is `hostId` on the wire). The literal key names below
 *   were derived from `src-tauri/src/commands.rs` and must stay in sync with it.
 * - Failures reject with the serialized `IpcError` object (`{ code, message, fields }`).
 *   Use `toIpcError` to narrow a caught value instead of `String(error)`.
 */
import { Channel, invoke } from '@tauri-apps/api/core'

import type {
  AggregateFilters,
  AppSettings,
  BreakdownDimensions,
  BreakdownRow,
  DateRange,
  Granularity,
  Host,
  HostCreateInput,
  HostUpdateInput,
  IpcError,
  IpcErrorCode,
  MessageFilters,
  MessagePage,
  PriceTable,
  RefreshEvent,
  SeriesPoint,
  SourceStatus,
  Summary,
  TriggerRefreshResult,
} from '@/generated'

/** The 15 commands registered in `src-tauri/src/lib.rs`. */
export const IPC_COMMANDS = [
  'get_summary',
  'get_trend',
  'get_breakdown',
  'query_messages',
  'hosts_list',
  'hosts_get',
  'hosts_create',
  'hosts_update',
  'hosts_delete',
  'trigger_refresh',
  'get_refresh_status',
  'get_settings',
  'set_settings',
  'prices_get',
  'prices_set',
] as const

export type IpcCommand = (typeof IPC_COMMANDS)[number]

const IPC_ERROR_CODES: readonly IpcErrorCode[] = [
  'invalidInput',
  'invalidRange',
  'invalidTimezone',
  'notFound',
  'conflict',
  'database',
  'pricing',
  'refresh',
  'internal',
]

/** True when `value` is the structured error object the Rust command layer returns. */
export function isIpcError(value: unknown): value is IpcError {
  if (typeof value !== 'object' || value === null) return false
  const candidate = value as Partial<IpcError>
  return (
    typeof candidate.message === 'string' &&
    typeof candidate.code === 'string' &&
    IPC_ERROR_CODES.includes(candidate.code)
  )
}

/**
 * Narrow anything thrown by an IPC call into an `IpcError`.
 *
 * Views should render `error.code` + `error.message` (and optionally `error.fields`)
 * rather than `String(error)`, which would leak `[object Object]`.
 */
export function toIpcError(error: unknown): IpcError {
  if (isIpcError(error)) {
    return { code: error.code, message: error.message, fields: error.fields ?? {} }
  }
  const message =
    error instanceof Error
      ? error.message
      : typeof error === 'string'
        ? error
        : JSON.stringify(error)
  return { code: 'internal', message: message ?? '', fields: {} }
}

// ---------------------------------------------------------------------------
// Aggregate queries
// ---------------------------------------------------------------------------

export function getSummary(
  range: DateRange,
  tz: string,
  filters: AggregateFilters,
): Promise<Summary> {
  return invoke<Summary>('get_summary', { range, tz, filters })
}

export function getTrend(
  range: DateRange,
  tz: string,
  granularity: Granularity,
  filters: AggregateFilters | null = null,
): Promise<SeriesPoint[]> {
  return invoke<SeriesPoint[]>('get_trend', { range, tz, granularity, filters })
}

export function getBreakdown(range: DateRange, dims: BreakdownDimensions): Promise<BreakdownRow[]> {
  return invoke<BreakdownRow[]>('get_breakdown', { range, dims })
}

/**
 * Server-side paging. `limit` is clamped to 200 by the Rust layer and `totalCount`
 * is computed independently of LIMIT/OFFSET, so it is safe to drive a pager from it.
 */
export function queryMessages(
  filters: MessageFilters,
  limit: number,
  offset: number,
): Promise<MessagePage> {
  return invoke<MessagePage>('query_messages', { filters, limit, offset })
}

// ---------------------------------------------------------------------------
// Hosts
// ---------------------------------------------------------------------------

export function hostsList(): Promise<Host[]> {
  return invoke<Host[]>('hosts_list')
}

export function hostsGet(hostId: string): Promise<Host> {
  return invoke<Host>('hosts_get', { hostId })
}

export function hostsCreate(input: HostCreateInput): Promise<Host> {
  return invoke<Host>('hosts_create', { input })
}

export function hostsUpdate(input: HostUpdateInput): Promise<Host> {
  return invoke<Host>('hosts_update', { input })
}

export function hostsDelete(hostId: string): Promise<void> {
  return invoke<void>('hosts_delete', { hostId })
}

// ---------------------------------------------------------------------------
// Refresh scheduling
// ---------------------------------------------------------------------------

export function triggerRefresh(
  hostId: string,
  onEvent: (event: RefreshEvent) => void,
): Promise<TriggerRefreshResult> {
  const channel = new Channel<RefreshEvent>()
  channel.onmessage = onEvent
  return invoke<TriggerRefreshResult>('trigger_refresh', { hostId, onEvent: channel })
}

export function getRefreshStatus(): Promise<SourceStatus[]> {
  return invoke<SourceStatus[]>('get_refresh_status')
}

// ---------------------------------------------------------------------------
// Settings and prices
// ---------------------------------------------------------------------------

export function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>('get_settings')
}

/** Upsert-merge: keys absent from `settings.values` are left untouched by the backend. */
export function setSettings(settings: AppSettings): Promise<AppSettings> {
  return invoke<AppSettings>('set_settings', { settings })
}

export function pricesGet(): Promise<PriceTable> {
  return invoke<PriceTable>('prices_get')
}

/** Writes `prices.json` atomically on the Rust side; returns the reloaded table. */
export function pricesSet(prices: PriceTable): Promise<PriceTable> {
  return invoke<PriceTable>('prices_set', { prices })
}
