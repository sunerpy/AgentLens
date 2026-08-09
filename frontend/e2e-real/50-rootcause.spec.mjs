/**
 * F3 defect root-cause: is the overlap structural (a clamped grid column) or an artifact of a
 * font that failed to load inside WebKit?
 */
import { browser, $ } from '@wdio/globals'

import { shot } from './support.mjs'

describe('F3 defect root cause', () => {
  it('reports font state, computed grid template and required vs available width', async () => {
    await $('[data-testid="overview-summary"]').waitForExist({ timeout: 60_000 })
    await browser.pause(2500)

    const report = await browser.execute(() => {
      const value = document.querySelector('[data-testid="summary-token-input"]')
      const cell = value.parentElement
      const grid = cell.parentElement
      const gridStyle = getComputedStyle(grid)
      const valueStyle = getComputedStyle(value)

      // Measure the text with the SAME font via canvas, so "needed width" is font-accurate.
      const canvas = document.createElement('canvas')
      const ctx = canvas.getContext('2d')
      ctx.font = `${valueStyle.fontWeight} ${valueStyle.fontSize} ${valueStyle.fontFamily}`
      const needed = Math.ceil(ctx.measureText(value.textContent).width)

      return {
        text: value.textContent,
        fontsStatus: document.fonts ? document.fonts.status : 'n/a',
        fontsLoaded: document.fonts ? document.fonts.check(`24px ${valueStyle.fontFamily}`) : null,
        fontFamily: valueStyle.fontFamily,
        fontSize: valueStyle.fontSize,
        fontWeight: valueStyle.fontWeight,
        gridTemplateColumns: gridStyle.gridTemplateColumns,
        gridDisplay: gridStyle.display,
        cellWidth: Math.round(cell.getBoundingClientRect().width),
        cellMinWidth: getComputedStyle(cell).minWidth,
        valueOverflow: valueStyle.overflow,
        valueTextOverflow: valueStyle.textOverflow,
        valueWhiteSpace: valueStyle.whiteSpace,
        neededPx: needed,
      }
    })
    console.log('[root] ' + JSON.stringify(report, null, 2))
    await shot('22-rootcause-overview')

    // Element-scoped screenshot of just the Token card, so the overlap is unambiguous.
    const grid = await $('[data-testid="overview-summary"]')
    await grid.saveScreenshot(
      '/config/workspace/ProdDir/AI/AgentLens/.omo/evidence/f3-agentlens-usage-dashboard/screenshots/23-defect-summary-cards-crop.png',
    )
    console.log('[root] saved cropped summary-cards screenshot')
  })
})
