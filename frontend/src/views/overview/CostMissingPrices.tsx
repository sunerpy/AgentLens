/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * The `部分缺失` badge plus the list of what is actually missing.
 *
 * The list is a disclosure panel rather than a tooltip: a tooltip is only discoverable by
 * hovering, and a user who does not already suspect there is more to see never finds it. The
 * badge keeps its own `title` as a supplement, not as the only copy.
 *
 * `entries` is the **range-scoped decomposition** of `unavailableCount` (see `costMissing.ts`),
 * so the headline and the rows add up and the user can check the arithmetic. `archiveEntries` is
 * a different scope and is therefore never rendered alongside those rows — it appears only when
 * `entries` is empty, in its own bordered block with its own scope label. That separation is the
 * fix: the previous version stacked a range-scoped total on an archive-wide list and users read
 * `21,947 条` over a list summing to `50,923` as an arithmetic error.
 */
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { zh } from '@/i18n/zh'
import { CostPartialBadge } from '@/views/overview/CostPartialBadge'
import {
  MISSING_PRICE_PREVIEW,
  unattributedCount,
  type MissingPriceEntry,
} from '@/views/overview/costMissing'
import { formatCount } from '@/views/overview/format'

function MissingRow({ entry, testIdPrefix }: { entry: MissingPriceEntry; testIdPrefix: string }) {
  return (
    <li
      data-testid={`${testIdPrefix}-${entry.providerId}-${entry.modelId}`}
      className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-xs"
    >
      <span className="font-mono select-text">
        {entry.providerId} / {entry.modelId}
      </span>
      <span className="tabular-nums text-muted-foreground select-text">
        {formatCount(entry.usageCount)} {zh.overview.summary.missingRecordUnit}
      </span>
    </li>
  )
}

export function CostMissingPrices({
  entries,
  unavailableCount,
  archiveEntries = [],
}: {
  entries: readonly MissingPriceEntry[]
  unavailableCount: number
  archiveEntries?: readonly MissingPriceEntry[]
}) {
  const [open, setOpen] = useState(false)
  const [expanded, setExpanded] = useState(false)

  if (unavailableCount <= 0) return null

  const shown = expanded ? entries : entries.slice(0, MISSING_PRICE_PREVIEW)
  const hidden = entries.length - shown.length
  const unattributed = unattributedCount(entries, unavailableCount)

  return (
    <div data-testid="cost-missing" className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <CostPartialBadge />
        <Button
          type="button"
          size="sm"
          variant="ghost"
          aria-expanded={open}
          data-testid="cost-missing-toggle"
          onClick={() => setOpen((current) => !current)}
        >
          {open ? zh.overview.summary.missingHide : zh.overview.summary.missingShow}
        </Button>
      </div>
      {open ? (
        <div className="flex flex-col gap-1.5 rounded-lg border border-border border-dashed bg-muted/30 px-3 py-2">
          <span data-testid="cost-missing-count" className="text-xs text-muted-foreground">
            {zh.overview.summary.missingSummary(entries.length, formatCount(unavailableCount))}
          </span>
          {entries.length === 0 ? (
            <span data-testid="cost-missing-empty" className="text-xs text-muted-foreground">
              {zh.overview.summary.missingNoIdentity}
            </span>
          ) : (
            <>
              <ul data-testid="cost-missing-list" className="flex flex-col gap-1">
                {shown.map((entry) => (
                  <MissingRow
                    key={`${entry.providerId}\u0000${entry.modelId}`}
                    entry={entry}
                    testIdPrefix="cost-missing-entry"
                  />
                ))}
              </ul>
              {hidden > 0 ? (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className="self-start"
                  data-testid="cost-missing-expand"
                  onClick={() => setExpanded(true)}
                >
                  {zh.overview.summary.missingExpand(hidden)}
                </Button>
              ) : null}
              {expanded && entries.length > MISSING_PRICE_PREVIEW ? (
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  className="self-start"
                  data-testid="cost-missing-collapse"
                  onClick={() => setExpanded(false)}
                >
                  {zh.overview.summary.missingCollapse}
                </Button>
              ) : null}
              {unattributed > 0 ? (
                <span
                  data-testid="cost-missing-unattributed"
                  className="text-xs text-muted-foreground select-text"
                >
                  {zh.overview.summary.missingUnattributed(formatCount(unattributed))}
                </span>
              ) : null}
              <span className="text-[0.7rem] text-muted-foreground select-none">
                {zh.overview.summary.missingRangeScopeHint}
              </span>
            </>
          )}
          {entries.length === 0 && archiveEntries.length > 0 ? (
            <section
              data-testid="cost-missing-archive"
              className="mt-1 flex flex-col gap-1 rounded-lg border border-border bg-background/60 px-2.5 py-2"
            >
              <span className="text-xs font-medium">{zh.overview.summary.missingArchiveTitle}</span>
              <span className="text-[0.7rem] text-muted-foreground select-none">
                {zh.overview.summary.missingArchiveScopeHint}
              </span>
              <ul data-testid="cost-missing-archive-list" className="flex flex-col gap-1">
                {archiveEntries.slice(0, MISSING_PRICE_PREVIEW).map((entry) => (
                  <MissingRow
                    key={`${entry.providerId}\u0000${entry.modelId}`}
                    entry={entry}
                    testIdPrefix="cost-missing-archive-entry"
                  />
                ))}
              </ul>
            </section>
          ) : null}
          <span className="text-[0.7rem] text-muted-foreground select-none">
            {zh.overview.summary.missingCauseHint}
          </span>
          <span className="text-[0.7rem] text-muted-foreground select-none">
            {zh.overview.summary.missingFixHint}
          </span>
        </div>
      ) : null}
    </div>
  )
}
