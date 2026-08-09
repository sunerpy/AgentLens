import { revealItemInDir } from '@tauri-apps/plugin-opener'

/**
 * Reveal a filesystem path in the OS file manager.
 *
 * Three outcomes rather than a thrown error, because the caller has to say something
 * different for each: `unsupported` means there is no desktop shell to ask (a plain
 * `vite dev` browser tab, or the Playwright QA run), so the copy button is the answer;
 * `failed` means the shell tried and the OS refused — on Linux that is a missing
 * `xdg-open`/DBus FileManager1, which the user can act on.
 */
export type RevealOutcome = 'revealed' | 'unsupported' | 'failed'

/**
 * A browser tab has no `__TAURI_INTERNALS__` at all, and `invoke` there rejects with a
 * generic message that is indistinguishable from a real OS failure — so the bridge is
 * probed up front instead of inferred from the rejection.
 */
function hasTauriBridge(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

export async function revealPath(path: string): Promise<RevealOutcome> {
  if (!hasTauriBridge()) return 'unsupported'
  try {
    await revealItemInDir(path)
    return 'revealed'
  } catch {
    return 'failed'
  }
}
