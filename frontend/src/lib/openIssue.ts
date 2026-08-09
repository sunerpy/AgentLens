/**
 * GitHub issue hand-off.
 *
 * ## Why the log body is not prefilled
 *
 * The obvious feature is "attach the log to the issue". It is rejected on purpose. A log line
 * can carry an SSH target (`user@10.0.0.7`), an absolute archive path (which contains the OS
 * user name), or a machine-id hash — and auto-redaction is a blacklist: the one pattern nobody
 * anticipated ends up permanently published in a public issue tracker, where deleting it does
 * not un-publish it.
 *
 * So the split is: this module prefills only build-time and platform constants, which cannot
 * identify a machine no matter what the log contains, and the user pastes whichever log lines
 * they have read and accepted. A human reviewing three lines they chose is a better filter than
 * a regex guessing at data it has never seen.
 */
import { openUrl } from '@tauri-apps/plugin-opener'

import type { DiagnosticsReport } from '@/generated'

const ISSUE_URL = 'https://github.com/sunerpy/AgentLens/issues/new'

/** Must match the `id` fields in `.github/ISSUE_TEMPLATE/bug_report.yml`. */
const TEMPLATE = 'bug_report.yml'
const FIELD_APP_VERSION = 'app-version'
const FIELD_PLATFORM = 'platform'

/**
 * Same three outcomes as `revealPath`, for the same reason: `unsupported` means there is no
 * desktop shell to ask (a `vite dev` tab or the Playwright run), where copying the link is the
 * answer; `failed` means the shell tried and the OS refused.
 */
export type OpenIssueOutcome = 'opened' | 'unsupported' | 'failed'

function hasTauriBridge(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/** Single-line platform summary, e.g. `linux x86_64 · WebView 2.48.1`. */
export function platformSummary(report: DiagnosticsReport): string {
  const webview = report.webviewVersion
  const base = `${report.os} ${report.arch}`
  return webview === null || webview === '' ? base : `${base} · WebView ${webview}`
}

/**
 * Builds the prefilled issue URL.
 *
 * Only `appVersion` and the platform summary are placed in query parameters. Nothing derived
 * from the archive path, the host list, the keyring or the log body is read here — that is the
 * invariant `openIssue.test.ts` asserts, so a future field cannot be added silently.
 */
export function buildIssueUrl(report: DiagnosticsReport): string {
  const url = new URL(ISSUE_URL)
  url.searchParams.set('template', TEMPLATE)
  url.searchParams.set(FIELD_APP_VERSION, report.appVersion)
  url.searchParams.set(FIELD_PLATFORM, platformSummary(report))
  return url.toString()
}

export async function openIssue(report: DiagnosticsReport): Promise<OpenIssueOutcome> {
  if (!hasTauriBridge()) return 'unsupported'
  try {
    await openUrl(buildIssueUrl(report))
    return 'opened'
  } catch {
    return 'failed'
  }
}
