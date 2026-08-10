/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Derived reading aids for the cost card. **Presentation only** — no pricing, no cost
 * arithmetic, and the three cost amounts are never summed.
 *
 * ### The defect this exists to fix
 *
 * The card used to put 实际 and 估算 side by side at identical visual weight:
 *
 * ```text
 * 实际  $83.5228        117 条记录 · 20.3M 可计费 Token
 * 估算  $312,235.4418   287,747 条记录 · 69.9B 可计费 Token
 * ```
 *
 * Users read that as "the estimate blew the cost up 3,700×" and asked why three separate times,
 * the last two after coverage annotations had already been added. The annotations were true but
 * they were footnotes to a comparison the layout itself was inviting.
 *
 * The two amounts are not two algorithms applied to one set of usage. They are **two disjoint
 * sets of records**: OpenCode is the only source carrying `CostSource::Actual`, and even there
 * only 117 rows had a real amount attached. Measured on that same data the unit prices are
 * $4.12 and $4.47 per million billable tokens — the same order of magnitude. Every bit of the
 * 3,700× gap is coverage, not method.
 *
 * So the fix is not another footnote. It is to publish the two figures a reader would otherwise
 * have to divide for: {@link CostTierView.tokenShare} (how much of the range each tier covers)
 * and {@link CostTierView.unitPricePerMillion} (the figure that *is* comparable across tiers).
 *
 * ### Why summing tokens here does not break "三态永不相加"
 *
 * That rule is about the amounts: 实际 / 估算 / 无可信成本 are never added into one number,
 * and nothing here does. {@link CostTiersView.totalBillableTokens} adds **token counts**, and the
 * three coverage layers partition the range's records, so their token counts have a well-defined
 * total — it is the denominator of a share, never a cost.
 */
import type { CostCoverage, CostTotals } from '@/generated'

export type CostTierKey = 'actual' | 'estimated' | 'unavailable'

export interface CostTierView {
  key: CostTierKey
  /** Records this tier covers. Zero means the tier's amount describes nothing at all. */
  recordCount: number
  billableTokens: number
  /** Share of the range's billable tokens, `0..1`; `null` when the range has none. */
  tokenShare: number | null
  /**
   * USD per one million billable tokens, or `null` when this tier has no billable tokens (a
   * price over nothing) or carries no amount at all (无可信成本 has a count, not a sum).
   *
   * This is the only cross-tier comparison the card offers, because it is the only one that is
   * defined: it divides out the coverage difference that makes the amounts incomparable.
   */
  unitPricePerMillion: number | null
}

/**
 * Which cross-tier statement the card is allowed to make.
 *
 * `incomparable` is the normal case and the one the user tripped over. The single-tier cases are
 * not merely "the other one is small" — the empty tier's `$0.0000` is the absence of coverage,
 * which is a different claim from "this tier cost nothing", and the copy has to say so.
 */
export type CostComparability = 'incomparable' | 'actualOnly' | 'estimatedOnly' | 'empty'

export interface CostTiersView {
  actual: CostTierView
  estimated: CostTierView
  unavailable: CostTierView
  /** Records across all three layers; the layers partition the range, so this is exact. */
  totalRecordCount: number
  /** Billable tokens across all three layers. A denominator, never an amount. */
  totalBillableTokens: number
  comparability: CostComparability
}

const TOKENS_PER_UNIT_PRICE = 1_000_000

function unitPrice(amount: number | null, billableTokens: number): number | null {
  if (amount === null || billableTokens <= 0) return null
  return amount / (billableTokens / TOKENS_PER_UNIT_PRICE)
}

function tier(
  key: CostTierKey,
  amount: number | null,
  layer: { recordCount: number; billableTokens: number },
  totalBillableTokens: number,
): CostTierView {
  return {
    key,
    recordCount: layer.recordCount,
    billableTokens: layer.billableTokens,
    tokenShare: totalBillableTokens > 0 ? layer.billableTokens / totalBillableTokens : null,
    unitPricePerMillion: unitPrice(amount, layer.billableTokens),
  }
}

function comparabilityOf(actualRecords: number, estimatedRecords: number): CostComparability {
  if (actualRecords > 0 && estimatedRecords > 0) return 'incomparable'
  if (actualRecords > 0) return 'actualOnly'
  if (estimatedRecords > 0) return 'estimatedOnly'
  return 'empty'
}

export function costTiers(cost: CostTotals, coverage: CostCoverage): CostTiersView {
  const totalBillableTokens =
    coverage.actual.billableTokens +
    coverage.estimated.billableTokens +
    coverage.unavailable.billableTokens
  return {
    actual: tier('actual', cost.actualSum, coverage.actual, totalBillableTokens),
    estimated: tier('estimated', cost.estimatedSum, coverage.estimated, totalBillableTokens),
    // 无可信成本 has no amount by definition, so it can have no unit price either.
    unavailable: tier('unavailable', null, coverage.unavailable, totalBillableTokens),
    totalRecordCount:
      coverage.actual.recordCount +
      coverage.estimated.recordCount +
      coverage.unavailable.recordCount,
    totalBillableTokens,
    comparability: comparabilityOf(coverage.actual.recordCount, coverage.estimated.recordCount),
  }
}
