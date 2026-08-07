import { expect, test } from '@playwright/test'

import { zh } from '../src/i18n/zh'
import { mockCalls, openShell, qaScreenshot } from './harness'

/**
 * Diagnostics view — the log tail and the feedback hand-off.
 *
 * The mock seeds one record per level (see `mockIpc.ts`), so severity filtering, ordering and
 * the copy path are all assertable without a real backend.
 */

async function openDiagnostics(page: Parameters<typeof openShell>[0]): Promise<void> {
  await page.getByTestId('nav-diagnostics').click()
  await expect(page.getByTestId('view-diagnostics')).toBeVisible()
}

test('the log list renders newest first with one row per seeded level', async ({ page }) => {
  await openShell(page)
  await openDiagnostics(page)

  const rows = page.getByTestId('diagnostics-log-row')
  await expect(rows).toHaveCount(5)

  // Newest first: the mock's last-written record is the ERROR one.
  await expect(rows.first()).toHaveAttribute('data-level', 'error')
  await expect(rows.first().getByTestId('diagnostics-log-message')).toHaveText(
    'archive unavailable: database is locked',
  )
  await expect(rows.last()).toHaveAttribute('data-level', 'trace')
  await expect(page.getByTestId('diagnostics-count')).toContainText('5')

  await qaScreenshot(page, 'diagnostics-logs.png')
})

test('the level filter keeps entries at or above the chosen severity', async ({ page }) => {
  await openShell(page)
  await openDiagnostics(page)

  await page.getByTestId('diagnostics-level-warn').click()
  await expect(page.getByTestId('diagnostics-level-warn')).toHaveAttribute('aria-pressed', 'true')
  const rows = page.getByTestId('diagnostics-log-row')
  // Selecting WARN must not hide the ERRORs a user picked WARN in order to find.
  await expect(rows).toHaveCount(2)
  await expect(rows.first()).toHaveAttribute('data-level', 'error')
  await expect(rows.last()).toHaveAttribute('data-level', 'warn')

  await page.getByTestId('diagnostics-level-error').click()
  await expect(page.getByTestId('diagnostics-log-row')).toHaveCount(1)

  await page.getByTestId('diagnostics-level-all').click()
  await expect(page.getByTestId('diagnostics-log-row')).toHaveCount(5)
})

test('log text stays selectable so Ctrl+C works when the clipboard API does not', async ({
  page,
}) => {
  await openShell(page)
  await openDiagnostics(page)

  const message = page.getByTestId('diagnostics-log-message').first()
  await expect(message).toHaveCSS('user-select', 'text')

  const selected = await message.evaluate((element) => {
    const range = document.createRange()
    range.selectNodeContents(element)
    const selection = window.getSelection()
    selection?.removeAllRanges()
    selection?.addRange(range)
    return window.getSelection()?.toString() ?? ''
  })
  expect(selected).toContain('archive unavailable')
})

test('the copy button writes the visible list to the clipboard', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await openShell(page)
  await openDiagnostics(page)

  await page.getByTestId('diagnostics-level-warn').click()
  await page.getByTestId('diagnostics-copy').click()
  await expect(page.getByTestId('diagnostics-copied')).toBeVisible()

  const clipboard = await page.evaluate(() => navigator.clipboard.readText())
  expect(clipboard.split('\n')).toHaveLength(2)
  expect(clipboard).toContain('ERROR')
  expect(clipboard).toContain('WARN')
  expect(clipboard).not.toContain('TRACE')
})

test('the log directory is revealed through the opener plugin', async ({ page }) => {
  await openShell(page)
  await openDiagnostics(page)

  const directory = page.getByTestId('diagnostics-directory')
  await expect(directory).toContainText('logs')
  const shown = (await directory.textContent()) ?? ''

  await expect(page.getByTestId('diagnostics-open-notice')).toHaveCount(0)
  await page.getByTestId('diagnostics-open-directory').click()
  // The mock installs `__TAURI_INTERNALS__` but not a file manager, so the reveal is
  // attempted and refused — the `openFailed` branch, exactly as in the settings view.
  await expect(page.getByTestId('diagnostics-open-notice')).toHaveText(
    zh.diagnostics.logs.openFailed,
  )

  // The wire contract: one reveal for exactly the displayed directory, as an array.
  const reveals = await mockCalls(page, 'plugin:opener|reveal_item_in_dir')
  expect(reveals).toHaveLength(1)
  expect(reveals[0]?.args).toMatchObject({ paths: [shown] })
})

test('an empty log directory says so instead of rendering a blank panel', async ({ page }) => {
  await openShell(page, {
    responses: {
      logs_tail: { directory: '/tmp/agentlens/logs', entries: [], empty: true },
    },
  })
  await openDiagnostics(page)

  await expect(page.getByTestId('empty-state')).toHaveText(zh.diagnostics.logs.empty)
  await expect(page.getByTestId('diagnostics-copy')).toBeDisabled()
})

test('a filtered-out level is distinguished from an empty log', async ({ page }) => {
  await openShell(page)
  await openDiagnostics(page)

  await page.getByTestId('diagnostics-level-error').click()
  await page.getByTestId('diagnostics-level-error').click()
  await expect(page.getByTestId('diagnostics-log-row')).toHaveCount(1)

  await openShell(page, {
    responses: {
      logs_tail: {
        directory: '/tmp/agentlens/logs',
        empty: false,
        entries: [
          {
            timestamp: '2026-08-07T09:00:00.000+08:00',
            level: 'info',
            target: 't',
            message: 'only info here',
          },
        ],
      },
    },
  })
  await openDiagnostics(page)
  await page.getByTestId('diagnostics-level-error').click()
  await expect(page.getByTestId('empty-state')).toHaveText(zh.diagnostics.logs.emptyFiltered)
})

test('a structured IpcError renders the error panel and retries on demand', async ({ page }) => {
  await openShell(page, {
    errors: {
      logs_tail: {
        code: 'internal',
        message: 'log directory is unavailable',
        fields: {},
      },
    },
  })
  await openDiagnostics(page)

  await expect(page.getByTestId('error-state')).toBeVisible()
  await expect(page.getByTestId('error-code')).toHaveText('internal')
  await expect(page.getByTestId('error-message')).toHaveText('log directory is unavailable')
})

test('refresh re-reads the log tail', async ({ page }) => {
  await openShell(page)
  await openDiagnostics(page)

  const before = (await mockCalls(page, 'logs_tail')).length
  expect(before).toBeGreaterThan(0)

  await page.getByTestId('diagnostics-refresh').click()
  await expect.poll(async () => (await mockCalls(page, 'logs_tail')).length).toBeGreaterThan(before)
})

test('the feedback card shows exactly the environment facts that will be published', async ({
  page,
}) => {
  await openShell(page)
  await openDiagnostics(page)

  await expect(page.getByTestId('diagnostics-privacy')).toHaveText(
    zh.diagnostics.feedback.privacyNotice,
  )
  await expect(page.getByTestId('diagnostics-app-version')).toHaveText('0.1.0')
  await expect(page.getByTestId('diagnostics-os')).toHaveText('linux')
  await expect(page.getByTestId('diagnostics-arch')).toHaveText('x86_64')
  await expect(page.getByTestId('diagnostics-webview')).toHaveText('2.48.1')

  await qaScreenshot(page, 'diagnostics-feedback.png')
})

test('an unreported webview version reads as unknown rather than blank', async ({ page }) => {
  await openShell(page, {
    responses: {
      diagnostics_report: {
        appVersion: '9.9.9',
        os: 'windows',
        arch: 'aarch64',
        webviewVersion: null,
      },
    },
  })
  await openDiagnostics(page)

  await expect(page.getByTestId('diagnostics-webview')).toHaveText(
    zh.diagnostics.feedback.webviewUnknown,
  )
})

/**
 * The privacy invariant, asserted on what the UI would actually hand to the browser rather
 * than on the URL builder in isolation.
 */
test('the prefilled issue link carries no host, path or credential material', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await openShell(page)
  await openDiagnostics(page)

  await page.getByTestId('diagnostics-copy-link').click()
  await expect(page.getByTestId('diagnostics-link-copied')).toBeVisible()

  const link = await page.evaluate(() => navigator.clipboard.readText())
  expect(link.startsWith('https://github.com/sunerpy/AgentLens/issues/new')).toBe(true)
  expect(link).toContain('template=bug_report.yml')
  expect(link).toContain('app-version=0.1.0')
  for (const forbidden of ['ssh', '%40', '%2Fhome', 'archive.db', 'password', 'token', '192.168']) {
    expect(link.toLowerCase()).not.toContain(forbidden.toLowerCase())
  }
})

test('the feedback button hands the prefilled url to the opener plugin', async ({ page }) => {
  await openShell(page)
  await openDiagnostics(page)

  await expect(page.getByTestId('diagnostics-issue-notice')).toHaveCount(0)
  await page.getByTestId('diagnostics-open-issue').click()
  // No browser behind the mock, so the refusal branch renders instead of throwing.
  await expect(page.getByTestId('diagnostics-issue-notice')).toHaveText(
    zh.diagnostics.feedback.openFailed,
  )

  const opens = await mockCalls(page, 'plugin:opener|open_url')
  expect(opens).toHaveLength(1)
  const args = opens[0]?.args as { url?: unknown } | undefined
  const url = String(args?.url ?? '')
  expect(url.startsWith('https://github.com/sunerpy/AgentLens/issues/new')).toBe(true)
  for (const forbidden of ['ssh', '%40', '%2Fhome', 'archive.db', 'password', 'token']) {
    expect(url.toLowerCase()).not.toContain(forbidden.toLowerCase())
  }
})
