/**
 * F3 tray verification through the REAL driver.
 *
 * The two commands under test are `#[cfg(debug_assertions)]`-only IPC commands in
 * `src-tauri/src/tray.rs`. `test_close_main_window` goes through the real `CloseRequested`
 * path (hide-to-tray, webview NOT destroyed); `test_quit` exits the process.
 *
 * The PID is discovered from the tauri-driver process tree, so the assertions are about the
 * real OS process, not about a UI state flag.
 */
import { execSync } from 'node:child_process'

import { expect, browser, $ } from '@wdio/globals'

import { invoke, shot } from './support.mjs'

function alive(pid) {
  try {
    execSync(`kill -0 ${pid}`, { stdio: 'ignore' })
    return true
  } catch {
    return false
  }
}

/**
 * The app PID, resolved through the driver process tree rather than a bare `pgrep`:
 * tauri-driver → WebKitWebDriver → agentlens-tauri. Measured with `ps -eo pid,ppid,args`,
 * a live session looks exactly like that, with WebKitNetworkProcess / WebKitWebProcess
 * hanging off the app. Walking the tree keeps a stray app left behind by an earlier run
 * from being mistaken for this session's process.
 */
function appPid() {
  const driver = execSync('pgrep -f "^tauri-driver" || true').toString().trim().split('\n')[0]
  if (!driver) throw new Error('tauri-driver process not found')
  const native = execSync(`pgrep -P ${driver} || true`).toString().trim().split('\n')[0]
  if (!native) throw new Error(`no WebKitWebDriver child under tauri-driver ${driver}`)
  const app = execSync(`pgrep -P ${native} || true`).toString().trim().split('\n').filter(Boolean)
  if (app.length !== 1) throw new Error(`expected one app under driver ${native}, got ${JSON.stringify(app)}`)
  const cmd = execSync(`tr '\\0' ' ' < /proc/${app[0]}/cmdline`).toString()
  console.log(`[tray] resolved tree: tauri-driver=${driver} WebKitWebDriver=${native} app=${app[0]}`)
  console.log(`[tray] app cmdline = ${cmd.trim()}`)
  if (!cmd.includes('agentlens-tauri')) throw new Error(`pid ${app[0]} is not the app: ${cmd}`)
  return Number(app[0])
}

function windows() {
  const out = execSync('DISPLAY=:98 xdotool search --name AgentLens || true').toString().trim()
  return out ? out.split('\n').filter(Boolean) : []
}

describe('F3 tray — close hides to tray, quit exits the process', () => {
  it('close via IPC leaves the process ALIVE; quit makes it DEAD', async () => {
    await $('[data-testid="nav-overview"]').waitForExist({ timeout: 60_000 })
    const pid = appPid()
    console.log(`[tray] app pid = ${pid}`)
    console.log(`[tray] X11 windows before close = ${JSON.stringify(windows())}`)
    console.log(`[tray] kill -0 before close -> ${alive(pid) ? 'ALIVE' : 'DEAD'}`)
    expect(alive(pid)).toBe(true)
    await shot('90-tray-before-close')

    await invoke('test_close_main_window', {})
    // Give the GTK main loop a beat to process the hide.
    await new Promise((resolve) => setTimeout(resolve, 3000))
    const aliveAfterClose = alive(pid)
    console.log(`[tray] kill -0 after test_close_main_window -> ${aliveAfterClose ? 'ALIVE' : 'DEAD'}`)
    console.log(`[tray] X11 windows after close = ${JSON.stringify(windows())}`)
    // The webview must survive: the session is still usable after the window is hidden.
    const stillDriving = await browser.execute(() => document.title)
    console.log(`[tray] document.title while hidden = ${JSON.stringify(stillDriving)}`)
    expect(aliveAfterClose).toBe(true)
    expect(stillDriving).toBe('AgentLens')

    // `test_quit` never returns a response (the process exits mid-call), so fire and forget.
    await browser
      .execute(() => {
        window.__TAURI_INTERNALS__.invoke('test_quit', {})
      })
      .catch((error) => console.log(`[tray] test_quit dispatch threw (expected): ${error.message}`))
    await new Promise((resolve) => setTimeout(resolve, 5000))
    const aliveAfterQuit = alive(pid)
    console.log(`[tray] kill -0 after test_quit -> ${aliveAfterQuit ? 'ALIVE' : 'DEAD'}`)
    console.log(`[tray] X11 windows after quit = ${JSON.stringify(windows())}`)
    expect(aliveAfterQuit).toBe(false)
  })
})
