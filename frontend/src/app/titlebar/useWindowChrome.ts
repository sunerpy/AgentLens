import { useCallback, useEffect, useMemo, useState } from 'react'
import type { UnlistenFn } from '@tauri-apps/api/event'

import { currentPlatform, type TitlebarPlatform } from './platform'
import { createWindowControls, type WindowControls } from './windowControls'

export type WindowChrome = {
  platform: TitlebarPlatform
  controls: WindowControls | null
  isMaximized: boolean
  minimize: () => void
  toggleMaximize: () => void
  close: () => void
  dragOnNonMousePointer: (pointerType: string) => void
}

/**
 * `UnlistenFn` is declared `() => void`, but `@tauri-apps/api` implements it as an async function
 * (`event.js:81`) that first touches `window.__TAURI_EVENT_PLUGIN_INTERNALS__` — a *different*
 * global from `__TAURI_INTERNALS__`, which a partial host such as a mock IPC harness may not
 * install. So a failure arrives in two shapes: a synchronous throw (caught by `try`) or a
 * rejected promise nobody awaits (caught by `.catch`). Both must be contained, because this runs
 * from the effect's cleanup and React turns an exception thrown out of cleanup into a
 * render-phase error that unmounts the whole tree. Losing the unsubscribe costs nothing by
 * comparison: teardown has nothing left to clean up and the listener dies with the webview.
 */
function unsubscribeQuietly(unlisten: UnlistenFn): void {
  try {
    void Promise.resolve(unlisten()).catch(() => undefined)
  } catch {
    // Contained deliberately; see above.
  }
}

export function useWindowChrome(
  factory: () => WindowControls | null = createWindowControls,
  platformOf: () => TitlebarPlatform = currentPlatform,
): WindowChrome {
  const controls = useMemo(() => factory(), [factory])
  const platform = useMemo(() => platformOf(), [platformOf])
  const [isMaximized, setIsMaximized] = useState(false)

  useEffect(() => {
    if (controls === null) return undefined

    let unlisten: UnlistenFn | undefined
    let disposed = false

    const sync = async () => {
      try {
        const maximized = await controls.isMaximized()
        if (!disposed) setIsMaximized(maximized)
      } catch {
        // A rejected query (no permission, mock IPC, window already gone) must not blank
        // the titlebar; the last known state stays on screen.
      }
    }

    // The initial query is not redundant with the subscription: a window restored as
    // maximized by the OS (or by a window-state plugin) emits no resize event at startup,
    // so without this the restore icon would be wrong until the first user resize.
    void sync()

    void controls
      .onResized(() => void sync())
      .then((fn) => {
        // The effect can be torn down before this promise settles (React 18 StrictMode
        // double-invokes effects), so unsubscribe immediately rather than leaking.
        if (disposed) unsubscribeQuietly(fn)
        else unlisten = fn
      })
      .catch(() => undefined)

    return () => {
      disposed = true
      if (unlisten !== undefined) unsubscribeQuietly(unlisten)
    }
  }, [controls])

  const run = useCallback((action: (() => Promise<void>) | undefined) => {
    if (action === undefined) return
    void action().catch(() => undefined)
  }, [])

  return {
    platform,
    controls,
    isMaximized,
    minimize: useCallback(() => run(controls?.minimize), [controls, run]),
    toggleMaximize: useCallback(() => run(controls?.toggleMaximize), [controls, run]),
    close: useCallback(() => run(controls?.close), [controls, run]),
    // Tauri's injected drag script is mouse-only, so touch and pen never reach it
    // (tauri-apps/tauri#4746). The documented `app-region: drag` workaround is not usable
    // here because it swallows clicks on every button in the bar, so the fallback is
    // scoped to non-mouse pointers instead — mouse drags stay with the native script and
    // are never double-invoked.
    dragOnNonMousePointer: useCallback(
      (pointerType: string) => {
        if (pointerType === 'mouse') return
        run(controls?.startDragging)
      },
      [controls, run],
    ),
  }
}
