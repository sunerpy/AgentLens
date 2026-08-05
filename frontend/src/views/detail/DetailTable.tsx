/**
 * EXCLUSIVE FILE BOUNDARY — todo 17 owns `src/views/detail/**`.
 *
 * Plain-markup table. No table or virtualization library is installed and none is needed: a page
 * is 50 rows, and adding a dependency would collide with a sibling worker's `package.json`.
 */
import type { MessageRow } from '@/generated'
import { zh } from '@/i18n/zh'

import { DetailBadge } from './DetailBadge'
import {
  displayTokens,
  formatCount,
  formatMoney,
  formatTimestamp,
  resolveDetailCost,
} from './formatDetail'

const HEAD_CLASS =
  'sticky top-0 z-10 bg-muted/70 px-3 py-2 text-left text-xs font-semibold text-muted-foreground backdrop-blur'
const NUM_HEAD_CLASS = `${HEAD_CLASS} text-right`
const CELL_CLASS = 'px-3 py-1.5 align-middle'
const NUM_CELL_CLASS = `${CELL_CLASS} text-right tabular-nums`

// `unavailable` is intentionally neutral, not warning: real archives have cost = 0 on every row,
// so it is the common case. The warning tone belongs to `is_incomplete`, which is exceptional.
const COST_TONE = {
  actual: 'accent',
  estimated: 'muted',
  unavailable: 'neutral',
} as const

const COST_LABEL = {
  actual: zh.common.cost.actual,
  estimated: zh.common.cost.estimated,
  unavailable: zh.common.cost.unavailable,
} as const

function CostCell({ row }: { row: MessageRow }) {
  const resolved = resolveDetailCost(row.cost)
  return (
    <td className={NUM_CELL_CLASS}>
      <span className="inline-flex items-center justify-end gap-1.5">
        {resolved.amount === null ? null : <span>{formatMoney(resolved.amount)}</span>}
        <DetailBadge tone={COST_TONE[resolved.kind]} data-testid="detail-cost-source">
          {COST_LABEL[resolved.kind]}
        </DetailBadge>
      </span>
    </td>
  )
}

function TokenCells({ row }: { row: MessageRow }) {
  const tokens = displayTokens(row.tokens)
  const cacheHint = `${zh.detail.cacheHint}: ${zh.common.tokens.cacheRead} ${formatCount(tokens.cacheRead)} / ${zh.common.tokens.cacheWrite} ${formatCount(tokens.cacheWrite)}`
  return (
    <>
      <td className={NUM_CELL_CLASS}>{formatCount(tokens.input)}</td>
      <td className={NUM_CELL_CLASS}>{formatCount(tokens.output)}</td>
      <td className={NUM_CELL_CLASS}>{formatCount(tokens.reasoning)}</td>
      <td className={NUM_CELL_CLASS} title={cacheHint}>
        {formatCount(tokens.cache)}
      </td>
    </>
  )
}

export function DetailTable({ rows, timezone }: { rows: MessageRow[]; timezone: string }) {
  return (
    <div
      data-testid="detail-table-scroll"
      className="overflow-x-auto rounded-xl border border-border"
    >
      <table data-testid="detail-table" className="w-full border-collapse text-sm">
        <caption className="sr-only">{zh.detail.tableLabel}</caption>
        <thead>
          <tr>
            <th scope="col" className={HEAD_CLASS}>
              {`${zh.detail.columns.time}（${timezone}）`}
            </th>
            <th scope="col" className={HEAD_CLASS}>
              {zh.detail.columns.host}
            </th>
            <th scope="col" className={HEAD_CLASS}>
              {zh.detail.columns.agent}
            </th>
            <th scope="col" className={HEAD_CLASS}>
              {zh.detail.columns.model}
            </th>
            <th scope="col" className={NUM_HEAD_CLASS}>
              {zh.common.tokens.input}
            </th>
            <th scope="col" className={NUM_HEAD_CLASS}>
              {zh.common.tokens.output}
            </th>
            <th scope="col" className={NUM_HEAD_CLASS}>
              {zh.common.tokens.reasoning}
            </th>
            <th scope="col" className={NUM_HEAD_CLASS}>
              {zh.detail.columns.cache}
            </th>
            <th scope="col" className={NUM_HEAD_CLASS}>
              {zh.common.cost.label}
            </th>
            <th scope="col" className={HEAD_CLASS}>
              {zh.detail.columns.flags}
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr
              key={`${row.hostId}/${row.source}/${row.messageId}`}
              data-testid="detail-row"
              data-message-id={row.messageId}
              className="border-t border-border/70 odd:bg-muted/20 hover:bg-muted/40"
            >
              <td className={`${CELL_CLASS} whitespace-nowrap tabular-nums`}>
                {formatTimestamp(row.timeCreatedUtc, timezone)}
              </td>
              <td className={`${CELL_CLASS} whitespace-nowrap`}>{row.hostId}</td>
              <td className={`${CELL_CLASS} whitespace-nowrap`}>{row.agentRaw || row.agentKey}</td>
              <td className={CELL_CLASS}>
                <span className="flex max-w-56 items-center gap-1.5">
                  <span className="truncate" title={`${row.providerId}/${row.modelId}`}>
                    {row.modelId}
                  </span>
                  {row.variant === null ? null : (
                    <DetailBadge tone="muted" data-testid="detail-variant">
                      {row.variant}
                    </DetailBadge>
                  )}
                </span>
              </td>
              <TokenCells row={row} />
              <CostCell row={row} />
              <td className={`${CELL_CLASS} whitespace-nowrap`}>
                {row.isIncomplete ? (
                  <DetailBadge
                    tone="warning"
                    data-testid="detail-incomplete"
                    title={zh.detail.incompleteHint}
                  >
                    {zh.detail.incomplete}
                  </DetailBadge>
                ) : null}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
