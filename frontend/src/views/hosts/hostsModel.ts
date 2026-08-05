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

export interface HostRowModel {
  host: Host
  status: SourceStatus | undefined
}

export function joinHostStatus(
  hosts: readonly Host[],
  statuses: readonly SourceStatus[],
): HostRowModel[] {
  const byHostId = new Map(statuses.map((status) => [status.hostId, status]))
  return hosts.map((host) => ({ host, status: byHostId.get(host.hostId) }))
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

/**
 * `last_success` inside the error variant is preserved by the scheduler across failures,
 * so a broken host still shows when it last worked instead of claiming "never".
 */
export function hostLastSuccessUtc(row: HostRowModel): number | null {
  const { host, status } = row
  if (status !== undefined) {
    if (status.state.state === 'error' && status.state.last_success !== null) {
      return status.state.last_success
    }
    if (status.lastSuccessUtc !== null) return status.lastSuccessUtc
  }
  return host.lastSuccessUtc
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

const TIME_FORMAT = new Intl.DateTimeFormat('sv-SE', {
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
  timeZone: 'UTC',
})

/** UTC epoch milliseconds → `YYYY-MM-DD HH:mm:ss`. No calendar arithmetic, no date library. */
export function formatUtcTimestamp(epochMs: number | null): string | null {
  if (epochMs === null || !Number.isFinite(epochMs)) return null
  return TIME_FORMAT.format(new Date(epochMs))
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
