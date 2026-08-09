/**
 * F3 tier-1 smoke: prove a REAL WebDriver session against the REAL Tauri webview, and that
 * the REAL IPC bridge (`window.__TAURI_INTERNALS__.invoke`) answers.
 */
import { expect, browser, $ } from '@wdio/globals'

import { invoke, shot } from './support.mjs'

describe('F3 tier-1 smoke — real driver session', () => {
  it('attaches to the real webview and renders the app shell', async () => {
    await browser.waitUntil(async () => (await $('[data-testid="nav-overview"]').isExisting()), {
      timeout: 60_000,
      timeoutMsg: 'app shell never rendered — is vite dev on :1420 up?',
    })
    const title = await browser.getTitle()
    console.log('[smoke] document.title =', JSON.stringify(title))
    const ua = await browser.execute(() => navigator.userAgent)
    console.log('[smoke] userAgent =', ua)
    const hasTauri = await browser.execute(
      () => typeof window.__TAURI_INTERNALS__?.invoke === 'function',
    )
    console.log('[smoke] __TAURI_INTERNALS__.invoke present =', hasTauri)
    expect(hasTauri).toBe(true)
    await shot('00-smoke-shell')
  })

  it('answers a real IPC call with real archive state', async () => {
    const settings = await invoke('get_settings', {})
    console.log('[smoke] get_settings ->', JSON.stringify(settings))
    expect(settings).toHaveProperty('values')
    const status = await invoke('get_refresh_status', {})
    console.log('[smoke] get_refresh_status ->', JSON.stringify(status))
  })
})
