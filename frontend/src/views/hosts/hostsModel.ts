/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Pure helpers behind the hosts view: joining hosts to their refresh status, and
 * extracting the backend's Chinese remediation out of a structured `IpcError`.
 * Keeping these free of React makes them assertable on their own.
 */
import type { Host, IpcError, SourceStatus } from '@/generated'
import { zh } from '@/i18n/zh'
import { toIpcError } from '@/lib/ipc'
import { formatInstantInZone } from '@/lib/localDate'

/**
 * One host and every scheduler slot registered for it.
 *
 * The scheduler is keyed `(host_id, source)`, so one host has one status **per enabled
 * source**: OpenCode can be idle while Claude Code is failing. Collapsing them to a single
 * status would silently hide the failure, so the row carries the whole list.
 */
export interface HostRowModel {
  host: Host
  statuses: readonly SourceStatus[]
}

export function joinHostStatus(
  hosts: readonly Host[],
  statuses: readonly SourceStatus[],
): HostRowModel[] {
  const byHostId = new Map<string, SourceStatus[]>()
  for (const status of statuses) {
    const bucket = byHostId.get(status.hostId)
    if (bucket === undefined) byHostId.set(status.hostId, [status])
    else bucket.push(status)
  }
  return hosts.map((host) => ({ host, statuses: byHostId.get(host.hostId) ?? [] }))
}

/**
 * The single state the row's headline badge shows.
 *
 * `error` wins over everything: a row whose Claude Code slot failed must read as failing even
 * while its OpenCode slot is idle, because the opposite would present a broken host as healthy.
 */
export function rowStateKey(statuses: readonly SourceStatus[]): HostStateKey {
  if (statuses.length === 0) return 'unknown'
  if (statuses.some((status) => status.state.state === 'error')) return 'error'
  if (statuses.some((status) => status.state.state === 'running')) return 'running'
  return 'idle'
}

export type HostStateKey = 'idle' | 'running' | 'error' | 'unknown'

export function hostStateKey(status: SourceStatus | undefined): HostStateKey {
  return status === undefined ? 'unknown' : status.state.state
}

export function hostStateLabel(state: HostStateKey): string {
  switch (state) {
    case 'idle':
      return zh.hosts.list.stateIdle
    case 'running':
      return zh.hosts.list.stateRunning
    case 'error':
      return zh.hosts.list.stateError
    default:
      return zh.hosts.list.statusUnavailable
  }
}

/**
 * The error text a row shows. `SourceState::Error` carries the authoritative message the
 * refresh round failed with (already a Chinese remediation when it came from
 * `agentlens_core::transport::ssh`), so it wins over the flat `lastError` mirror.
 */
export function hostErrorText(status: SourceStatus | undefined): string | null {
  if (status === undefined) return null
  if (status.state.state === 'error') return status.state.last_error
  return status.lastError
}

/** The last success one scheduler slot can attest to, or `null` when it never succeeded. */
export function statusLastSuccessUtc(status: SourceStatus): number | null {
  if (status.state.state === 'error' && status.state.last_success !== null) {
    return status.state.last_success
  }
  return status.lastSuccessUtc
}

/**
 * `last_success` inside the error variant is preserved by the scheduler across failures,
 * so a broken host still shows when it last worked instead of claiming "never".
 *
 * With several sources per host the row shows the most recent of them: "最近成功" is a
 * property of the host, and the newest success is the one that answers "is this host alive".
 */
export function hostLastSuccessUtc(row: HostRowModel): number | null {
  const candidates = row.statuses
    .map(statusLastSuccessUtc)
    .filter((value): value is number => value !== null)
  if (candidates.length > 0) return Math.max(...candidates)
  return row.host.lastSuccessUtc
}

/**
 * Remediation text carried in `IpcError.fields.remediation`.
 *
 * The Rust command layer puts the typed variant's Chinese `remediation()` there rather
 * than inventing a new `IpcErrorCode`, because the shared `isIpcError` guard validates
 * `code` against a fixed list.
 */
export function ipcRemediation(error: unknown): string | null {
  const fields = toIpcError(error).fields
  const remediation = fields?.remediation
  return typeof remediation === 'string' && remediation.length > 0 ? remediation : null
}

/** Typed SSH failure variant, e.g. `authFailed`; `null` for non-SSH failures. */
export function ipcVariant(error: unknown): string | null {
  const variant = toIpcError(error).fields?.variant
  return typeof variant === 'string' && variant.length > 0 ? variant : null
}

export function ipcMessage(error: unknown): IpcError {
  return toIpcError(error)
}

/**
 * Variants where the actionable next step is "pick a key file". Windows without a usable
 * ssh-agent lands here, which is why the form's guidance is bound to these two.
 */
const KEY_FILE_GUIDANCE_VARIANTS: readonly string[] = ['authFailed', 'sshUnavailable']

export function needsKeyFileGuidance(error: unknown): boolean {
  const variant = ipcVariant(error)
  return variant !== null && KEY_FILE_GUIDANCE_VARIANTS.includes(variant)
}

/** `user@host` when a user was given, otherwise the bare alias from `~/.ssh/config`. */
export function composeSshTarget(user: string, host: string): string {
  const trimmedHost = host.trim()
  const trimmedUser = user.trim()
  return trimmedUser === '' ? trimmedHost : `${trimmedUser}@${trimmedHost}`
}

/**
 * UTC epoch milliseconds → `YYYY-MM-DD HH:mm:ss` **in the report timezone**.
 *
 * A thin alias over {@link formatInstantInZone}: host rows, detail rows and log records must
 * agree to the second, so they all share one formatter in `@/lib/localDate` rather than three
 * near-identical `Intl` wrappers that could drift in locale, options or fallback behaviour.
 */
export function formatTimestampInZone(epochMs: number | null, timezone: string): string | null {
  return formatInstantInZone(epochMs, timezone)
}

const KIB_UNITS = ['KiB', 'MiB', 'GiB', 'TiB'] as const

export function formatKib(availableKib: number): string {
  let value = availableKib
  let unitIndex = 0
  while (value >= 1024 && unitIndex < KIB_UNITS.length - 1) {
    value /= 1024
    unitIndex += 1
  }
  const rounded = value >= 100 || unitIndex === 0 ? Math.round(value) : Math.round(value * 10) / 10
  return `${rounded} ${KIB_UNITS[unitIndex]}`
}

const MACHINE_ID_HASH_PATTERN = /^[0-9a-f]{64}$/

export function isMachineIdHash(value: string): boolean {
  return MACHINE_ID_HASH_PATTERN.test(value.trim().toLowerCase())
}
