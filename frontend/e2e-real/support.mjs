/**
 * Shared helpers for the F3 real-driver specs.
 *
 * `invoke` deliberately does NOT use `browser.executeAsync` (removed in WebdriverIO 9).
 * Instead it starts the real IPC call, parks the settled result on `window`, then polls with
 * `waitUntil` — a pattern that works on any wdio version and keeps the *real* Tauri IPC
 * bridge in the loop rather than a mock.
 */
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { browser } from '@wdio/globals'

const HERE = path.dirname(fileURLToPath(import.meta.url))
// Mirrors `wdio.conf.mjs`: `AGENTLENS_WDIO_EVIDENCE` redirects a re-run's screenshots so the
// F3 reviewer's 38 shots are never overwritten.
export const SHOTS = path.join(
  process.env.AGENTLENS_WDIO_EVIDENCE ??
    path.resolve(HERE, '../../.omo/evidence/f3-agentlens-usage-dashboard'),
  'screenshots',
)
fs.mkdirSync(SHOTS, { recursive: true })

let shotIndex = 0

/** Save a screenshot into the evidence directory and return its path. */
export async function shot(name) {
  shotIndex += 1
  const file = path.join(SHOTS, `${String(shotIndex).padStart(2, '0')}-${name}.png`)
  await browser.saveScreenshot(file)
  const bytes = fs.statSync(file).size
  console.log(`[shot] ${path.basename(file)} (${bytes} bytes)`)
  return file
}

/** Call a real Tauri command over the real IPC bridge and return its resolved value. */
export async function invoke(command, args = {}) {
  const token = `ipc_${Date.now()}_${Math.random().toString(36).slice(2)}`
  await browser.execute(
    (cmd, payload, key) => {
      const store = (window.__F3__ = window.__F3__ || {})
      store[key] = { done: false }
      window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
        (value) => {
          store[key] = { done: true, ok: true, value }
        },
        (error) => {
          store[key] = {
            done: true,
            ok: false,
            error: typeof error === 'object' ? error : String(error),
          }
        },
      )
    },
    command,
    args,
    token,
  )
  await browser.waitUntil(async () => await browser.execute((k) => !!window.__F3__?.[k]?.done, token), {
    timeout: 240_000,
    interval: 250,
    timeoutMsg: `IPC ${command} never settled`,
  })
  const settled = await browser.execute((k) => window.__F3__[k], token)
  if (!settled.ok) {
    throw new Error(`IPC ${command} failed: ${JSON.stringify(settled.error)}`)
  }
  return settled.value
}

/** The app's own report timezone, read from the real archive. */
export async function reportTimezone() {
  const settings = await invoke('get_settings', {})
  return settings.values['report.timezone'] ?? 'UTC'
}

/** Build a DateRange payload the way the frontend does. */
export function dateRange(startDate, endDateExclusive, weekStart = 'monday') {
  return { startDate, endDateExclusive, weekStart }
}

export const NO_FILTERS = {
  hostId: null,
  source: null,
  agentKey: null,
  providerId: null,
  modelId: null,
}
