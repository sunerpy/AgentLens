import { afterEach, describe, expect, it, vi } from 'vitest'

import type { DiagnosticsReport } from '@/generated'

import { buildIssueUrl, openIssue, platformSummary } from './openIssue'

vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn(),
}))

const { openUrl } = await import('@tauri-apps/plugin-opener')
const openUrlMock = vi.mocked(openUrl)

const BRIDGE = '__TAURI_INTERNALS__'

function withBridge(): void {
  Object.defineProperty(window, BRIDGE, { value: {}, configurable: true, writable: true })
}

function report(overrides: Partial<DiagnosticsReport> = {}): DiagnosticsReport {
  return {
    appVersion: '0.1.0',
    os: 'linux',
    arch: 'x86_64',
    webviewVersion: '2.48.1',
    ...overrides,
  }
}

afterEach(() => {
  Reflect.deleteProperty(window, BRIDGE)
  openUrlMock.mockReset()
})

describe('platformSummary', () => {
  it('joins os, arch and the webview version', () => {
    expect(platformSummary(report())).toBe('linux x86_64 · WebView 2.48.1')
  })

  it('omits the webview clause when the runtime cannot report one', () => {
    expect(platformSummary(report({ webviewVersion: null }))).toBe('linux x86_64')
    expect(platformSummary(report({ webviewVersion: '' }))).toBe('linux x86_64')
  })
})

describe('buildIssueUrl', () => {
  it('targets the bug-report template with the version and platform prefilled', () => {
    const url = new URL(buildIssueUrl(report()))

    expect(url.origin).toBe('https://github.com')
    expect(url.pathname).toBe('/sunerpy/AgentLens/issues/new')
    expect(url.searchParams.get('template')).toBe('bug_report.yml')
    expect(url.searchParams.get('app-version')).toBe('0.1.0')
    expect(url.searchParams.get('platform')).toBe('linux x86_64 · WebView 2.48.1')
  })

  it('carries exactly three parameters, so a new one cannot be added unnoticed', () => {
    const url = new URL(buildIssueUrl(report()))
    expect([...url.searchParams.keys()].sort()).toEqual(['app-version', 'platform', 'template'])
  })

  /**
   * The load-bearing privacy assertion. Anything identifying that reached the prefill would be
   * published in a public issue tracker, where a later deletion does not un-publish it.
   */
  it('leaks no host, path, user or credential material', () => {
    const url = buildIssueUrl(
      report({
        // A hostile snapshot: if any of these ever reached DiagnosticsReport, the prefill
        // must still not carry them.
        appVersion: '0.1.0',
        os: 'linux',
        arch: 'x86_64',
      }),
    )

    for (const forbidden of [
      'ssh',
      '@',
      '/home/',
      'C:\\',
      'Users',
      'archive.db',
      'machineId',
      'machine_id',
      'password',
      'passphrase',
      'token',
      'secret',
      'keyring',
      'localhost',
      '10.0.0',
      '192.168',
    ]) {
      expect(url.toLowerCase()).not.toContain(forbidden.toLowerCase())
    }
  })

  it('percent-encodes the platform summary rather than emitting a broken URL', () => {
    const url = buildIssueUrl(report({ webviewVersion: 'a b&c=d' }))
    expect(url).toContain('WebView+a+b%26c%3Dd')
    expect(new URL(url).searchParams.get('platform')).toBe('linux x86_64 · WebView a b&c=d')
  })
})

describe('openIssue', () => {
  it('degrades to "unsupported" without touching the plugin when no shell is present', async () => {
    expect(BRIDGE in window).toBe(false)

    await expect(openIssue(report())).resolves.toBe('unsupported')
    expect(openUrlMock).not.toHaveBeenCalled()
  })

  it('opens the prefilled url inside the shell', async () => {
    withBridge()
    openUrlMock.mockResolvedValue(undefined)

    await expect(openIssue(report())).resolves.toBe('opened')
    expect(openUrlMock).toHaveBeenCalledExactlyOnceWith(buildIssueUrl(report()))
  })

  it('reports "failed" instead of rejecting when the OS refuses', async () => {
    withBridge()
    openUrlMock.mockRejectedValue(new Error('opener.open_url not allowed'))

    await expect(openIssue(report())).resolves.toBe('failed')
  })
})
