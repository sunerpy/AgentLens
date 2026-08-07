/**
 * Desktop context-menu policy.
 *
 * A Tauri webview still ships Chromium's page context menu ("Reload", "Inspect", "Back"),
 * which is web-browser chrome leaking into a desktop app: reloading blows away the whole
 * React tree mid-refresh, and "Back" has no meaning in a single-document shell. So the
 * default is suppressed.
 *
 * The suppression is **not** blanket, because the native menu is the only paste affordance
 * on an editable field. AgentLens asks the user to type SSH host addresses, key paths and
 * price numbers, all of which are pasted in practice; a global `preventDefault` would leave
 * Ctrl+V as the only way in and would be a usability regression worse than the stray menu.
 * Editable targets are therefore exempt.
 *
 * Kept out of `main.tsx` on purpose: `main.tsx` is excluded from the coverage denominator
 * (see `vitest.config.ts`), and the exemption rule is exactly the part that must be tested.
 */

/**
 * Targets that keep the native menu: the browser's cut/copy/paste/select-all block is the
 * only pointer-driven paste path.
 *
 * `[contenteditable]` alone would also match `contenteditable="false"`, which is an explicit
 * opt-out and must stay suppressed — hence the `:not(...)`.
 */
const EDITABLE_SELECTOR = 'input, textarea, [contenteditable]:not([contenteditable="false"])'

/**
 * True when the event must be cancelled, i.e. the target is not an editable field.
 *
 * Non-`Element` targets (a bare text node, or `null` on a synthetic event) cannot be
 * editable, so they are suppressed like any other page content.
 */
export function shouldSuppressContextMenu(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return true
  return target.closest(EDITABLE_SELECTOR) === null
}

/**
 * Install the guard for the lifetime of the document. Returns the uninstaller so tests (and
 * any future teardown) can remove the listener instead of leaking it across cases.
 */
export function installContextMenuGuard(root: Document = document): () => void {
  const onContextMenu = (event: Event) => {
    if (shouldSuppressContextMenu(event.target)) event.preventDefault()
  }
  root.addEventListener('contextmenu', onContextMenu)
  return () => root.removeEventListener('contextmenu', onContextMenu)
}
