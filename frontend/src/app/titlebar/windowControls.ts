import { getCurrentWindow } from '@tauri-apps/api/window'
import type { UnlistenFn } from '@tauri-apps/api/event'

export type WindowControls = {
  minimize: () => Promise<void>
  toggleMaximize: () => Promise<void>
  close: () => Promise<void>
  isMaximized: () => Promise<boolean>
  startDragging: () => Promise<void>
  onResized: (handler: () => void) => Promise<UnlistenFn>
}

/**
 * Returns `null` outside a Tauri webview. `getCurrentWindow()` reads
 * `window.__TAURI_INTERNALS__.metadata.currentWindow.label` eagerly, so in a plain browser
 * it throws a `TypeError` rather than returning a dead handle — which is exactly the
 * vitest/`npm run dev` case. Callers render a button-less titlebar instead of crashing.
 */
export function createWindowControls(): WindowControls | null {
  try {
    const appWindow = getCurrentWindow()
    return {
      minimize: () => appWindow.minimize(),
      toggleMaximize: () => appWindow.toggleMaximize(),
      close: () => appWindow.close(),
      isMaximized: () => appWindow.isMaximized(),
      startDragging: () => appWindow.startDragging(),
      onResized: (handler) => appWindow.onResized(() => handler()),
    }
  } catch {
    return null
  }
}
