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
 * the last two after coverage annotations and a stacked per-tier layout had already been added.
 *
 * Those rounds each answered "how do we explain the gap". That was the wrong question twice over:
 *
 * 1. **The upper figure's name was wrong.** `CostSource::Actual` comes from the `cost` field
 *    OpenCode writes into its own message records — a number OpenCode computed from *its* price
 *    table, not a cloud invoice. Calling it 实际 made every explanation read as "the real figure
 *    is $83, so the estimate is broken". It is renamed 来源自带 in `zh.ts`; see the note there
 *    for why the Rust variant keeps its name.
 * 2. **It should not hold the main visual slot.** Measured on this project: 117 of 289,834
 *    records (0.03%) carry an upstream amount. Giving a 0.03%-coverage figure the same weight as
 *    the 99.97% one is what invited the subtraction, and no footnote outranks a layout.
 *
 * So the local estimate is now the single headline, and the source-provided tier is a collapsed
 * disclosure that is absent entirely at zero coverage ({@link CostTiersView.hasSourceProvided}).
 * The derived reading aids stay, because inside that disclosure they are exactly what a reader
 * needs: {@link CostTierView.tokenShare} (how much of the range the tier covers) and
 * {@link CostTierView.unitPricePerMillion} (the figure that *is* comparable across tiers — on the
 * reported data $4.12 vs $4.47 per million, the same order of magnitude, which is the proof that
 * all of the 3,700× gap is coverage rather than method).
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

/**
 * Whether the primary amount's `$0` needs disclaiming, and on what grounds.
 *
 * The card now leads with **one** number — the local estimate — so only that number's zero can
 * mislead. `estimatedNoCoverage` is the case the previous layout papered over with a paragraph
 * about "两格的 $0": no record was priced locally, so the headline `$0` means "nothing landed
 * here", not "this range was free". `noCoverage` is the same claim widened to the whole range.
 * `null` means the headline stands on real coverage and needs no caveat at all.
 */
export type CostPrimaryNote = 'estimatedNoCoverage' | 'noCoverage' | null

export interface CostTiersView {
  actual: CostTierView
  estimated: CostTierView
  unavailable: CostTierView
  /** Records across all three layers; the layers partition the range, so this is exact. */
  totalRecordCount: number
  /** Billable tokens across all three layers. A denominator, never an amount. */
  totalBillableTokens: number
  comparability: CostComparability
  /**
   * Whether the source-provided tier is worth showing at all.
   *
   * Gated on record count, not on the amount: 117 records summing to `$0` is still coverage worth
   * disclosing, while zero records is an amount that describes nothing. When this is `false` the
   * tier is absent from the DOM entirely — that is the structural fix for the `$0` beside a real
   * amount, which every previous round tried to solve with copy.
   */
  hasSourceProvided: boolean
  primaryNote: CostPrimaryNote
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

function primaryNoteOf(comparability: CostComparability): CostPrimaryNote {
  if (comparability === 'actualOnly') return 'estimatedNoCoverage'
  if (comparability === 'empty') return 'noCoverage'
  return null
}

export function costTiers(cost: CostTotals, coverage: CostCoverage): CostTiersView {
  const totalBillableTokens =
    coverage.actual.billableTokens +
    coverage.estimated.billableTokens +
    coverage.unavailable.billableTokens
  const comparability = comparabilityOf(coverage.actual.recordCount, coverage.estimated.recordCount)
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
    comparability,
    hasSourceProvided: coverage.actual.recordCount > 0,
    primaryNote: primaryNoteOf(comparability),
  }
}
