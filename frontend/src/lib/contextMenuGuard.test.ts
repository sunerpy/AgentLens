import { afterEach, describe, expect, it } from 'vitest'

import { installContextMenuGuard, shouldSuppressContextMenu } from './contextMenuGuard'

const teardowns: (() => void)[] = []

afterEach(() => {
  while (teardowns.length > 0) teardowns.pop()?.()
  document.body.innerHTML = ''
})

/** Install the guard and register its uninstaller so one case cannot leak into the next. */
function install(): void {
  teardowns.push(installContextMenuGuard(document))
}

/** Dispatch a real, cancelable `contextmenu` event and report whether it was cancelled. */
function rightClick(target: EventTarget): boolean {
  const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true })
  target.dispatchEvent(event)
  return event.defaultPrevented
}

describe('shouldSuppressContextMenu', () => {
  it('suppresses page content and non-element targets', () => {
    document.body.innerHTML = '<div id="page">总览</div>'
    const page = document.querySelector('#page')!

    expect(shouldSuppressContextMenu(page)).toBe(true)
    expect(shouldSuppressContextMenu(null)).toBe(true)
    // A text node is what a click on prose actually resolves to in some engines.
    expect(shouldSuppressContextMenu(document.createTextNode('归档库'))).toBe(true)
  })

  it('exempts editable fields and anything nested inside them', () => {
    document.body.innerHTML = `
      <input id="host" />
      <textarea id="note"></textarea>
      <div id="rich" contenteditable="true"><span id="inner">x</span></div>
      <div id="locked" contenteditable="false"><span id="locked-inner">x</span></div>
    `

    for (const id of ['host', 'note', 'rich', 'inner']) {
      expect(shouldSuppressContextMenu(document.querySelector(`#${id}`))).toBe(false)
    }
    // `contenteditable="false"` is an explicit opt-out, so it stays suppressed.
    expect(shouldSuppressContextMenu(document.querySelector('#locked'))).toBe(true)
    expect(shouldSuppressContextMenu(document.querySelector('#locked-inner'))).toBe(true)
  })
})

describe('installContextMenuGuard', () => {
  it('cancels the menu on page content but lets an input keep its native edit menu', () => {
    document.body.innerHTML = '<div id="page">总览</div><input id="host" />'
    install()

    expect(rightClick(document.querySelector('#page')!)).toBe(true)
    // The native cut/copy/paste block is the only pointer-driven paste path for SSH
    // addresses and key paths, so this event must reach the OS uncancelled.
    expect(rightClick(document.querySelector('#host')!)).toBe(false)
  })

  it('stops guarding once the returned uninstaller runs', () => {
    document.body.innerHTML = '<div id="page">总览</div>'
    const page = document.querySelector('#page')!
    const uninstall = installContextMenuGuard(document)

    expect(rightClick(page)).toBe(true)
    uninstall()
    expect(rightClick(page)).toBe(false)
  })

  it('defaults to the ambient document when no root is passed', () => {
    document.body.innerHTML = '<div id="page">总览</div>'
    teardowns.push(installContextMenuGuard())

    expect(rightClick(document.querySelector('#page')!)).toBe(true)
  })
})
