import { defineConfig, devices } from '@playwright/test'

/**
 * Component-level QA: real Chromium against `vite dev` with the IPC layer mocked
 * (`?mockIpc=1`). Real-IPC verification through tauri-driver is a separate, later gate.
 *
 * `webServer.reuseExistingServer` is on outside CI so a developer's running `npm run dev`
 * is reused instead of fighting over the fixed port 1420 that Tauri requires.
 */
export default defineConfig({
  testDir: './e2e',
  outputDir: '../artifacts/qa/playwright',
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: [['list']],
  use: {
    baseURL: 'http://localhost:1420',
    trace: 'retain-on-failure',
    screenshot: 'off',
    video: 'off',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'npm run dev',
    url: 'http://localhost:1420',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
})
