/**
 * F3 real-manual-QA harness — WebdriverIO + tauri-driver against the REAL debug app.
 *
 * This is QA harness code, not product code. It is intentionally the only wdio config in
 * the repo and is driven by `.omo/evidence/f3-agentlens-usage-dashboard/`.
 *
 * Why the pieces are shaped this way (all measured, not guessed):
 *  - `tauri-driver` is spawned in `onPrepare` and killed in `onComplete`; it in turn spawns
 *    `WebKitWebDriver` (from Arch `webkitgtk-6.0`) and the application binary, so the app
 *    inherits this process's environment. That is how `XDG_DATA_HOME` / `OPENCODE_DATA_DIR`
 *    / `DISPLAY` reach the app.
 *  - The debug binary loads its frontend from `build.devUrl` (http://localhost:1420), so a
 *    `vite dev` server must already be running. It is started outside wdio because a
 *    backgrounded server inheriting this process's stdout hangs the surrounding tool.
 *  - `AGENTLENS_TRAY_SELFTEST_DIR` is deliberately deleted from the child environment: when
 *    set, the app's built-in tray self-test driver hides/quits the window on its own and
 *    fights the WebDriver session for control.
 */
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const REPO = path.resolve(HERE, '..')
const APP = path.join(REPO, 'target/debug/agentlens-tauri')
// `AGENTLENS_WDIO_EVIDENCE` lets a later gate write its own run artifacts instead of
// overwriting the F3 reviewer's record, which is read-only history.
const EVIDENCE =
  process.env.AGENTLENS_WDIO_EVIDENCE ??
  path.join(REPO, '.omo/evidence/f3-agentlens-usage-dashboard')

if (!fs.existsSync(APP)) throw new Error(`debug binary missing: ${APP}`)
fs.mkdirSync(path.join(EVIDENCE, 'screenshots'), { recursive: true })

let driver = null

export const config = {
  runner: 'local',
  hostname: 'localhost',
  port: 4444,
  path: '/',
  specs: [path.join(HERE, 'e2e-real/*.spec.mjs')],
  maxInstances: 1,
  capabilities: [
    {
      // tauri-driver reads this and launches the binary itself.
      'tauri:options': { application: APP },
      browserName: 'wry',
      // WebdriverIO 9 asks for BiDi (`alwaysMatch.webSocketUrl = true`) by default.
      // WebKitWebDriver behind tauri-driver has no BiDi endpoint and answers the whole
      // POST /session with "Failed to match capabilities", so classic must be forced.
      // Verified against the same server: a raw curl WITHOUT webSocketUrl creates a
      // session (browserName wry 0.55.1) while wdio's default request does not.
      'wdio:enforceWebDriverClassic': true,
    },
  ],
  logLevel: 'info',
  waitforTimeout: 30_000,
  connectionRetryTimeout: 180_000,
  connectionRetryCount: 3,
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: { ui: 'bdd', timeout: 300_000 },

  onPrepare: () =>
    new Promise((resolve, reject) => {
      const env = { ...process.env }
      delete env.AGENTLENS_TRAY_SELFTEST_DIR
      // `--native-port` MUST be moved off its 4445 default: measured here, tauri-driver
      // otherwise makes WebKitWebDriver try to bind a port that collides in this container
      // and the native driver dies with "FATAL: Unable to listen for HTTP server", which
      // surfaces confusingly as "Failed to match capabilities" on POST /session.
      driver = spawn(
        'tauri-driver',
        ['--port', '4444', '--native-port', '4450', '--native-host', '127.0.0.1'],
        { env, stdio: ['ignore', 'pipe', 'pipe'] },
      )
      const log = fs.createWriteStream(path.join(EVIDENCE, 'tauri-driver.log'))
      driver.stdout.pipe(log)
      driver.stderr.pipe(log)
      driver.on('error', reject)
      // tauri-driver binds immediately; give it a beat rather than racing the first session.
      setTimeout(resolve, 2000)
    }),

  onComplete: () => {
    driver?.kill('SIGTERM')
  },
}
