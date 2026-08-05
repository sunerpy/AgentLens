/**
 * Platform detection for the self-drawn titlebar.
 *
 * Deliberately **not** `@tauri-apps/plugin-os`: that would add an npm package, a Cargo
 * crate, a `.plugin(tauri_plugin_os::init())` call in `src-tauri/src/lib.rs` and an
 * `os:default` capability entry — four moving parts, plus a `Cargo.lock` churn — to answer
 * one question that the webview's own user-agent already answers synchronously. The
 * user-agent is also the only source available in the Playwright/vitest runs, where no
 * Tauri plugin is injected at all.
 */
export const TITLEBAR_PLATFORMS = ['macos', 'windows', 'linux', 'unknown'] as const

export type TitlebarPlatform = (typeof TITLEBAR_PLATFORMS)[number]

/**
 * Anything not positively identified maps to `unknown`, which the titlebar renders with
 * trailing-edge buttons and no leading inset — the safe majority layout. Mobile agents are
 * screened first because Android's user-agent also contains `Linux`, and iOS's contains
 * `Mac OS X`.
 */
export function detectPlatform(userAgent: string): TitlebarPlatform {
  if (/iPhone|iPad|iPod|Android/i.test(userAgent)) return 'unknown'
  if (/Mac OS X|Macintosh/i.test(userAgent)) return 'macos'
  if (/Windows/i.test(userAgent)) return 'windows'
  if (/Linux|X11|CrOS|BSD|SunOS/i.test(userAgent)) return 'linux'
  return 'unknown'
}

export function currentPlatform(): TitlebarPlatform {
  if (typeof navigator === 'undefined' || typeof navigator.userAgent !== 'string') {
    return 'unknown'
  }
  return detectPlatform(navigator.userAgent)
}
