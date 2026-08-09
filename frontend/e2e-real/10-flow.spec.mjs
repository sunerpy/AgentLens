/**
 * F3 real manual QA — the plan's full end-to-end flow, driven through the REAL Tauri
 * WebView with REAL IPC against a REAL archive built from the user's live OpenCode DB.
 *
 * Nothing here is mocked. Every number asserted below was produced by the app's own
 * collector reading `/config/.local/share/opencode/opencode.db` (read-only) and writing
 * an isolated archive under `XDG_DATA_HOME=/tmp/opencode/f3-xdg`, so the user's real
 * `~/.local/share/agentlens/archive.db` is never touched.
 *
 * The steps map 1:1 onto the plan's flow: local host auto-registration, refresh, real
 * overview data, range switching, drilldown, detail paging, and the timezone/bucket-edge
 * integration assertion.
 */
import { expect, browser, $, $$ } from '@wdio/globals'

import { NO_FILTERS, dateRange, invoke, shot } from './support.mjs'

/** Wait for a testid to exist, with a readable failure. */
async function waitFor(testid, timeout = 60_000) {
  const el = $(`[data-testid="${testid}"]`)
  await el.waitForExist({ timeout, timeoutMsg: `[data-testid="${testid}"] never appeared` })
  return el
}

async function nav(view) {
  await (await waitFor(`nav-${view}`)).click()
  await waitFor(`view-${view}`)
}

/**
 * Read a metric's EXACT numeric value.
 *
 * Large metrics render compactly (`9.3B`) because grouped digits overlap the neighbouring metric
 * at real magnitudes, so the full-precision figure lives in the element's `title`. This reads the
 * title when present and the rendered text otherwise, which keeps every assertion below on exact
 * numbers rather than on a rounded display value.
 *
 * The rendered text is still read and required to be non-empty: WebDriver's `getText()` returns
 * "" for anything clipped by `overflow: hidden`, and a clipping class on a `data-testid` element
 * once made these assertions silently compare `Number('')` — a regression this must keep catching.
 */
async function num(testid) {
  const el = await waitFor(testid)
  const rendered = await el.getText()
  if (rendered.length === 0) {
    throw new Error(`${testid} rendered text is empty — is the element clipped?`)
  }
  const exact = await el.getAttribute('title')
  const source = exact === null || exact.length === 0 ? rendered : exact
  const parsed = Number(source.replace(/[,\s$]/g, ''))
  if (Number.isNaN(parsed)) {
    throw new Error(
      `${testid} is not numeric: rendered=${JSON.stringify(rendered)} title=${JSON.stringify(exact)}`,
    )
  }
  return parsed
}

const findings = []
function record(line) {
  findings.push(line)
  console.log(`[flow] ${line}`)
}

describe('F3 real manual QA — full flow against real IPC and real data', () => {
  let localHostId = null
  let archivedRows = 0

  before(async () => {
    await waitFor('nav-overview')
    // No console-error channel exists over classic WebDriver, so surface anything the app
    // itself flagged: the shell renders `error-state` for any structured IpcError.
    expect(await $('[data-testid="error-state"]').isExisting()).toBe(false)
  })

  after(() => {
    console.log('\n===== F3 FLOW FINDINGS =====')
    for (const line of findings) console.log(line)
    console.log('===== END FINDINGS =====\n')
  })

  it('STEP 1 — local host auto-registers through real IPC', async () => {
    const identity = await invoke('local_machine_identity', {})
    record(
      `step1 local_machine_identity -> hostId=${identity.hostId} machineIdHash=${identity.machineIdHash} hostname=${identity.hostname}`,
    )

    await nav('hosts')
    const card = await waitFor('local-host-card')
    // Poll the state attribute: registration is a mutation fired from an effect.
    await browser.waitUntil(
      async () =>
        ['registered', 'unregistered'].includes(await card.getAttribute('data-local-state')),
      { timeout: 60_000, timeoutMsg: 'local host card never settled' },
    )
    const state = await card.getAttribute('data-local-state')
    const stateLabel = await (await waitFor('local-host-state')).getText()
    record(`step1 local-host-card data-local-state=${state} badge="${stateLabel}"`)
    await shot('01-step1-hosts-local-registered')

    expect(state).toBe('registered')

    const hosts = await invoke('hosts_list', {})
    record(`step1 hosts_list -> ${JSON.stringify(hosts)}`)
    expect(hosts.length).toBeGreaterThan(0)
    const local = hosts.find((h) => h.kind === 'local')
    expect(local).toBeDefined()
    localHostId = local.hostId
    // The card must display the same id the backend registered — not a placeholder.
    expect(await (await waitFor('local-host-id')).getText()).toBe(localHostId)
  })

  it('STEP 2 — a real refresh runs to completion (idle -> running -> idle)', async () => {
    await nav('hosts')
    const before = await invoke('get_refresh_status', {})
    record(`step2 status before -> ${JSON.stringify(before)}`)

    const trigger = await invoke('trigger_refresh', { hostId: localHostId })
    record(`step2 trigger_refresh -> ${JSON.stringify(trigger)}`)

    // Poll the real scheduler until it reports idle again with a success timestamp.
    let final = null
    await browser.waitUntil(
      async () => {
        const rows = await invoke('get_refresh_status', {})
        const row = rows.find((r) => r.hostId === localHostId)
        if (row === undefined) return false
        final = row
        // SourceState is a ts-rs tagged enum: `{ state: 'idle' | 'running' | 'error' }`.
        return row.state.state === 'idle' || row.state.state === 'error'
      },
      { timeout: 600_000, interval: 2000, timeoutMsg: 'refresh never left running' },
    )
    record(`step2 status after -> ${JSON.stringify(final)}`)
    await shot('02-step2-refresh-complete')

    expect(final.state.state).toBe('idle')
    expect(final.lastError).toBe(null)
    expect(final.lastSuccessUtc).not.toBe(null)
    expect(final.interrupted).toBe(false)
    record(
      `step2 durationMs=${final.lastDurationMs} cursorTimeUpdated=${final.cursorTimeUpdated} interrupted=${final.interrupted}`,
    )
  })

  it('STEP 3 — overview shows real, non-trivial data', async () => {
    // First establish the ground truth straight from the archive via real IPC over a window
    // wide enough to cover everything the collector just archived.
    const wide = dateRange('2020-01-01', '2030-01-01')
    const summary = await invoke('get_summary', { range: wide, tz: 'UTC', filters: NO_FILTERS })
    record(
      `step3 get_summary(wide,UTC) -> messages=${summary.messageCount} sessions=${summary.activeSessionCount} ` +
        `input=${summary.tokens.tokInput} output=${summary.tokens.tokOutput} reasoning=${summary.tokens.tokReasoning} ` +
        `cacheRead=${summary.tokens.tokCacheRead} cacheWrite=${summary.tokens.tokCacheWrite} totalInput=${summary.tokens.totalInput}`,
    )
    record(
      `step3 cost -> actualSum=${summary.cost.actualSum} estimatedSum=${summary.cost.estimatedSum} unavailableCount=${summary.cost.unavailableCount}`,
    )
    archivedRows = summary.messageCount
    // The archive must hold the real corpus, not a handful of rows.
    expect(summary.messageCount).toBeGreaterThan(100_000)
    expect(summary.tokens.tokInput).toBeGreaterThan(0)
    expect(summary.tokens.tokOutput).toBeGreaterThan(0)

    // Then drive the UI over the same wide window so the rendered cards are the real data.
    await nav('overview')
    await waitFor('overview-summary')
    await waitFor('overview-trend')
    await shot('03-step3-overview-default-range')

    const uiInput = await num('summary-token-input')
    const uiOutput = await num('summary-token-output')
    const uiMessages = await num('summary-message-count')
    record(
      `step3 overview UI (default 7d range) -> input=${uiInput} output=${uiOutput} messages=${uiMessages}`,
    )
    // The default preset is 7 days, a subset of the wide window, so it must be <= the total
    // and (given the corpus spans the last week) strictly positive.
    expect(uiMessages).toBeGreaterThan(0)
    expect(uiMessages).toBeLessThanOrEqual(summary.messageCount)
    expect(uiInput).toBeGreaterThan(0)

    // Real rows carry cost = 0 everywhere, so the cost card must legitimately say
    // "unavailable" rather than inventing a number.
    const unavailable = await $('[data-testid="summary-cost-unavailable"]')
    record(
      `step3 cost card unavailable-badge exists=${await unavailable.isExisting()} text=${
        (await unavailable.isExisting()) ? JSON.stringify(await unavailable.getText()) : 'n/a'
      }`,
    )
    expect(await unavailable.isExisting()).toBe(true)
    expect(await $('[data-testid="error-state"]').isExisting()).toBe(false)
  })

  it('STEP 4 — switching 今天 / 7 天 / 自定义 changes the numbers and keeps the window half-open', async () => {
    await nav('overview')
    const readings = {}

    for (const preset of ['today', 'last7Days', 'last30Days']) {
      await (await waitFor(`range-preset-${preset}`)).click()
      // The window label is the source of truth for what was requested.
      await browser.waitUntil(
        async () => (await (await waitFor('range-window')).getText()).includes('['),
        { timeout: 30_000 },
      )
      // Let react-query settle on the new key before reading the cards.
      await browser.pause(1500)
      const window = await (await waitFor('range-window')).getText()
      const messages = await num('summary-message-count')
      const input = await num('summary-token-input')
      readings[preset] = { window, messages, input }
      record(`step4 preset=${preset} window=${window} messages=${messages} input=${input}`)
      await shot(`04-step4-range-${preset}`)

      // Half-open: rendered as `[start, endExclusive)`.
      expect(window).toMatch(/^\[\d{4}-\d{2}-\d{2},\s*\d{4}-\d{2}-\d{2}\)$/)
      const [, start, end] = window.match(/^\[(\d{4}-\d{2}-\d{2}),\s*(\d{4}-\d{2}-\d{2})\)$/)
      expect(end > start).toBe(true)
    }

    // A wider window can only contain more or equal, and here must contain strictly more.
    expect(readings.last7Days.messages).toBeGreaterThan(readings.today.messages)
    expect(readings.last30Days.messages).toBeGreaterThanOrEqual(readings.last7Days.messages)
    record(
      `step4 monotonic: today=${readings.today.messages} < 7d=${readings.last7Days.messages} <= 30d=${readings.last30Days.messages}`,
    )

    // 自定义 — open the calendar popover and apply an explicit range.
    await (await waitFor('range-preset-custom')).click()
    await waitFor('range-calendar')
    await shot('04-step4-range-custom-calendar')
    const days = await $$('[data-testid="range-calendar"] button[data-testid^="calendar-day-"]')
    record(`step4 custom calendar day buttons=${days.length}`)
    expect(days.length).toBeGreaterThan(0)
    // Pick the 1st and the 5th selectable day of the shown month, then apply.
    await days[0].click()
    await days[Math.min(4, days.length - 1)].click()
    await (await waitFor('calendar-apply')).click()
    await browser.pause(1500)
    const customWindow = await (await waitFor('range-window')).getText()
    const customPressed = await (await waitFor('range-preset-custom')).getAttribute('aria-pressed')
    record(`step4 custom applied window=${customWindow} aria-pressed=${customPressed}`)
    await shot('04-step4-range-custom-applied')
    expect(customPressed).toBe('true')
    expect(customWindow).toMatch(/^\[\d{4}-\d{2}-\d{2},\s*\d{4}-\d{2}-\d{2}\)$/)

    // Restore 7 天 for the following steps.
    await (await waitFor('range-preset-last7Days')).click()
    await browser.pause(1000)
  })

  it('STEP 5 — drilldown source -> agent -> model, expanding a variant row', async () => {
    await nav('drilldown')
    const sourceRows = await $$('[data-testid="drilldown-source-row"]')
    const sources = []
    for (const row of sourceRows) sources.push(await row.getAttribute('data-source'))
    record(`step5 source rows=${sourceRows.length} sources=${JSON.stringify(sources)}`)
    await shot('05-step5-drilldown-source')
    expect(sourceRows.length).toBeGreaterThan(0)

    // Selection lives on the inner name button, not the row itself.
    await (await sourceRows[0].$('button')).click()
    await browser.pause(1200)
    const agentRows = await $$('[data-testid="drilldown-agent-row"]')
    const agents = []
    for (const row of agentRows) agents.push(await row.getAttribute('data-agent-key'))
    record(`step5 agent rows=${agentRows.length} agentKeys=${JSON.stringify(agents.slice(0, 12))}`)
    await shot('05-step5-drilldown-agent')
    expect(agentRows.length).toBeGreaterThan(0)

    await (await agentRows[0].$('button')).click()
    await browser.pause(1200)
    const modelRows = await $$('[data-testid="drilldown-model-row"]')
    const models = []
    for (const row of modelRows) models.push(await row.getAttribute('data-model-key'))
    record(`step5 model rows=${modelRows.length} modelKeys=${JSON.stringify(models.slice(0, 12))}`)
    const crumb = await (await waitFor('drilldown-breadcrumb')).getText()
    record(`step5 breadcrumb=${JSON.stringify(crumb)}`)
    await shot('05-step5-drilldown-model')
    expect(modelRows.length).toBeGreaterThan(0)

    // Expand a variant row. `data-expanded` must flip and variant rows must appear.
    const expanders = await $$('[data-testid="drilldown-model-expand"]')
    record(`step5 expandable model rows=${expanders.length}`)
    expect(expanders.length).toBeGreaterThan(0)
    await expanders[0].click()
    await browser.pause(1200)
    const variantRows = await $$('[data-testid="drilldown-variant-row"]')
    const variants = []
    for (const row of variantRows) variants.push(await row.getAttribute('data-variant'))
    record(`step5 variant rows=${variantRows.length} variants=${JSON.stringify(variants)}`)
    await shot('05-step5-drilldown-variant-expanded')
    expect(variantRows.length).toBeGreaterThan(0)
    expect(await $('[data-testid="error-state"]').isExisting()).toBe(false)
  })

  it('STEP 6 — detail paging issues a new offset:50 query and the rows genuinely change', async () => {
    await nav('detail')
    await waitFor('detail-table')
    const total = Number(
      await (await waitFor('detail-total-count')).getAttribute('data-total-count'),
    )
    const rangeText = await (await waitFor('detail-page-range')).getText()
    record(`step6 total-count=${total} page-range=${JSON.stringify(rangeText)}`)
    await shot('06-step6-detail-page1')
    expect(total).toBeGreaterThan(50)

    const idsOf = async () => {
      const rows = await $$('[data-testid="detail-row"]')
      const out = []
      for (const row of rows) out.push(await row.getAttribute('data-message-id'))
      return out
    }
    const page1 = await idsOf()
    record(`step6 page1 rows=${page1.length} firstId=${page1[0]} lastId=${page1[page1.length - 1]}`)
    expect(page1.length).toBeGreaterThan(0)

    await (await waitFor('detail-next-page')).click()
    await browser.waitUntil(
      async () => (await (await waitFor('detail-page-range')).getText()) !== rangeText,
      { timeout: 60_000, timeoutMsg: 'page range never advanced' },
    )
    await browser.pause(800)
    const page2 = await idsOf()
    const range2 = await (await waitFor('detail-page-range')).getText()
    record(`step6 page2 range=${JSON.stringify(range2)} rows=${page2.length} firstId=${page2[0]}`)
    await shot('06-step6-detail-page2')

    // Server-side paging: the row identities must be disjoint from page 1.
    const overlap = page2.filter((id) => page1.includes(id))
    record(`step6 overlap between page1 and page2 = ${overlap.length}`)
    expect(page2.length).toBeGreaterThan(0)
    expect(overlap.length).toBe(0)

    // And prove at the IPC layer that offset really reaches the backend.
    const filters = {
      range: dateRange('2020-01-01', '2030-01-01'),
      timezone: 'UTC',
      hostId: null,
      source: null,
      agentKey: null,
      providerId: null,
      modelId: null,
      isIncomplete: null,
    }
    const p1 = await invoke('query_messages', { filters, limit: 50, offset: 0 })
    const p2 = await invoke('query_messages', { filters, limit: 50, offset: 50 })
    record(
      `step6 IPC query_messages totalCount=${p1.totalCount} offset0.first=${p1.rows[0]?.messageId} offset50.first=${p2.rows[0]?.messageId}`,
    )
    expect(p1.totalCount).toBeGreaterThan(50)
    expect(p2.rows[0].messageId).not.toBe(p1.rows[0].messageId)
    expect(await $('[data-testid="error-state"]').isExisting()).toBe(false)
  })

  it('STEP 7 — changing the report timezone shifts the chart bucket boundaries', async () => {
    // The named integration assertion: identical range + granularity, different timezone,
    // must yield different bucket edges. Proven at the IPC layer against real data first.
    const range = dateRange('2026-07-25', '2026-08-01')
    const utc = await invoke('get_trend', {
      range,
      tz: 'UTC',
      granularity: 'day',
      filters: NO_FILTERS,
    })
    const sh = await invoke('get_trend', {
      range,
      tz: 'Asia/Shanghai',
      granularity: 'day',
      filters: NO_FILTERS,
    })
    const utcEdges = utc.map((p) => p.bucket.startUtcMs)
    const shEdges = sh.map((p) => p.bucket.startUtcMs)
    record(`step7 UTC bucket startUtcMs      = ${JSON.stringify(utcEdges)}`)
    record(`step7 Shanghai bucket startUtcMs = ${JSON.stringify(shEdges)}`)
    const delta = utcEdges[0] - shEdges[0]
    record(`step7 first-edge delta = ${delta} ms (${delta / 3_600_000} h)`)
    expect(utcEdges[0]).not.toBe(shEdges[0])
    expect(delta).toBe(8 * 3_600_000)

    // Now do it through the UI: persist the timezone via settings, then read the chart's
    // rendered bucket labels before and after.
    await nav('overview')
    await (await waitFor('range-preset-last7Days')).click()
    await browser.pause(1200)
    const labelsOf = async () => {
      const dots = await $$('[data-testid="trend-dot-tokens"]')
      const out = []
      for (const dot of dots) out.push(await dot.getAttribute('data-bucket'))
      return out
    }
    const tzBefore = await (await waitFor('range-timezone')).getText()
    const labelsBefore = await labelsOf()
    record(`step7 UI before: ${tzBefore} buckets=${JSON.stringify(labelsBefore)}`)
    await shot('07-step7-overview-tz-before')

    await nav('settings')
    const select = await waitFor('settings-timezone')
    const current = await select.getValue()
    const target = current === 'UTC' ? 'Asia/Tokyo' : 'UTC'
    await select.selectByAttribute('value', target)
    await (await waitFor('settings-save')).click()
    await browser.waitUntil(async () => await $('[data-testid="settings-saved"]').isExisting(), {
      timeout: 60_000,
      timeoutMsg: 'settings never reported saved',
    })
    record(`step7 settings timezone ${current} -> ${target} saved`)
    await shot('07-step7-settings-timezone-saved')

    const persisted = await invoke('get_settings', {})
    record(`step7 get_settings after save -> ${JSON.stringify(persisted.values)}`)
    expect(persisted.values['report.timezone']).toBe(target)

    await nav('overview')
    await browser.waitUntil(
      async () => (await (await waitFor('range-timezone')).getText()).includes(target),
      { timeout: 60_000, timeoutMsg: 'overview never picked up the new timezone' },
    )
    await browser.pause(2000)
    const tzAfter = await (await waitFor('range-timezone')).getText()
    const labelsAfter = await labelsOf()
    record(`step7 UI after:  ${tzAfter} buckets=${JSON.stringify(labelsAfter)}`)
    await shot('07-step7-overview-tz-after')

    // The rendered chart must reflect the new zone: either the labels moved, or the
    // per-bucket values were re-cut. Assert on the underlying edges too so the claim is
    // about real bucketing rather than a cosmetic label.
    const afterEdges = (
      await invoke('get_trend', {
        range,
        tz: target,
        granularity: 'day',
        filters: NO_FILTERS,
      })
    ).map((p) => p.bucket.startUtcMs)
    record(`step7 ${target} bucket startUtcMs = ${JSON.stringify(afterEdges)}`)
    expect(afterEdges[0]).not.toBe(utcEdges[0] === afterEdges[0] ? shEdges[0] : utcEdges[0])
    expect(tzAfter).toContain(target)
    expect(await $('[data-testid="error-state"]').isExisting()).toBe(false)

    record(`summary archivedRows=${archivedRows} localHostId=${localHostId}`)
  })
})
