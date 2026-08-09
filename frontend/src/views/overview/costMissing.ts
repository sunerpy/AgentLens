/**
 * EXCLUSIVE FILE BOUNDARY — todo 15 owns `src/views/overview/**`.
 *
 * Which provider / model pairs the archive holds no usable price for.
 *
 * ## Why there are two functions and only one of them feeds the panel
 *
 * `部分缺失` on its own told the user a number and nothing else, so "什么没有写价格" had no
 * answer anywhere in the UI. Two sources can answer it and they measure **different things**:
 *
 * - {@link rangeMissingPriceEntries} reads the `model` groups of `get_trend`, which the
 *   overview already fetches. Each group is `providerId\0modelId` and each of its buckets
 *   carries a `CostTotals`, so summing `unavailableCount` over the buckets gives the number of
 *   records that model contributed to the headline — **same range, same filters, same
 *   `is_incomplete = 0` exclusion as `get_summary`**. The per-model figures therefore *add up
 *   to* `CostTotals.unavailableCount`; they are a decomposition of it, not a second statistic.
 * - {@link missingPriceEntries} reads `PriceCatalog.observedModels`, whose SQL groups over the
 *   whole `usage_record` table with **no time predicate**. It is archive-wide.
 *
 * The shipped panel put the range-scoped count above the archive-wide list, and users read the
 * two as one statistic: `21,947 条` over a list summing to `50,923`. Prose could not rescue
 * that — two figures on different scopes must not sit side by side. So the panel now lists the
 * range-scoped decomposition, and the archive-wide list is a **separately labelled fallback**
 * used only when the trend query has produced no groups (the full archive-wide view, with
 * paging, filtering and search, already lives in 设置 → 价格覆盖 → 归档中的模型匹配).
 *
 * ## Why the cause of each gap is deliberately not classified here
 *
 * Splitting the list into "变体后缀问题 / 目录缺条目 / provider 隔离" is not derivable from any
 * DTO the frontend receives. `ObservedModelPrice.matchKind` collapses all three: a suffix miss,
 * a genuinely absent catalog entry and a deliberate provider-isolation refusal **all** surface
 * as `unknown` with `matchedPrice: null`, while `normalized` / `family` each cover several
 * unrelated normalisations. Guessing from model-name substrings would be a hardcoded list that
 * goes stale the moment the catalog changes, so the panel names the gap and points at the
 * settings surface instead of inventing a taxonomy it cannot support.
 */
import type { ObservedModelPrice, SeriesGroup, TokenValues } from '@/generated'

/** One provider / model pair with no price, and how many records it covers. */
export interface MissingPriceEntry {
  providerId: string
  modelId: string
  usageCount: number
}

/** Rows shown before the list has to be expanded. */
export const MISSING_PRICE_PREVIEW = 5

/**
 * The `id` separator Rust uses for a `model` series group:
 * `format!("{provider_id}\0{model_id}")` in `agentlens_core::query`. A NUL cannot occur inside
 * either identifier, which is why it was chosen over `/` — a model id may contain a slash
 * (`openai.gpt-5.6-sol`, `us.anthropic.claude-…`), so splitting on `/` would corrupt identities.
 */
const MODEL_GROUP_SEPARATOR = '\u0000'

/** Heaviest usage first; ties broken by identity so the order is stable across renders. */
function byUsageThenIdentity(left: MissingPriceEntry, right: MissingPriceEntry): number {
  return (
    right.usageCount - left.usageCount ||
    left.providerId.localeCompare(right.providerId) ||
    left.modelId.localeCompare(right.modelId)
  )
}

/**
 * Only input, output, cache read and cache write require pricing. Reasoning is an output subset,
 * so treating it as a fifth billable bucket would turn exact zero-cost usage into a false gap.
 */
function hasBillableTokens(tokens: TokenValues): boolean {
  return (
    tokens.tokInput > 0 ||
    tokens.tokOutput > 0 ||
    tokens.tokCacheRead > 0 ||
    tokens.tokCacheWrite > 0
  )
}

/**
 * Unpriced models **within the current report range and filters**, heaviest usage first.
 *
 * Derived from series groups the overview has already fetched, so this adds no IPC round-trip.
 * That matters beyond cost: `get_breakdown` carries the same numbers, but the overview is
 * asserted to issue **zero** `get_breakdown` calls (the trend-grouping fan-out regression), and
 * reaching for it here would quietly reintroduce the query storm those assertions prevent.
 *
 * A bucket with `cost: null` is an uncovered window rather than a zero, so it contributes
 * nothing. A group must also carry at least one of the four billable buckets (input, output,
 * cache read, cache write). `tokReasoning` is an output subset and is deliberately not priced
 * separately, so reasoning-only or entirely zero usage cannot cause a missing-price warning.
 */
export function rangeMissingPriceEntries(
  groups: readonly SeriesGroup[],
): readonly MissingPriceEntry[] {
  const entries: MissingPriceEntry[] = []
  for (const group of groups) {
    if (group.dimension !== 'model') continue
    const separator = group.id.indexOf(MODEL_GROUP_SEPARATOR)
    if (separator === -1) continue
    if (!group.series.some((point) => point.tokens !== null && hasBillableTokens(point.tokens))) {
      continue
    }
    const usageCount = group.series.reduce(
      (sum, point) => sum + (point.cost?.unavailableCount ?? 0),
      0,
    )
    if (usageCount <= 0) continue
    entries.push({
      providerId: group.id.slice(0, separator),
      modelId: group.id.slice(separator + 1),
      usageCount,
    })
  }
  return entries.sort(byUsageThenIdentity)
}

/**
 * Unpriced models across the **whole archive**, heaviest usage first.
 *
 * Only `unknown` qualifies. `normalized`, `family` and `crossProvider` all resolved to *some*
 * price, so listing them here would report models that do have a cost as missing one.
 *
 * Archive-wide by construction — see the module docstring for why that scope must never be
 * rendered next to a range-scoped record count.
 */
export function missingPriceEntries(
  models: readonly ObservedModelPrice[],
): readonly MissingPriceEntry[] {
  return models
    .filter((model) => model.matchKind === 'unknown')
    .map((model) => ({
      providerId: model.providerId,
      modelId: model.modelId,
      usageCount: model.usageCount,
    }))
    .sort(byUsageThenIdentity)
}

/**
 * Records the headline counts that no listed model accounts for.
 *
 * Zero is the expected value: both sides come from the same range, filters and exclusion, so
 * the decomposition is exhaustive. It is surfaced anyway, and only when positive, because a
 * residual is the one thing that would make the list stop adding up — and the whole defect this
 * panel is fixing was a total the list did not add up to. Better a visible remainder than a
 * user redoing the arithmetic.
 */
export function unattributedCount(
  entries: readonly MissingPriceEntry[],
  unavailableCount: number,
): number {
  const listed = entries.reduce((sum, entry) => sum + entry.usageCount, 0)
  return Math.max(0, unavailableCount - listed)
}
