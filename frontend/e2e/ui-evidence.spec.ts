import { expect, test, type Page } from '@playwright/test'

import { openShell } from './harness'

/**
 * 需求验收截图。产物写到仓库外的 `/config/workspace/.scratch/agentlens/shots-ui/`，
 * 而不是 `frontend/` 下：`frontend/test-results/` 之类的残留会被 `make lint` 的 fmt-check
 * 扫到并失败。
 *
 * 报表时区刻意设为 Asia/Shanghai：种子里的「最近成功」时间戳这样才会渲染成东八区的墙上时钟，
 * 从而在图上直接看出时区设置真的生效了（旧实现无条件按 UTC 渲染）。
 */
const SHOTS = '/config/workspace/.scratch/agentlens/shots-ui'

const SHANGHAI_DATASET = {
  settings: {
    values: {
      'report.timezone': 'Asia/Shanghai',
      'report.weekStart': 'monday',
      'refresh.localIntervalMs': '600000',
      'refresh.remoteIntervalMs': '900000',
      'archive.path': '/tmp/agentlens-qa/agentlens/archive.db',
    },
  },
} as const

/**
 * `local_machine_identity` 与 `credential_status` 不在共享 `IpcCommand` 联合里，共享 mock
 * 因此答不上来。截图必须反映正常状态，不能带着一条 mock 夹具的报错，所以这里按
 * `hosts.spec.ts` 的同一套办法把它们补上。
 */
async function stubLocalIdentity(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const handle = async (command: string, args: Record<string, unknown>): Promise<unknown> => {
      switch (command) {
        case 'local_machine_identity':
          return {
            hostId: 'local-host-000001',
            machineIdHash: 'a'.repeat(64),
            hostname: 'workstation',
          }
        case 'credential_status':
          return { hostId: args.hostId, kind: args.kind, present: false }
        default:
          return undefined
      }
    }

    const extra = ['local_machine_identity', 'credential_status']
    let current: Record<string, unknown> | undefined
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      get: () => current,
      set: (value: Record<string, unknown>) => {
        const inner = value.invoke as (
          command: string,
          args?: Record<string, unknown>,
        ) => Promise<unknown>
        value.invoke = async (command: string, args: Record<string, unknown> = {}) =>
          extra.includes(command) ? handle(command, args) : inner(command, args)
        current = value
      },
    })
  })
}

test('主机列表：多采集源状态、本机无凭据按钮、一键刷新、时区感知时间', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1000 })
  await stubLocalIdentity(page)
  await openShell(page, { dataset: SHANGHAI_DATASET })
  await page.getByTestId('nav-hosts').click()
  await expect(page.getByTestId('view-hosts')).toBeVisible()
  await expect(page.getByTestId('host-source-ssh-host-0000002-claude-code')).toHaveAttribute(
    'data-source-state',
    'error',
  )

  // 时区生效的可视证据：UTC 的 2026-01-05 00:00:00 在东八区是当日 08:00:00。
  await expect(page.getByTestId('host-last-success-ssh-host-0000002')).toHaveText(
    '2026-01-05 08:00:00',
  )

  await page.screenshot({
    path: `${SHOTS}/hosts-multi-source.png`,
    fullPage: true,
    animations: 'disabled',
  })
})

test('设置页：自动刷新开关与 600 秒下限', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1000 })
  await openShell(page, { dataset: SHANGHAI_DATASET })
  await page.getByTestId('nav-settings').click()
  await expect(page.getByTestId('settings-report')).toBeVisible()
  await expect(page.getByTestId('settings-auto-refresh')).toBeChecked()

  // 让下限报错一起进图，证明它是错误而不是「已自动调整」。
  await page.getByTestId('settings-local-interval').fill('60')
  await expect(page.getByTestId('settings-local-interval-issue')).toBeVisible()
  await expect(page.getByTestId('settings-save')).toBeDisabled()

  await page.screenshot({
    path: `${SHOTS}/settings-auto-refresh.png`,
    fullPage: true,
    animations: 'disabled',
  })
})

test('自定义区间：起始与截止两个独立日期选择器', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1000 })
  await openShell(page, { dataset: SHANGHAI_DATASET })
  await expect(page.getByTestId('view-overview')).toBeVisible()

  await page.getByTestId('range-preset-custom').click()
  await expect(page.getByTestId('range-custom-panel')).toBeVisible()
  await page.getByTestId('range-start-date').fill('2026-01-01')
  await page.getByTestId('range-end-date').fill('2026-01-07')
  await expect(page.getByTestId('range-custom-apply')).toBeEnabled()

  await page.screenshot({
    path: `${SHOTS}/range-custom-dual-date.png`,
    animations: 'disabled',
  })
})
