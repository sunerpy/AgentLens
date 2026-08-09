/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Local IPC wrapper for the commands todo 18 added to `src-tauri`.
 *
 * Why this exists instead of an edit to `@/lib/ipc`: that file is shared shell surface a
 * sibling worker holds during this wave, so an edit there would be lost in a concurrent
 * write. The shape here is deliberately identical to `@/lib/ipc` — typed `invoke`,
 * argument keys copied verbatim from `src-tauri/src/commands.rs`, DTOs imported from
 * `@/generated` only — so a future consolidation is a file move, not a rewrite.
 */
import { invoke } from '@tauri-apps/api/core'

import type {
  CredentialKind,
  CredentialStatus,
  LocalIdentity,
  SshProbeInput,
  SshProbeResult,
} from '@/generated'

/** The commands added by todo 18; kept alongside `IPC_COMMANDS` for the same purpose. */
export const HOSTS_IPC_COMMANDS = [
  'local_machine_identity',
  'ssh_probe',
  'ssh_probe_cancel',
  'credential_set',
  'credential_status',
  'credential_delete',
] as const

export type HostsIpcCommand = (typeof HOSTS_IPC_COMMANDS)[number]

export function localMachineIdentity(): Promise<LocalIdentity> {
  return invoke<LocalIdentity>('local_machine_identity')
}

export function newSshProbeRequestId(): string {
  return crypto.randomUUID()
}

/** Runs the SSH connection probe. Rejects with an `IpcError` carrying `fields.remediation`. */
export function sshProbe(input: SshProbeInput, requestId: string): Promise<SshProbeResult> {
  return invoke<SshProbeResult>('ssh_probe', { input, requestId })
}

export function sshProbeCancel(requestId: string): Promise<void> {
  return invoke<void>('ssh_probe_cancel', { requestId })
}

/**
 * Writes a secret to the OS keyring. The secret travels over IPC exactly once and is
 * never returned: the response only reports presence.
 */
export function credentialSet(
  hostId: string,
  kind: CredentialKind,
  secret: string,
): Promise<CredentialStatus> {
  return invoke<CredentialStatus>('credential_set', { hostId, kind, secret })
}

export function credentialStatus(hostId: string, kind: CredentialKind): Promise<CredentialStatus> {
  return invoke<CredentialStatus>('credential_status', { hostId, kind })
}

export function credentialDelete(hostId: string, kind: CredentialKind): Promise<CredentialStatus> {
  return invoke<CredentialStatus>('credential_delete', { hostId, kind })
}
