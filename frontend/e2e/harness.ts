/**
 * Shared e2e helpers — the template todos 15-18 copy.
 *
 * Owner: W8 prep (shell/infrastructure).
 *
 * Screenshots land in the repo-root `artifacts/qa/` directory (gitignored) because the
 * plan's acceptance criteria name exact paths there, e.g. `artifacts/qa/cost-mixed.png`.
 */
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import type { Page } from '@playwright/test'

import type { MockIpcConfig, MockIpcController } from '../src/lib/mockIpc'

const E2E_DIR = path.dirname(fileURLToPath(import.meta.url))

export const QA_DIR = path.resolve(E2E_DIR, '../../artifacts/qa')

/**
 * Write a full-page screenshot to `artifacts/qa/<name>`.
 *
 * `animations: 'disabled'` is mandatory, not cosmetic: the shadcn buttons carry
 * `transition-all`, so without it a screenshot taken right after a click captures the
 * active-tab background mid-fade and the image misrepresents the rendered state.
 */
export async function qaScreenshot(page: Page, name: string): Promise<string> {
  const target = path.join(QA_DIR, name)
  await page.screenshot({ path: target, fullPage: true, animations: 'disabled' })
  return target
}

/**
 * Install a mock-IPC configuration that takes effect before the app's first render, then
 * open the shell. Configuration must be seeded via `addInitScript` (not `evaluate`) so a
 * forced error is already in place when the very first `invoke` fires.
 */
export async function openShell(page: Page, config: MockIpcConfig = {}): Promise<void> {
  await page.addInitScript((serialized: string) => {
    ;(window as unknown as Record<string, unknown>).__AGENTLENS_MOCK_IPC_CONFIG__ =
      JSON.parse(serialized)
  }, JSON.stringify(config))
  await page.goto('/?mockIpc=1')
}

/** Read recorded IPC calls out of the page. */
export function mockCalls(
  page: Page,
  command: string,
): Promise<ReturnType<MockIpcController['calls']>> {
  return page.evaluate((name: string) => {
    const controller = (window as unknown as Record<string, MockIpcController>)
      .__AGENTLENS_MOCK_IPC__
    return controller.callsFor(name)
  }, command)
}

/** Replace part of the seeded dataset after the app has already rendered. */
export function mockSetDataset(page: Page, patch: MockIpcConfig['dataset']): Promise<void> {
  return page.evaluate((serialized: string) => {
    const controller = (window as unknown as Record<string, MockIpcController>)
      .__AGENTLENS_MOCK_IPC__
    controller.setDataset(JSON.parse(serialized))
  }, JSON.stringify(patch))
}

/** Deliver a Tauri event to the app's own `listen()` subscribers; resolves to the count. */
export function mockEmitEvent(page: Page, event: string, payload?: unknown): Promise<number> {
  return page.evaluate(
    ([name, serialized]: [string, string]) => {
      const controller = (window as unknown as Record<string, MockIpcController>)
        .__AGENTLENS_MOCK_IPC__
      return controller.emitEvent(name, JSON.parse(serialized))
    },
    [event, JSON.stringify(payload ?? null)] as [string, string],
  )
}
