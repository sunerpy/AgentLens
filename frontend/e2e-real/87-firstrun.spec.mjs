/**
 * F3 first-run staleness, corrected reproduction.
 *
 * Two findings shaped this spec:
 *  1. With an empty archive, no collection EVER happens until the user opens 主机 once —
 *     local-host auto-registration lives in the hosts view. That is documented behaviour
 *     (README: "本机会在首次打开「主机管理」时自动注册"), so the spec performs that visit.
 *  2. The order that exposed the defect in the full-suite run is: boot on 总览 with an empty
 *     archive (cached zeros) → visit 主机 (registers + collects) → return to 总览.
 *
 * Nothing in the app is patched. The spec asserts the fix for DEFECT-2: after the first round
 * commits, the overview must show the archive's real numbers with NO user interaction — no
 * range change, no navigation, no reload. Both halves of the check matter:
 *   - `> 0` catches the original symptom (a dashboard frozen at zero);
 *   - equality with a fresh `get_summary` over the window the UI itself displays catches the
 *     warm-archive form of the same bug, where a stale-but-plausible number hides it.
 */
import { browser, $, expect } from '@wdio/globals'

import { NO_FILTERS, dateRange, invoke, shot } from './support.mjs'

const WIDE = dateRange('2020-01-01', '2030-01-01')

/** Read the half-open window the overview is actually showing: `[start, endExclusive)`. */
async function shownWindow() {
  const text = await (await $('[data-testid="range-window"]')).getText()
  const match = /\[(\d{4}-\d{2}-\d{2}),\s*(\d{4}-\d{2}-\d{2})\)/.exec(text)
  if (match === null) throw new Error(`unparseable range window: ${text}`)
  return dateRange(match[1], match[2])
}

/**
 * The timezone the overview is actually reporting in, taken from the rendered chip rather than
 * from `get_settings`: with no persisted `report.timezone` the app resolves the system zone
 * while a settings read would fall back to UTC, and an 8h bucket shift makes the cross-check
 * compare two different windows.
 */
async function shownTimezone() {
  const text = await (await $('[data-testid="range-timezone"]')).getText()
  const zone = text.split(':').pop()?.trim()
  if (zone === undefined || zone.length === 0) throw new Error(`unparseable timezone: ${text}`)
  return zone
}

async function uiMessages() {
  const el = await $('[data-testid="summary-message-count"]')
  if (!(await el.isExisting())) return null
  return Number((await el.getText()).replace(/[,\s]/g, ''))
}

async function archiveMessages() {
  return (await invoke('get_summary', { range: WIDE, tz: 'UTC', filters: NO_FILTERS })).messageCount
}


describe('F3 first-run staleness (corrected)', () => {
  it('boot empty -> visit hosts -> return to overview: does the overview show the data?', async () => {
    await $('[data-testid="view-overview"]').waitForExist({ timeout: 60_000 })
    await browser.pause(3000)
    console.log(`[fr] T0 boot on 总览: UI=${await uiMessages()} archive=${await archiveMessages()}`)
    await shot('87-firstrun-boot-empty')

    // Visit 主机 — this is what triggers local-host auto-registration and the first round.
    await (await $('[data-testid="nav-hosts"]')).click()
    await $('[data-testid="view-hosts"]').waitForExist({ timeout: 30_000 })
    let settled = null
    await browser.waitUntil(
      async () => {
        const rows = await invoke('get_refresh_status', {})
        if (rows.length === 0) return false
        settled = rows[0]
        return settled.state.state !== 'running' && settled.lastSuccessUtc !== null
      },
      { timeout: 280_000, interval: 2000, timeoutMsg: 'first collection never committed' },
    )
    const archived = await archiveMessages()
    console.log(
      `[fr] first collection committed: durationMs=${settled.lastDurationMs} archive=${archived}`,
    )
    await shot('88-firstrun-hosts-after-collection')

    // Return to 总览. This remounts the view whose cached data is zeros.
    await (await $('[data-testid="nav-overview"]')).click()
    await $('[data-testid="view-overview"]').waitForExist({ timeout: 30_000 })
    let cumulative = 0
    for (const wait of [1000, 3000, 5000, 10_000, 20_000]) {
      await browser.pause(wait)
      cumulative += wait
      console.log(`[fr] on 总览 +${cumulative}ms: UI=${await uiMessages()} archive=${archived}`)
    }
    const finalUi = await uiMessages()
    const window = await shownWindow()
    const expectedUi = (
      await invoke('get_summary', {
        range: window,
        tz: await shownTimezone(),
        filters: NO_FILTERS,
      })
    ).messageCount
    console.log(
      `[fr] VERDICT return-to-overview: UI=${finalUi} archive=${archived} ` +
        `window=[${window.startDate}, ${window.endDateExclusive}) expected=${expectedUi} -> ${
          finalUi === 0 ? 'STALE ZEROS SHOWN' : 'shows data'
        }`,
    )
    await shot('89-firstrun-back-on-overview')

    // No interaction happened between the commit and this assertion, so a pass means the
    // committed round itself reached the dashboard.
    expect(archived).toBeGreaterThan(0)
    expect(finalUi).toBeGreaterThan(0)
    expect(finalUi).toEqual(expectedUi)

    // The old recovery path must still work, and must now be a no-op in terms of correctness.
    await (await $('[data-testid="range-preset-last30Days"]')).click()
    await browser.pause(6000)
    const wide = await uiMessages()
    console.log(`[fr] after range change to 30 天: UI=${wide}`)
    await shot('90-firstrun-after-range-change')
    expect(wide).toBeGreaterThanOrEqual(finalUi)
    await (await $('[data-testid="range-preset-last7Days"]')).click()
    await browser.pause(6000)
    const back = await uiMessages()
    console.log(`[fr] after switching back to 7 天: UI=${back}`)
    await shot('91-firstrun-back-to-7d')
    expect(back).toEqual(finalUi)
  })
})
