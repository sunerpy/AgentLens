import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import type { MessageRow } from '@/generated'
import { zh } from '@/i18n/zh'

import { DetailTable } from './DetailTable'

afterEach(cleanup)

/** A real Bedrock-routed model id — the value that was overflowing the 标记 column. */
const LONG_MODEL = 'us.anthropic.claude-sonnet-4-5-20250929-v1:0'
const LONG_VARIANT = 'thinking-extra-high-with-a-very-long-suffix'
const LONG_HOST = 'build-box-eu-central-1-shared-runner-07'

function row(overrides: Partial<MessageRow> = {}): MessageRow {
  return {
    hostId: 'local',
    source: 'opencode',
    messageId: 'msg-1',
    sessionId: 'ses-1',
    timeCreatedUtc: Date.UTC(2026, 0, 2, 3, 4, 5),
    agentRaw: 'build',
    agentKey: 'build',
    providerId: 'anthropic',
    modelId: 'claude-opus-4-8',
    variant: null,
    tokens: {
      tokInput: 10,
      tokOutput: 20,
      tokReasoning: 30,
      tokCacheRead: 40,
      tokCacheWrite: 50,
      totalInput: 100,
    },
    cost: { actual: 0.25, estimated: null, unavailable: false },
    isIncomplete: false,
    projectDir: '/home/user/project',
    ...overrides,
  }
}

/**
 * A `<td>` grows to fit its widest unbreakable child, so the guard against a blown-out column is
 * structural, not cosmetic: the cap must sit on a wrapper that owns the overflow, and the clipped
 * child must still repeat its full value in `title`. Asserting the classes is what keeps a future
 * edit from dropping the wrapper and silently restoring the overflow.
 */
describe('DetailTable 的长内容不撑破单元格', () => {
  it('长 model id 落在限宽且截断的单元格里，完整值留在 title 上', () => {
    render(<DetailTable rows={[row({ modelId: LONG_MODEL })]} timezone="UTC" />)

    const cell = screen.getByTestId('detail-model-cell')
    expect(cell.className).toContain('max-w-52')

    const model = cell.querySelector('span[title]')
    expect(model?.className).toContain('truncate')
    expect(model?.getAttribute('title')).toBe(`anthropic/${LONG_MODEL}`)
  })

  it('变体 badge 可收缩并截断，完整变体名留在 title 上', () => {
    render(
      <DetailTable rows={[row({ modelId: LONG_MODEL, variant: LONG_VARIANT })]} timezone="UTC" />,
    )

    const badge = screen.getByTestId('detail-variant')
    expect(badge.getAttribute('title')).toBe(LONG_VARIANT)
    // 外层负责让 flex item 允许收缩，内层负责出省略号；缺任一半截断都不生效。
    expect(badge.className).toContain('overflow-hidden')
    expect(badge.className).toContain('max-w-full')
    expect(badge.firstElementChild?.className).toContain('truncate')
    expect(badge.firstElementChild?.className).toContain('min-w-0')
  })

  it('标记列限宽，未完成 badge 在其中截断', () => {
    render(<DetailTable rows={[row({ isIncomplete: true })]} timezone="UTC" />)

    const cell = screen.getByTestId('detail-flags-cell')
    expect(cell.className).toContain('max-w-28')

    const badge = screen.getByTestId('detail-incomplete')
    expect(badge.className).toContain('overflow-hidden')
    expect(badge.getAttribute('title')).toBe(zh.detail.incompleteHint)
  })

  it('主机与 agent 这两个同样会变长的列也限宽并保留完整值', () => {
    render(<DetailTable rows={[row({ hostId: LONG_HOST, agentRaw: LONG_HOST })]} timezone="UTC" />)

    const cells = screen.getAllByTitle(LONG_HOST)
    expect(cells).toHaveLength(2)
    for (const cell of cells) {
      expect(cell.className).toContain('truncate')
      expect(cell.className).toContain('max-w-28')
    }
  })

  it('成本来源 badge 与金额同格且不换行', () => {
    render(<DetailTable rows={[row()]} timezone="UTC" />)

    const badge = screen.getByTestId('detail-cost-source')
    expect(badge.textContent).toBe(zh.common.cost.actual)
    expect(badge.className).toContain('whitespace-nowrap')
  })

  it('每列都限了宽之后表格仍可横向滚动兜底', () => {
    render(<DetailTable rows={[row({ modelId: LONG_MODEL })]} timezone="UTC" />)

    expect(screen.getByTestId('detail-table-scroll').className).toContain('overflow-x-auto')
  })
})
