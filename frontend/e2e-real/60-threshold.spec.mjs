/**
 * F3 defect threshold: at what magnitude does the metric value start to spill, and at what
 * magnitude does it actually collide with the neighbouring metric?
 *
 * Measured by temporarily rewriting only the text nodes of the already-rendered metric spans
 * (no app state is touched, and the session is discarded afterwards), so the geometry comes
 * from the app's real CSS at the app's real window size.
 */
import { browser, $, expect } from '@wdio/globals'

describe('F3 defect threshold', () => {
  it('finds the digit count where spill starts and where collision starts', async () => {
    await $('[data-testid="overview-summary"]').waitForExist({ timeout: 60_000 })
    await browser.pause(2000)

    const rows = await browser.execute(() => {
      const input = document.querySelector('[data-testid="summary-token-input"]')
      const output = document.querySelector('[data-testid="summary-token-output"]')
      const original = input.textContent
      const fmt = new Intl.NumberFormat('en-US')
      const ink = (el) => {
        const r = document.createRange()
        r.selectNodeContents(el)
        return r.getBoundingClientRect()
      }
      const out = []
      for (let digits = 4; digits <= 13; digits += 1) {
        const value = Number('9'.repeat(digits))
        input.textContent = fmt.format(value)
        const a = ink(input)
        const b = ink(output)
        const cell = input.parentElement.getBoundingClientRect()
        out.push({
          digits,
          text: input.textContent,
          chars: input.textContent.length,
          inkW: Math.round(a.width),
          cellW: Math.round(cell.width),
          spill: Math.round(a.right - cell.right),
          gapToNeighbour: Math.round(b.left - a.right),
        })
      }
      input.textContent = original
      return out
    })

    for (const r of rows) {
      console.log(
        `[thresh] digits=${String(r.digits).padStart(2)} text=${r.text.padEnd(17)} chars=${r.chars} ` +
          `inkW=${r.inkW}px cellW=${r.cellW}px spill=${String(r.spill).padStart(4)}px ` +
          `gapToNeighbour=${String(r.gapToNeighbour).padStart(4)}px ` +
          `${r.gapToNeighbour < 0 ? '*** COLLIDES ***' : r.spill > 0 ? '(spills, no collision)' : 'ok'}`,
      )
    }

    // A `Range` rect reports the text's LAYOUT extent and ignores clipping, so the spills above
    // describe what plain grouped formatting would need, not what the metric actually paints.
    //
    // Permanent guard against the regression that made this fix un-shippable once: WebDriver
    // defines getText() as RENDERED text and excludes whatever `overflow: hidden` clips away, so
    // putting a clipping class on a `data-testid` element returns "" to every spec while the
    // pixels still look right. Every metric the specs read must therefore stay unclipped, and
    // getText() must return the compact literal.
    const COMPACT = /^\d+(\.\d+)?[KMBT]?$/
    for (const id of [
      'summary-token-input',
      'summary-token-output',
      'summary-token-reasoning',
      'summary-token-cache',
    ]) {
      const el = await $(`[data-testid="${id}"]`)
      const rendered = await el.getText()
      const dom = await browser.execute((testid) => {
        const node = document.querySelector(`[data-testid="${testid}"]`)
        const style = getComputedStyle(node)
        return {
          textContent: node.textContent,
          overflowX: style.overflowX,
          overflowY: style.overflowY,
          cellMinWidth: getComputedStyle(node.parentElement).minWidth,
        }
      }, id)
      console.log(
        `[guard] ${id} getText()=${JSON.stringify(rendered)} textContent=${JSON.stringify(dom.textContent)} ` +
          `overflow=${dom.overflowX}/${dom.overflowY} cellMinWidth=${dom.cellMinWidth}`,
      )
      // Non-empty is the load-bearing half: a clipped element reads as "" here.
      expect(rendered.length).toBeGreaterThan(0)
      expect(rendered).toEqual(dom.textContent)
      expect(rendered).toMatch(COMPACT)
      expect(dom.overflowX).toEqual('visible')
      expect(dom.overflowY).toEqual('visible')
      // The wrapper still guards the grid from a blowout; that class is safe because the spec
      // never reads the wrapper.
      expect(dom.cellMinWidth).toEqual('0px')
    }
  })
})
