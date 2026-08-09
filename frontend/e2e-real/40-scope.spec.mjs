/**
 * F3 defect scope — how wide the window must be before the real-data numbers stop
 * overlapping, and whether other views show the same class of overflow.
 */
import { browser, $ } from '@wdio/globals'

import { shot } from './support.mjs'

async function inkOf(ids) {
  return browser.execute((list) => {
    const out = {}
    for (const id of list) {
      const el = document.querySelector(`[data-testid="${id}"]`)
      if (el === null) {
        out[id] = null
        continue
      }
      const range = document.createRange()
      range.selectNodeContents(el)
      const ink = range.getBoundingClientRect()
      const cell = el.parentElement.getBoundingClientRect()
      out[id] = {
        text: el.textContent,
        inkLeft: Math.round(ink.left),
        inkRight: Math.round(ink.right),
        cellRight: Math.round(cell.right),
        cellWidth: Math.round(cell.width),
      }
    }
    return out
  }, ids)
}

const TOKENS = [
  'summary-token-input',
  'summary-token-output',
  'summary-token-reasoning',
  'summary-token-cache',
]

describe('F3 defect scope', () => {
  it('reports the overlap at several window widths', async () => {
    await $('[data-testid="overview-summary"]').waitForExist({ timeout: 60_000 })
    for (const width of [1180, 1280, 1440, 1600, 1920]) {
      await browser.setWindowRect(0, 0, width, 900)
      await browser.pause(1200)
      const inner = await browser.execute(() => window.innerWidth)
      const boxes = await inkOf(TOKENS)
      const parts = []
      let worst = Infinity
      for (let i = 0; i < TOKENS.length - 1; i += 1) {
        const a = boxes[TOKENS[i]]
        const b = boxes[TOKENS[i + 1]]
        if (a === null || b === null) continue
        const gap = b.inkLeft - a.inkRight
        worst = Math.min(worst, gap)
        parts.push(`${TOKENS[i].replace('summary-token-', '')}->${TOKENS[i + 1].replace('summary-token-', '')}=${gap}px`)
      }
      console.log(
        `[scope] requested=${width} innerWidth=${inner} cellW=${boxes[TOKENS[0]]?.cellWidth} ` +
          `inkGaps: ${parts.join(' ')} worst=${worst}px ${worst < 0 ? '*** OVERLAP ***' : 'ok'}`,
      )
      console.log(
        `[scope]   values: ${TOKENS.map((id) => `${id.replace('summary-token-', '')}=${JSON.stringify(boxes[id]?.text)}`).join(' ')}`,
      )
      await shot(`20-scope-overview-${width}px`)
    }
  })

  it('checks the drilldown and detail tables for the same class of overflow', async () => {
    await browser.setWindowRect(0, 0, 1180, 900)
    await browser.pause(800)
    for (const view of ['drilldown', 'detail']) {
      await $(`[data-testid="nav-${view}"]`).click()
      await $(`[data-testid="view-${view}"]`).waitForExist({ timeout: 60_000 })
      await browser.pause(2500)
      const report = await browser.execute(() => {
        // Any element whose painted text is wider than its own content box is clipped or
        // spilling. Report the worst offenders with their text so they can be judged.
        const rows = []
        for (const el of document.querySelectorAll('td, th, span, dd')) {
          if (el.children.length > 0) continue
          const text = (el.textContent || '').trim()
          if (text === '') continue
          const range = document.createRange()
          range.selectNodeContents(el)
          const ink = range.getBoundingClientRect()
          const box = el.getBoundingClientRect()
          const spill = Math.round(ink.right - box.right)
          if (spill > 2) rows.push({ text, spill, boxW: Math.round(box.width) })
        }
        rows.sort((a, b) => b.spill - a.spill)
        return rows.slice(0, 10)
      })
      console.log(`[scope] ${view} worst text spills: ${JSON.stringify(report)}`)
      await shot(`21-scope-${view}-1180px`)
    }
  })
})
