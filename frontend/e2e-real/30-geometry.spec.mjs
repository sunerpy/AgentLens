/**
 * F3 defect measurement — geometry of the overview summary metrics with REAL data.
 *
 * Motivation: the real corpus produces 10-13 digit token counts (e.g. 26,445,784,256),
 * whereas the mocked-IPC baseline in `artifacts/qa/` only ever rendered 6-digit values.
 * This spec measures the rendered boxes so any overflow claim is a number, not an eyeball.
 */
import { browser, $, expect } from '@wdio/globals'

import { shot } from './support.mjs'

async function box(testid) {
  return browser.execute((id) => {
    const el = document.querySelector(`[data-testid="${id}"]`)
    if (el === null) return null
    const r = el.getBoundingClientRect()
    const parent = el.parentElement.getBoundingClientRect()
    // The element BOX is clamped by the grid cell, but glyphs can still paint outside it
    // when nothing truncates or wraps. Measure the actual painted text via a Range, which
    // is the only way to see the real ink extent.
    const range = document.createRange()
    range.selectNodeContents(el)
    const ink = range.getBoundingClientRect()
    return {
      inkLeft: Math.round(ink.left),
      inkRight: Math.round(ink.right),
      inkWidth: Math.round(ink.width),
      text: el.textContent,
      left: Math.round(r.left),
      right: Math.round(r.right),
      width: Math.round(r.width),
      scrollWidth: el.scrollWidth,
      clientWidth: el.clientWidth,
      cellLeft: Math.round(parent.left),
      cellRight: Math.round(parent.right),
      cellWidth: Math.round(parent.width),
    }
  }, testid)
}

describe('F3 defect measurement — real-data metric geometry', () => {
  it('measures whether the token metrics overflow their grid cells', async () => {
    await $('[data-testid="overview-summary"]').waitForExist({ timeout: 60_000 })
    await browser.pause(2500)
    const ids = [
      'summary-token-input',
      'summary-token-output',
      'summary-token-reasoning',
      'summary-token-cache',
      'summary-message-count',
      'summary-active-session-count',
    ]
    const boxes = {}
    for (const id of ids) boxes[id] = await box(id)
    for (const [id, b] of Object.entries(boxes)) {
      if (b === null) {
        console.log(`[geom] ${id} MISSING`)
        continue
      }
      const overflow = b.right - b.cellRight
      console.log(
        `[geom] ${id} text=${JSON.stringify(b.text)} box=[${b.left},${b.right}] w=${b.width} ` +
          `cell=[${b.cellLeft},${b.cellRight}] cellW=${b.cellWidth} overflowPx=${overflow} ` +
          `scrollW=${b.scrollWidth} clientW=${b.clientWidth} ` +
          `INK=[${b.inkLeft},${b.inkRight}] inkW=${b.inkWidth} inkOverflowPx=${b.inkRight - b.cellRight}`,
      )
    }
    // Do neighbouring metric values physically overlap?
    const pairs = [
      ['summary-token-input', 'summary-token-output'],
      ['summary-token-output', 'summary-token-reasoning'],
      ['summary-token-reasoning', 'summary-token-cache'],
    ]
    const inkGaps = []
    for (const [a, b] of pairs) {
      const left = boxes[a]
      const right = boxes[b]
      if (left === null || right === null) continue
      const gap = right.left - left.right
      const inkGap = right.inkLeft - left.inkRight
      inkGaps.push([`${a} -> ${b}`, inkGap])
      console.log(
        `[geom] gap ${a} -> ${b}: boxGap=${gap}px inkGap=${inkGap}px ${inkGap < 0 ? '*** INK OVERLAP ***' : ''}`,
      )
    }
    await shot('19-defect-summary-geometry')

    // The readability contract, asserted rather than eyeballed: painted glyphs must stay
    // inside their grid cell and must not touch the neighbouring metric. Ink extent is what
    // matters — the element box is clamped by the grid, so a box-only check passes even while
    // the digits are printing on top of each other.
    for (const [id, b] of Object.entries(boxes)) {
      if (b === null) continue
      expect(b.inkRight - b.cellRight).toBeLessThanOrEqual(0)
    }
    for (const [label, inkGap] of inkGaps) {
      console.log(`[geom] assert inkGap ${label} = ${inkGap}px`)
      expect(inkGap).toBeGreaterThan(0)
    }
  })
})
