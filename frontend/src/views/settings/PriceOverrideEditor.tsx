/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Manual price-override table: add / edit / delete rows, saved through `prices_set`.
 *
 * Both identity columns are dropdowns fed by three candidate sources, in this order:
 *
 * 1. what the archive actually observed (`PriceCatalog.observed_models`), usage-ranked so the
 *    provider / model the user most likely wants to override is the first thing they see;
 * 2. the built-in catalog entries;
 * 3. the override rows themselves.
 *
 * Source 3 is what keeps a saved override selectable when its model had no usage in the current
 * window *and* is absent from the catalog. Without it the `<select>` value has no matching
 * option, so the browser falls back and the row silently reads as `手动输入…` — the user sees a
 * price they saved turn into "custom". A provider missing from the catalog (a gateway such as
 * `kiro-auth`) is a normal case, not an error, so neither dropdown is ever disabled.
 *
 * The manual-entry escape hatch is therefore driven by explicit per-row state rather than
 * derived from "is this value known": a value the row itself contributes would otherwise
 * legitimise itself the moment the first character is typed and yank the input away.
 */
import { AlertTriangle, Info, Trash2 } from 'lucide-react'
import { useCallback, useState } from 'react'
import type { KeyboardEvent } from 'react'

import { EmptyState, ErrorState, LoadingState } from '@/components/app-state'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { ObservedModelPrice, PriceEntry } from '@/generated'
import { zh } from '@/i18n/zh'

import { CONTROL_CLASS, SettingsField } from './SettingsField'
import { priceIssues } from './usePriceOverrides'
import type {
  PriceIssue,
  PriceNumericField,
  PriceRowDraft,
  usePriceOverrides,
} from './usePriceOverrides'

type Overrides = ReturnType<typeof usePriceOverrides>

const NUMERIC_COLUMNS: { field: PriceNumericField; label: string }[] = [
  { field: 'inputPerMtok', label: zh.settings.prices.columnInput },
  { field: 'outputPerMtok', label: zh.settings.prices.columnOutput },
  { field: 'cacheReadPerMtok', label: zh.settings.prices.columnCacheRead },
  { field: 'cacheWritePerMtok', label: zh.settings.prices.columnCacheWrite },
]

const ISSUE_TEXT: Record<PriceIssue, string> = {
  blank: zh.settings.prices.invalidBlank,
  number: zh.settings.prices.invalidNumber,
  duplicate: zh.settings.prices.invalidDuplicate,
}

const CUSTOM_VALUE = '__custom__'

const PROVIDER_LABELS: Record<string, string> = {
  'amazon-bedrock': zh.settings.prices.providerAmazonBedrock,
  anthropic: zh.settings.prices.providerAnthropic,
  google: zh.settings.prices.providerGoogle,
  openai: zh.settings.prices.providerOpenAi,
}

/** Which identity column a row keeps in manual-entry mode. */
type CustomField = 'provider' | 'model'

type CustomRows = Readonly<Record<string, Partial<Record<CustomField, boolean>>>>

/** One selectable model plus the price that selecting it should copy into the row. */
interface ModelOption {
  modelId: string
  usageCount: number | null
  price: PriceEntry | null
}

function providerLabel(providerId: string): string {
  return PROVIDER_LABELS[providerId] ?? providerId
}

function isCustom(customRows: CustomRows, rowId: string, field: CustomField): boolean {
  return customRows[rowId]?.[field] === true
}

/** Rows whose value is still being typed must not feed the candidate lists. */
function candidateRows(rows: readonly PriceRowDraft[], customRows: CustomRows, field: CustomField) {
  return rows.filter((row) => !isCustom(customRows, row.rowId, field))
}

function byUsageDesc(left: ObservedModelPrice, right: ObservedModelPrice): number {
  return right.usageCount - left.usageCount || left.modelId.localeCompare(right.modelId)
}

/**
 * Observed providers first, ranked by the usage AgentLens actually archived, then catalog
 * providers by label, then providers only an existing override mentions.
 */
function providerOptions(
  entries: readonly PriceEntry[],
  observedModels: readonly ObservedModelPrice[],
  rows: readonly PriceRowDraft[],
): string[] {
  const usage = new Map<string, number>()
  for (const model of observedModels) {
    usage.set(model.providerId, (usage.get(model.providerId) ?? 0) + model.usageCount)
  }
  const observed = [...usage.entries()]
    .sort(([leftId, left], [rightId, right]) => right - left || leftId.localeCompare(rightId))
    .map(([providerId]) => providerId)

  const seen = new Set(observed)
  const catalog = [...new Set(entries.map((entry) => entry.providerId))]
    .filter((providerId) => !seen.has(providerId))
    .sort((left, right) => providerLabel(left).localeCompare(providerLabel(right)))
  for (const providerId of catalog) seen.add(providerId)

  const saved = [
    ...new Set(
      rows
        .map((row) => row.providerId.trim())
        .filter((providerId) => providerId !== '' && !seen.has(providerId)),
    ),
  ].sort((left, right) => left.localeCompare(right))

  return [...observed, ...catalog, ...saved]
}

/** Same three sources as {@link providerOptions}, scoped to one provider. */
function modelOptions(
  providerId: string,
  entries: readonly PriceEntry[],
  observedModels: readonly ObservedModelPrice[],
  rows: readonly PriceRowDraft[],
): ModelOption[] {
  const catalogByModel = new Map(
    entries
      .filter((entry) => entry.providerId === providerId)
      .map((entry) => [entry.modelId, entry]),
  )
  const options: ModelOption[] = []
  const seen = new Set<string>()

  for (const model of observedModels
    .filter((model) => model.providerId === providerId)
    .sort(byUsageDesc)) {
    if (seen.has(model.modelId)) continue
    seen.add(model.modelId)
    options.push({
      modelId: model.modelId,
      usageCount: model.usageCount,
      price: model.matchedPrice ?? catalogByModel.get(model.modelId) ?? null,
    })
  }

  for (const modelId of [...catalogByModel.keys()].sort((left, right) =>
    left.localeCompare(right),
  )) {
    if (seen.has(modelId)) continue
    seen.add(modelId)
    options.push({ modelId, usageCount: null, price: catalogByModel.get(modelId) ?? null })
  }

  const savedModels = rows
    .filter((row) => row.providerId.trim() === providerId)
    .map((row) => row.modelId.trim())
    .sort((left, right) => left.localeCompare(right))
  for (const modelId of savedModels) {
    if (modelId === '' || seen.has(modelId)) continue
    seen.add(modelId)
    options.push({ modelId, usageCount: null, price: null })
  }

  return options
}

function modelOptionLabel(option: ModelOption): string {
  return option.usageCount === null
    ? option.modelId
    : zh.settings.prices.modelWithUsage(option.modelId, option.usageCount)
}

function pricePatch(modelId: string, price: PriceEntry | null): Partial<PriceRowDraft> {
  if (price === null) return { modelId }
  return {
    modelId,
    inputPerMtok: String(price.inputPerMtok),
    outputPerMtok: String(price.outputPerMtok),
    cacheReadPerMtok: String(price.cacheReadPerMtok),
    cacheWritePerMtok: String(price.cacheWritePerMtok),
  }
}

function IdentityCells({
  row,
  index,
  providers,
  models,
  inferred,
  customProvider,
  customModel,
  setCustom,
  editRow,
}: {
  row: PriceRowDraft
  index: number
  providers: readonly string[]
  models: readonly ModelOption[]
  inferred: boolean
  customProvider: boolean
  customModel: boolean
  setCustom: (rowId: string, field: CustomField, value: boolean) => void
  editRow: Overrides['editRow']
}) {
  const providerValue = customProvider
    ? CUSTOM_VALUE
    : row.providerId.trim() === ''
      ? ''
      : row.providerId
  const modelValue = customModel ? CUSTOM_VALUE : row.modelId.trim() === '' ? '' : row.modelId

  return (
    <>
      <td className="py-2 pr-3 align-top">
        <select
          aria-label={zh.settings.prices.columnProvider}
          data-testid={`price-provider-${index}`}
          className={`${CONTROL_CLASS} w-44`}
          value={providerValue}
          onChange={(event) => {
            const providerId = event.target.value
            const custom = providerId === CUSTOM_VALUE
            setCustom(row.rowId, 'provider', custom)
            setCustom(row.rowId, 'model', false)
            editRow(row.rowId, { providerId: custom ? ' ' : providerId, modelId: '' })
          }}
        >
          <option value="">{zh.settings.prices.chooseProvider}</option>
          {providers.map((providerId) => (
            <option key={providerId} value={providerId}>
              {providerLabel(providerId)}
            </option>
          ))}
          <option value={CUSTOM_VALUE}>{zh.settings.prices.customEntry}</option>
        </select>
        {customProvider ? (
          <input
            aria-label={zh.settings.prices.customProvider}
            data-testid={`price-provider-custom-${index}`}
            className={`${CONTROL_CLASS} mt-1 w-44`}
            value={row.providerId.trim() === '' ? '' : row.providerId}
            onChange={(event) => editRow(row.rowId, { providerId: event.target.value })}
          />
        ) : null}
        {inferred ? (
          <span
            data-testid={`price-row-inferred-${index}`}
            title={zh.settings.prices.inferredHint}
            className="mt-1 inline-flex rounded-full bg-sky-500/10 px-2 py-0.5 text-xs font-medium text-sky-700 ring-1 ring-sky-500/20 dark:text-sky-300"
          >
            {zh.settings.prices.inferred}
          </span>
        ) : null}
      </td>
      <td className="py-2 pr-3 align-top">
        <select
          aria-label={zh.settings.prices.columnModel}
          data-testid={`price-model-${index}`}
          className={`${CONTROL_CLASS} w-64`}
          value={modelValue}
          onChange={(event) => {
            const modelId = event.target.value
            if (modelId === CUSTOM_VALUE) {
              setCustom(row.rowId, 'model', true)
              editRow(row.rowId, { modelId: ' ' })
              return
            }
            setCustom(row.rowId, 'model', false)
            const selected = models.find((option) => option.modelId === modelId)
            editRow(row.rowId, pricePatch(modelId, selected?.price ?? null))
          }}
        >
          <option value="">{zh.settings.prices.chooseModel}</option>
          {models.map((option) => (
            <option key={option.modelId} value={option.modelId}>
              {modelOptionLabel(option)}
            </option>
          ))}
          <option value={CUSTOM_VALUE}>{zh.settings.prices.customEntry}</option>
        </select>
        {customModel ? (
          <input
            aria-label={zh.settings.prices.customModel}
            data-testid={`price-model-custom-${index}`}
            className={`${CONTROL_CLASS} mt-1 w-64`}
            value={row.modelId.trim() === '' ? '' : row.modelId}
            onChange={(event) => editRow(row.rowId, { modelId: event.target.value })}
          />
        ) : null}
      </td>
    </>
  )
}

type ObservedStatus = 'inferred' | 'approximate' | 'unknown'

/**
 * Render order of the three groups, and the order the list pages through.
 *
 * `exact` matches are deliberately absent: a model whose price came straight from the catalog
 * needs no attention, so listing it would bury the ones that do.
 */
const OBSERVED_ORDER: readonly ObservedStatus[] = ['inferred', 'approximate', 'unknown']

const OBSERVED_PAGE_SIZE = 10

type ObservedFilter = ObservedStatus | 'all'

const OBSERVED_FILTERS: readonly ObservedFilter[] = ['all', ...OBSERVED_ORDER]

/** One classified row. `index` is its ordinal **within its own status group**, unaffected by
 * filtering, searching or paging — so a row keeps the same test id however the list is sliced. */
interface ObservedEntry {
  model: ObservedModelPrice
  status: ObservedStatus
  index: number
}

/** Stable identity of an observed model, independent of filtering, searching and paging. */
function observedKey(model: ObservedModelPrice): string {
  return `${model.providerId}\u0000${model.modelId}`
}

function observedStatusOf(model: ObservedModelPrice): ObservedStatus | null {
  switch (model.matchKind) {
    case 'crossProvider':
      return 'inferred'
    case 'normalized':
    case 'family':
      return 'approximate'
    case 'unknown':
      return 'unknown'
    default:
      return null
  }
}

function classifyObserved(models: readonly ObservedModelPrice[]): ObservedEntry[] {
  const entries: ObservedEntry[] = []
  for (const status of OBSERVED_ORDER) {
    const group = models.filter((model) => observedStatusOf(model) === status).sort(byUsageDesc)
    group.forEach((model, index) => entries.push({ model, status, index }))
  }
  return entries
}

function matchesObservedQuery(entry: ObservedEntry, query: string): boolean {
  if (query === '') return true
  const needle = query.toLowerCase()
  return (
    entry.model.providerId.toLowerCase().includes(needle) ||
    entry.model.modelId.toLowerCase().includes(needle)
  )
}

const OBSERVED_BADGE: Record<ObservedStatus, { label: string; className: string }> = {
  inferred: {
    label: zh.settings.prices.inferred,
    className: 'bg-sky-500/10 text-sky-700 ring-sky-500/20 dark:text-sky-300',
  },
  approximate: {
    label: zh.settings.prices.approximate,
    className: 'bg-amber-500/10 text-amber-700 ring-amber-500/20 dark:text-amber-300',
  },
  unknown: {
    label: zh.settings.prices.unknown,
    className: 'bg-rose-500/10 text-rose-700 ring-rose-500/20 dark:text-rose-300',
  },
}

type InlineDraft = Record<PriceNumericField, string>

/**
 * Where a fill candidate came from.
 *
 * `catalog` is the shipped official rate; `override` is a rate the user already saved. Both are
 * offered because they answer different questions — "what does the vendor charge" versus "what did
 * I decide a sibling model is worth" — and the second is the one that makes pricing a gateway's
 * pseudo-model cheap, which is the whole reason this picker exists.
 */
type FillKind = 'catalog' | 'override'

interface FillSource {
  kind: FillKind
  providerId: string
  modelId: string
  /** Index within its own kind group; stable under filtering, searching and paging. */
  index: number
  rates: InlineDraft
}

const FILL_KINDS: readonly FillKind[] = ['catalog', 'override']

const FILL_KIND_LABELS: Record<FillKind, string> = {
  catalog: zh.settings.prices.fillKindCatalog,
  override: zh.settings.prices.fillKindOverride,
}

const FILL_KIND_BADGES: Record<FillKind, string> = {
  catalog: 'bg-emerald-500/10 text-emerald-700 ring-emerald-500/20 dark:text-emerald-300',
  override: 'bg-violet-500/10 text-violet-700 ring-violet-500/20 dark:text-violet-300',
}

type FillFilter = FillKind | 'all'

const FILL_FILTERS: readonly FillFilter[] = ['all', ...FILL_KINDS]

const FILL_FILTER_LABELS: Record<FillFilter, string> = {
  all: zh.settings.prices.fillKindAll,
  ...FILL_KIND_LABELS,
}

const FILL_PAGE_SIZE = 5

function byIdentity(
  left: { providerId: string; modelId: string },
  right: { providerId: string; modelId: string },
): number {
  return (
    left.providerId.localeCompare(right.providerId) || left.modelId.localeCompare(right.modelId)
  )
}

/**
 * Catalog entries first, then saved override rows.
 *
 * A provider/model pair present in both is deliberately listed twice: the two sources can disagree
 * (that disagreement is precisely what an override *is*), so collapsing them would hide one of the
 * two numbers the user is choosing between. The kind badge tells them apart.
 *
 * Override rows keep their rates as the raw strings the table holds, so a row that is mid-edit and
 * currently invalid fills an invalid value — which `inlineIssues` then rejects, exactly as it would
 * for a typed one. Blank identities are dropped: an empty new row is not a price.
 */
function fillSources(entries: readonly PriceEntry[], rows: readonly PriceRowDraft[]): FillSource[] {
  const catalog = [...entries].sort(byIdentity).map((entry, index) => ({
    kind: 'catalog' as const,
    providerId: entry.providerId,
    modelId: entry.modelId,
    index,
    rates: {
      inputPerMtok: String(entry.inputPerMtok),
      outputPerMtok: String(entry.outputPerMtok),
      cacheReadPerMtok: String(entry.cacheReadPerMtok),
      cacheWritePerMtok: String(entry.cacheWritePerMtok),
    },
  }))

  const override = rows
    .filter((row) => row.providerId.trim() !== '' && row.modelId.trim() !== '')
    .map((row) => ({
      providerId: row.providerId.trim(),
      modelId: row.modelId.trim(),
      rates: {
        inputPerMtok: row.inputPerMtok,
        outputPerMtok: row.outputPerMtok,
        cacheReadPerMtok: row.cacheReadPerMtok,
        cacheWritePerMtok: row.cacheWritePerMtok,
      },
    }))
    .sort(byIdentity)
    .map((row, index) => ({ kind: 'override' as const, ...row, index }))

  return [...catalog, ...override]
}

function matchesFillQuery(source: FillSource, query: string): boolean {
  if (query === '') return true
  const needle = query.toLowerCase()
  return (
    source.providerId.toLowerCase().includes(needle) ||
    source.modelId.toLowerCase().includes(needle)
  )
}

function sameRates(left: InlineDraft, right: InlineDraft): boolean {
  return NUMERIC_COLUMNS.every((column) => left[column.field] === right[column.field])
}

/**
 * The fill picker nested inside the inline rate form.
 *
 * Filter and query changes reset the page to 1 for the same reason the observed list does: a stale
 * page index on a narrowed list reads as "no matches".
 */
function PriceFillPicker({
  testId,
  sources,
  onApply,
  onClose,
}: {
  testId: string
  sources: readonly FillSource[]
  onApply: (source: FillSource) => void
  onClose: () => void
}) {
  const [filter, setFilter] = useState<FillFilter>('all')
  const [query, setQuery] = useState('')
  const [page, setPage] = useState(1)

  const matched = sources.filter(
    (source) => (filter === 'all' || source.kind === filter) && matchesFillQuery(source, query),
  )
  const pageCount = Math.max(1, Math.ceil(matched.length / FILL_PAGE_SIZE))
  const currentPage = Math.min(page, pageCount)
  const paged = matched.slice((currentPage - 1) * FILL_PAGE_SIZE, currentPage * FILL_PAGE_SIZE)

  /**
   * The surrounding form reads `Enter` as save and `Escape` as collapse. Inside a search box both
   * mean something else entirely — `Enter` would save a half-filled row while the user is still
   * hunting for a price — so the picker consumes them and maps `Escape` to closing itself.
   */
  function swallowFormKeys(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === 'Enter') {
      event.stopPropagation()
      return
    }
    if (event.key === 'Escape') {
      event.stopPropagation()
      onClose()
    }
  }

  return (
    <div
      data-testid={`${testId}-fill-panel`}
      className="flex flex-col gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2"
      onKeyDown={swallowFormKeys}
    >
      <span className="text-xs text-muted-foreground select-none">
        {zh.settings.prices.fillHint}
      </span>
      <div className="flex flex-wrap items-end gap-3">
        <SettingsField id={`${testId}-fill-kind`} label={zh.settings.prices.fillKindLabel}>
          <select
            id={`${testId}-fill-kind`}
            data-testid={`${testId}-fill-kind`}
            className={`${CONTROL_CLASS} w-32`}
            value={filter}
            onChange={(event) => {
              setFilter(event.target.value as FillFilter)
              setPage(1)
            }}
          >
            {FILL_FILTERS.map((value) => (
              <option key={value} value={value}>
                {FILL_FILTER_LABELS[value]}
              </option>
            ))}
          </select>
        </SettingsField>
        <SettingsField id={`${testId}-fill-search`} label={zh.settings.prices.fillSearchLabel}>
          <input
            id={`${testId}-fill-search`}
            data-testid={`${testId}-fill-search`}
            type="search"
            className={`${CONTROL_CLASS} w-56`}
            placeholder={zh.settings.prices.fillSearchPlaceholder}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value)
              setPage(1)
            }}
          />
        </SettingsField>
        {query === '' ? null : (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            data-testid={`${testId}-fill-clear-search`}
            onClick={() => {
              setQuery('')
              setPage(1)
            }}
          >
            {zh.settings.prices.fillClearSearch}
          </Button>
        )}
        <div className="ml-auto flex items-center gap-2">
          <span data-testid={`${testId}-fill-total`} className="text-xs text-muted-foreground">
            {zh.settings.prices.fillTotal(matched.length, sources.length)}
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            data-testid={`${testId}-fill-prev`}
            disabled={currentPage <= 1}
            onClick={() => setPage(currentPage - 1)}
          >
            {zh.settings.prices.fillPrevPage}
          </Button>
          <span
            data-testid={`${testId}-fill-page`}
            className="text-xs tabular-nums text-muted-foreground"
          >
            {zh.settings.prices.fillPage(currentPage, pageCount)}
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            data-testid={`${testId}-fill-next`}
            disabled={currentPage >= pageCount}
            onClick={() => setPage(currentPage + 1)}
          >
            {zh.settings.prices.fillNextPage}
          </Button>
        </div>
      </div>
      {paged.length === 0 ? (
        <div data-testid={`${testId}-fill-empty`}>
          <EmptyState
            label={
              sources.length === 0 ? zh.settings.prices.fillEmpty : zh.settings.prices.fillNoMatch
            }
          />
        </div>
      ) : (
        paged.map((source) => (
          <div
            key={`${source.kind}-${source.index}`}
            data-testid={`${testId}-fill-option-${source.kind}-${source.index}`}
            className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-background px-3 py-2"
          >
            <span
              className={`rounded-full px-2 py-0.5 text-xs font-medium ring-1 ${FILL_KIND_BADGES[source.kind]}`}
            >
              {FILL_KIND_LABELS[source.kind]}
            </span>
            <span className="font-mono text-xs select-text">
              {source.providerId} / {source.modelId}
            </span>
            <span className="text-xs text-muted-foreground select-text">
              {zh.settings.prices.fillRateSummary(
                source.rates.inputPerMtok,
                source.rates.outputPerMtok,
                source.rates.cacheReadPerMtok,
                source.rates.cacheWritePerMtok,
              )}
            </span>
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="ml-auto"
              data-testid={`${testId}-fill-apply-${source.kind}-${source.index}`}
              onClick={() => onApply(source)}
            >
              {zh.settings.prices.fillApply}
            </Button>
          </div>
        ))
      )}
    </div>
  )
}

function inlineDraftFor(model: ObservedModelPrice): InlineDraft {
  const matched = model.matchedPrice
  return {
    inputPerMtok: String(matched?.inputPerMtok ?? 0),
    outputPerMtok: String(matched?.outputPerMtok ?? 0),
    cacheReadPerMtok: String(matched?.cacheReadPerMtok ?? 0),
    cacheWritePerMtok: String(matched?.cacheWritePerMtok ?? 0),
  }
}

/**
 * Validates an inline draft with the **same** rules the table rows use.
 *
 * `priceIssues` is reused rather than reimplemented so the inline form cannot accept a value the
 * table would reject; the identity fields come from the observed model, so `blank` and `duplicate`
 * are unreachable here and only the numeric verdict can fire.
 */
function inlineIssues(model: ObservedModelPrice, draft: InlineDraft): PriceIssue[] {
  return priceIssues([
    {
      rowId: 'inline',
      providerId: model.providerId,
      modelId: model.modelId,
      extra: {},
      ...draft,
    },
  ])
}

/**
 * The inline rate form that expands directly beneath the row the user clicked.
 *
 * Appending straight to the price table below was the previous behaviour and it left the user
 * hunting for the row they had just created — on a paged list that row is often off-screen. The
 * expansion keeps the edit where the click happened.
 *
 * `Escape` collapses and `Enter` saves, so the form is completable without ever reaching for the
 * mouse. The rate inputs are ordinary `<input>` elements, which keeps the native context menu
 * (the only pointer-driven paste affordance) — `contextMenuGuard` exempts editable targets.
 */
function InlinePriceForm({
  model,
  status,
  index,
  sources,
  onCancel,
  onSave,
}: {
  model: ObservedModelPrice
  status: ObservedStatus
  index: number
  sources: readonly FillSource[]
  onCancel: () => void
  onSave: (draft: InlineDraft) => void
}) {
  const [draft, setDraft] = useState<InlineDraft>(() => inlineDraftFor(model))
  const [pickerOpen, setPickerOpen] = useState(false)
  /**
   * The applied source plus the draft it replaced.
   *
   * `before` is what makes the fill undoable, which matters because the pre-fill draft may be the
   * `matchedPrice` prefill the row opened with — information the user cannot retype once it is gone.
   */
  const [filled, setFilled] = useState<{ source: FillSource; before: InlineDraft } | null>(null)
  const issues = inlineIssues(model, draft)
  const testId = `price-observed-inline-${status}-${index}`

  function commit() {
    if (issues.length === 0) onSave(draft)
  }

  function editRate(field: PriceNumericField, value: string) {
    setDraft((current) => ({ ...current, [field]: value }))
  }

  return (
    <div
      data-testid={testId}
      className="flex flex-col gap-2 rounded-lg border border-border border-dashed bg-background px-3 py-2"
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.stopPropagation()
          onCancel()
          return
        }
        if (event.key === 'Enter') {
          event.stopPropagation()
          commit()
        }
      }}
    >
      <span className="font-mono text-xs text-muted-foreground select-text">
        {model.providerId} / {model.modelId}
      </span>
      <div className="flex flex-wrap items-end gap-3">
        {NUMERIC_COLUMNS.map((column) => {
          const inputId = `${testId}-${column.field}`
          return (
            <SettingsField key={column.field} id={inputId} label={column.label}>
              <input
                id={inputId}
                data-testid={inputId}
                type="number"
                min={0}
                step="any"
                className={`${CONTROL_CLASS} w-24`}
                value={draft[column.field]}
                onChange={(event) => editRate(column.field, event.target.value)}
              />
            </SettingsField>
          )
        })}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          size="sm"
          variant="outline"
          aria-expanded={pickerOpen}
          data-testid={`${testId}-fill-toggle`}
          onClick={() => setPickerOpen((current) => !current)}
        >
          {pickerOpen ? zh.settings.prices.fillCollapse : zh.settings.prices.fillTitle}
        </Button>
        {filled === null ? null : (
          <>
            <span
              data-testid={`${testId}-fill-origin`}
              className="text-xs text-muted-foreground select-text"
            >
              {zh.settings.prices.fillOrigin(
                FILL_KIND_LABELS[filled.source.kind],
                filled.source.providerId,
                filled.source.modelId,
              )}
            </span>
            {sameRates(draft, filled.source.rates) ? null : (
              <span
                data-testid={`${testId}-fill-adjusted`}
                className="rounded-full bg-amber-500/10 px-2 py-0.5 text-xs font-medium text-amber-700 ring-1 ring-amber-500/20 dark:text-amber-300"
              >
                {zh.settings.prices.fillAdjusted}
              </span>
            )}
            <Button
              type="button"
              size="sm"
              variant="ghost"
              data-testid={`${testId}-fill-undo`}
              onClick={() => {
                setDraft(filled.before)
                setFilled(null)
              }}
            >
              {zh.settings.prices.fillUndo}
            </Button>
          </>
        )}
      </div>
      {pickerOpen ? (
        <PriceFillPicker
          testId={testId}
          sources={sources}
          onClose={() => setPickerOpen(false)}
          onApply={(source) => {
            setFilled((current) => ({ source, before: current?.before ?? draft }))
            setDraft(source.rates)
          }}
        />
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          size="sm"
          data-testid={`${testId}-save`}
          disabled={issues.length > 0}
          onClick={commit}
        >
          {zh.settings.prices.inlineSave}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          data-testid={`${testId}-cancel`}
          onClick={onCancel}
        >
          {zh.settings.prices.inlineCancel}
        </Button>
        {issues.map((issue) => (
          <span
            key={issue}
            data-testid={`price-issue-${issue}`}
            className="text-xs text-destructive"
          >
            {ISSUE_TEXT[issue]}
          </span>
        ))}
        <span className="ml-auto text-xs text-muted-foreground select-none">
          {zh.settings.prices.inlineKeyboardHint}
        </span>
      </div>
    </div>
  )
}

function ObservedPriceRow({
  model,
  status,
  index,
  expanded,
  sources,
  onToggle,
  onSave,
}: {
  model: ObservedModelPrice
  status: ObservedStatus
  index: number
  expanded: boolean
  sources: readonly FillSource[]
  onToggle: () => void
  onSave: (draft: InlineDraft) => void
}) {
  const badge = OBSERVED_BADGE[status]
  const Icon = status === 'inferred' ? Info : AlertTriangle

  return (
    <div className="flex flex-col gap-1">
      <div
        data-testid={`price-observed-${status}-${index}`}
        data-expanded={String(expanded)}
        className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2"
      >
        <Icon
          aria-hidden
          className={status === 'inferred' ? 'size-4 text-sky-600' : 'size-4 text-amber-600'}
        />
        <span
          title={status === 'inferred' ? zh.settings.prices.inferredHint : undefined}
          className={`rounded-full px-2 py-0.5 text-xs font-medium ring-1 ${badge.className}`}
        >
          {badge.label}
        </span>
        <span className="font-mono text-xs select-text">
          {model.providerId} / {model.modelId}
        </span>
        <span className="text-xs text-muted-foreground select-text">
          {model.usageCount} {zh.settings.prices.usageCount}
        </span>
        {model.matchedPrice === null ? null : (
          <span className="text-xs text-muted-foreground select-text">
            {zh.settings.prices.matchedTo}: {model.matchedPrice.providerId} /{' '}
            {model.matchedPrice.modelId}
          </span>
        )}
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="ml-auto"
          aria-expanded={expanded}
          data-testid={`price-observed-add-${status}-${index}`}
          onClick={onToggle}
        >
          {expanded ? zh.settings.prices.inlineCollapse : zh.settings.prices.addObserved}
        </Button>
      </div>
      {expanded ? (
        <InlinePriceForm
          model={model}
          status={status}
          index={index}
          sources={sources}
          onCancel={onToggle}
          onSave={onSave}
        />
      ) : null}
    </div>
  )
}

const OBSERVED_FILTER_LABELS: Record<ObservedFilter, string> = {
  all: zh.settings.prices.observedFilterAll,
  inferred: zh.settings.prices.inferred,
  approximate: zh.settings.prices.approximate,
  unknown: zh.settings.prices.unknown,
}

/**
 * Filter, search and paging controls for the observed-model list.
 *
 * Changing the filter or the query resets the page to 1. Without that the user lands on a page
 * index the narrowed list no longer has and reads an empty list as "no matches" — the single most
 * common pagination defect, and the reason the reset lives in the handlers rather than in a
 * clamp: a clamp would silently move them to the *last* page instead of the first.
 */
function ObservedControls({
  filter,
  query,
  matched,
  total,
  page,
  pageCount,
  onFilter,
  onQuery,
  onPage,
}: {
  filter: ObservedFilter
  query: string
  matched: number
  total: number
  page: number
  pageCount: number
  onFilter: (value: ObservedFilter) => void
  onQuery: (value: string) => void
  onPage: (value: number) => void
}) {
  return (
    <div className="flex flex-wrap items-end gap-3">
      <SettingsField id="price-observed-filter" label={zh.settings.prices.observedFilterLabel}>
        <select
          id="price-observed-filter"
          data-testid="price-observed-filter"
          className={`${CONTROL_CLASS} w-32`}
          value={filter}
          onChange={(event) => onFilter(event.target.value as ObservedFilter)}
        >
          {OBSERVED_FILTERS.map((value) => (
            <option key={value} value={value}>
              {OBSERVED_FILTER_LABELS[value]}
            </option>
          ))}
        </select>
      </SettingsField>
      <SettingsField id="price-observed-search" label={zh.settings.prices.observedSearchLabel}>
        <input
          id="price-observed-search"
          data-testid="price-observed-search"
          type="search"
          className={`${CONTROL_CLASS} w-56`}
          placeholder={zh.settings.prices.observedSearchPlaceholder}
          value={query}
          onChange={(event) => onQuery(event.target.value)}
        />
      </SettingsField>
      {query === '' ? null : (
        <Button
          type="button"
          size="sm"
          variant="ghost"
          data-testid="price-observed-clear-search"
          onClick={() => onQuery('')}
        >
          {zh.settings.prices.observedClearSearch}
        </Button>
      )}
      <div className="ml-auto flex items-center gap-2">
        <span data-testid="price-observed-total" className="text-xs text-muted-foreground">
          {zh.settings.prices.observedTotal(matched, total)}
        </span>
        <Button
          type="button"
          size="sm"
          variant="outline"
          data-testid="price-observed-prev"
          disabled={page <= 1}
          onClick={() => onPage(page - 1)}
        >
          {zh.settings.prices.observedPrevPage}
        </Button>
        <span
          data-testid="price-observed-page"
          className="text-xs tabular-nums text-muted-foreground"
        >
          {zh.settings.prices.observedPage(page, pageCount)}
        </span>
        <Button
          type="button"
          size="sm"
          variant="outline"
          data-testid="price-observed-next"
          disabled={page >= pageCount}
          onClick={() => onPage(page + 1)}
        >
          {zh.settings.prices.observedNextPage}
        </Button>
      </div>
    </div>
  )
}

export function PriceOverrideEditor({ overrides }: { overrides: Overrides }) {
  const [customRows, setCustomRows] = useState<CustomRows>({})
  const [observedFilter, setObservedFilter] = useState<ObservedFilter>('all')
  const [observedQuery, setObservedQuery] = useState('')
  const [observedPage, setObservedPage] = useState(1)
  /**
   * Identity of the single expanded row, or `null`.
   *
   * One key rather than a set: two open forms mean two unsaved drafts competing for the same
   * price table, and the user has no way to tell which one a save applied to. Opening a row
   * therefore closes any other.
   */
  const [expandedKey, setExpandedKey] = useState<string | null>(null)
  const setCustom = useCallback((rowId: string, field: CustomField, value: boolean) => {
    setCustomRows((current) => ({ ...current, [rowId]: { ...current[rowId], [field]: value } }))
  }, [])

  const entries = overrides.catalog?.entries ?? []
  const observedModels = overrides.catalog?.observedModels ?? []
  const observedEntries = classifyObserved(observedModels)
  const matchedEntries = observedEntries.filter(
    (entry) =>
      (observedFilter === 'all' || entry.status === observedFilter) &&
      matchesObservedQuery(entry, observedQuery),
  )
  const observedPageCount = Math.max(1, Math.ceil(matchedEntries.length / OBSERVED_PAGE_SIZE))
  /**
   * The handlers reset to page 1; this clamp only covers a page that went out of range because
   * the catalog itself changed under a refetch, which no handler observes.
   */
  const currentObservedPage = Math.min(observedPage, observedPageCount)
  const pagedEntries = matchedEntries.slice(
    (currentObservedPage - 1) * OBSERVED_PAGE_SIZE,
    currentObservedPage * OBSERVED_PAGE_SIZE,
  )
  const inferredProviders = new Set(
    observedEntries
      .filter((entry) => entry.status === 'inferred')
      .map((entry) => entry.model.providerId),
  )

  const providers = providerOptions(
    entries,
    observedModels,
    candidateRows(overrides.rows, customRows, 'provider'),
  )
  const modelCandidateRows = candidateRows(overrides.rows, customRows, 'model')
  const sources = fillSources(entries, overrides.rows)

  return (
    <Card data-testid="settings-prices">
      <CardHeader>
        <CardTitle>{zh.settings.prices.title}</CardTitle>
        <CardDescription>{zh.settings.prices.description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {overrides.error !== null ? (
          <ErrorState error={overrides.error} onRetry={overrides.refetch} />
        ) : overrides.isPending ? (
          <LoadingState />
        ) : (
          <>
            {overrides.catalog === undefined ? null : (
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
                <span data-testid="price-catalog-version">
                  {zh.settings.prices.catalogVersion}: {overrides.catalog.catalogVersion}
                </span>
                <span>
                  {zh.settings.prices.catalogUpdated}: {overrides.catalog.updatedAt}
                </span>
                <span>{zh.settings.prices.catalogOffline}</span>
              </div>
            )}

            {observedEntries.length === 0 ? null : (
              <div data-testid="price-observed" className="flex flex-col gap-2">
                <div>
                  <h3 className="text-sm font-medium">{zh.settings.prices.observedTitle}</h3>
                  <p className="text-xs text-muted-foreground">
                    {zh.settings.prices.observedDescription}
                  </p>
                </div>
                <ObservedControls
                  filter={observedFilter}
                  query={observedQuery}
                  matched={matchedEntries.length}
                  total={observedEntries.length}
                  page={currentObservedPage}
                  pageCount={observedPageCount}
                  onFilter={(value) => {
                    setObservedFilter(value)
                    setObservedPage(1)
                  }}
                  onQuery={(value) => {
                    setObservedQuery(value)
                    setObservedPage(1)
                  }}
                  onPage={setObservedPage}
                />
                {pagedEntries.length === 0 ? (
                  <div data-testid="price-observed-empty">
                    <EmptyState label={zh.settings.prices.observedNoMatch} />
                  </div>
                ) : (
                  pagedEntries.map((entry) => {
                    const key = observedKey(entry.model)
                    return (
                      <ObservedPriceRow
                        key={key}
                        model={entry.model}
                        status={entry.status}
                        index={entry.index}
                        expanded={expandedKey === key}
                        sources={sources}
                        onToggle={() => setExpandedKey((current) => (current === key ? null : key))}
                        onSave={(draft) => {
                          overrides.addObservedModel(entry.model, draft)
                          setExpandedKey(null)
                        }}
                      />
                    )
                  })
                )}
              </div>
            )}

            {overrides.rows.length === 0 ? (
              <EmptyState label={zh.settings.prices.empty} />
            ) : (
              <div className="overflow-x-auto">
                <table
                  data-testid="price-table"
                  className="w-full border-collapse text-left text-sm"
                >
                  <thead>
                    <tr className="border-b border-border text-xs text-muted-foreground">
                      <th scope="col" className="py-2 pr-3 font-medium">
                        {zh.settings.prices.columnProvider}
                      </th>
                      <th scope="col" className="py-2 pr-3 font-medium">
                        {zh.settings.prices.columnModel}
                      </th>
                      {NUMERIC_COLUMNS.map((column) => (
                        <th key={column.field} scope="col" className="py-2 pr-3 font-medium">
                          {column.label}
                        </th>
                      ))}
                      <th scope="col" className="py-2 font-medium">
                        {zh.settings.prices.columnActions}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {overrides.rows.map((row, index) => (
                      <tr
                        key={row.rowId}
                        data-testid={`price-row-${index}`}
                        className="border-b border-border/60 last:border-b-0"
                      >
                        <IdentityCells
                          row={row}
                          index={index}
                          providers={providers}
                          models={modelOptions(
                            row.providerId.trim(),
                            entries,
                            observedModels,
                            modelCandidateRows,
                          )}
                          inferred={inferredProviders.has(row.providerId.trim())}
                          customProvider={isCustom(customRows, row.rowId, 'provider')}
                          customModel={isCustom(customRows, row.rowId, 'model')}
                          setCustom={setCustom}
                          editRow={overrides.editRow}
                        />
                        {NUMERIC_COLUMNS.map((column) => (
                          <td key={column.field} className="py-2 pr-3">
                            <input
                              aria-label={column.label}
                              data-testid={`price-${column.field}-${index}`}
                              type="number"
                              min={0}
                              step="any"
                              className={`${CONTROL_CLASS} w-24`}
                              value={row[column.field]}
                              onChange={(event) =>
                                overrides.editRow(row.rowId, {
                                  [column.field]: event.target.value,
                                })
                              }
                            />
                          </td>
                        ))}
                        <td className="py-2">
                          <Button
                            type="button"
                            size="icon-sm"
                            variant="ghost"
                            aria-label={zh.settings.prices.deleteRow}
                            data-testid={`price-delete-${index}`}
                            onClick={() => overrides.deleteRow(row.rowId)}
                          >
                            <Trash2 aria-hidden="true" />
                          </Button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            <div className="flex flex-wrap items-center gap-3">
              <Button
                type="button"
                size="sm"
                variant="outline"
                data-testid="price-add"
                onClick={overrides.addRow}
              >
                {zh.settings.prices.addRow}
              </Button>
              <Button
                type="button"
                size="sm"
                data-testid="price-save"
                disabled={!overrides.dirty || overrides.issues.length > 0 || overrides.isSaving}
                onClick={overrides.submit}
              >
                {zh.settings.prices.save}
              </Button>
              {overrides.saved ? (
                <span data-testid="price-saved" className="text-xs text-muted-foreground">
                  {zh.settings.prices.saved}
                </span>
              ) : null}
              {overrides.issues.map((issue) => (
                <span
                  key={issue}
                  data-testid={`price-issue-${issue}`}
                  className="text-xs text-destructive"
                >
                  {ISSUE_TEXT[issue]}
                </span>
              ))}
            </div>

            <div className="flex flex-col gap-1 text-xs text-muted-foreground">
              <span>{zh.settings.prices.variantHint}</span>
              <span>{zh.settings.prices.reasoningHint}</span>
            </div>
          </>
        )}
      </CardContent>
    </Card>
  )
}
