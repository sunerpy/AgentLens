/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Manual price-override table: add / edit / delete rows, saved through `prices_set`.
 */
import { AlertTriangle, Trash2 } from 'lucide-react'

import { EmptyState, ErrorState, LoadingState } from '@/components/app-state'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { ObservedModelPrice, PriceEntry } from '@/generated'
import { zh } from '@/i18n/zh'

import { CONTROL_CLASS } from './SettingsField'
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

function providerLabel(providerId: string): string {
  return PROVIDER_LABELS[providerId] ?? providerId
}

function pricePatch(entry: PriceEntry): Partial<PriceRowDraft> {
  return {
    modelId: entry.modelId,
    inputPerMtok: String(entry.inputPerMtok),
    outputPerMtok: String(entry.outputPerMtok),
    cacheReadPerMtok: String(entry.cacheReadPerMtok),
    cacheWritePerMtok: String(entry.cacheWritePerMtok),
  }
}

function IdentityCells({
  row,
  index,
  entries,
  editRow,
}: {
  row: PriceRowDraft
  index: number
  entries: PriceEntry[]
  editRow: Overrides['editRow']
}) {
  const providers = [...new Set(entries.map((entry) => entry.providerId))].sort((left, right) =>
    providerLabel(left).localeCompare(providerLabel(right)),
  )
  const providerIsKnown = providers.includes(row.providerId)
  const providerValue = providerIsKnown ? row.providerId : row.providerId === '' ? '' : CUSTOM_VALUE
  const models = providerIsKnown
    ? entries.filter((entry) => entry.providerId === row.providerId)
    : []
  const modelIsKnown = models.some((entry) => entry.modelId === row.modelId)
  const modelValue = modelIsKnown ? row.modelId : row.modelId === '' ? '' : CUSTOM_VALUE

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
            editRow(row.rowId, {
              providerId: providerId === CUSTOM_VALUE ? ' ' : providerId,
              modelId: '',
            })
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
        {providerValue === CUSTOM_VALUE ? (
          <input
            aria-label={zh.settings.prices.customProvider}
            data-testid={`price-provider-custom-${index}`}
            className={`${CONTROL_CLASS} mt-1 w-44`}
            value={row.providerId.trim() === '' ? '' : row.providerId}
            onChange={(event) => editRow(row.rowId, { providerId: event.target.value })}
          />
        ) : null}
      </td>
      <td className="py-2 pr-3 align-top">
        {providerValue === CUSTOM_VALUE ? (
          <input
            aria-label={zh.settings.prices.customModel}
            data-testid={`price-model-custom-${index}`}
            className={`${CONTROL_CLASS} w-64`}
            value={row.modelId.trim() === '' ? '' : row.modelId}
            onChange={(event) => editRow(row.rowId, { modelId: event.target.value })}
          />
        ) : (
          <>
            <select
              aria-label={zh.settings.prices.columnModel}
              data-testid={`price-model-${index}`}
              className={`${CONTROL_CLASS} w-64`}
              disabled={!providerIsKnown}
              value={modelValue}
              onChange={(event) => {
                const modelId = event.target.value
                if (modelId === CUSTOM_VALUE) {
                  editRow(row.rowId, { modelId: ' ' })
                  return
                }
                const selected = models.find((entry) => entry.modelId === modelId)
                editRow(row.rowId, selected === undefined ? { modelId } : pricePatch(selected))
              }}
            >
              <option value="">{zh.settings.prices.chooseModel}</option>
              {models.map((entry) => (
                <option key={entry.modelId} value={entry.modelId}>
                  {entry.modelId}
                </option>
              ))}
              <option value={CUSTOM_VALUE}>{zh.settings.prices.customEntry}</option>
            </select>
            {modelValue === CUSTOM_VALUE ? (
              <input
                aria-label={zh.settings.prices.customModel}
                data-testid={`price-model-custom-${index}`}
                className={`${CONTROL_CLASS} mt-1 w-64`}
                value={row.modelId.trim() === '' ? '' : row.modelId}
                onChange={(event) => editRow(row.rowId, { modelId: event.target.value })}
              />
            ) : null}
          </>
        )}
      </td>
    </>
  )
}

function ObservedPriceRow({
  model,
  status,
  index,
  onAdd,
}: {
  model: ObservedModelPrice
  status: 'approximate' | 'unknown'
  index: number
  onAdd: (model: ObservedModelPrice) => void
}) {
  return (
    <div
      data-testid={`price-observed-${status}-${index}`}
      className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2"
    >
      <AlertTriangle aria-hidden className="size-4 text-amber-600" />
      <span className="rounded-full bg-amber-500/10 px-2 py-0.5 text-xs font-medium text-amber-700 ring-1 ring-amber-500/20 dark:text-amber-300">
        {status === 'unknown' ? zh.settings.prices.unknown : zh.settings.prices.approximate}
      </span>
      <span className="font-mono text-xs">
        {model.providerId} / {model.modelId}
      </span>
      <span className="text-xs text-muted-foreground">
        {model.usageCount} {zh.settings.prices.usageCount}
      </span>
      {model.matchedPrice === null ? null : (
        <span className="text-xs text-muted-foreground">
          {zh.settings.prices.matchedTo}: {model.matchedPrice.providerId} /{' '}
          {model.matchedPrice.modelId}
        </span>
      )}
      <Button
        type="button"
        size="sm"
        variant="outline"
        className="ml-auto"
        data-testid={`price-observed-add-${status}-${index}`}
        onClick={() => onAdd(model)}
      >
        {zh.settings.prices.addObserved}
      </Button>
    </div>
  )
}

export function PriceOverrideEditor({ overrides }: { overrides: Overrides }) {
  const entries = overrides.catalog?.entries ?? []
  const approximateModels =
    overrides.catalog?.observedModels.filter(
      (model) => model.matchKind === 'normalized' || model.matchKind === 'family',
    ) ?? []
  const unknownModels =
    overrides.catalog?.observedModels.filter((model) => model.matchKind === 'unknown') ?? []

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

            {approximateModels.length === 0 && unknownModels.length === 0 ? null : (
              <div className="flex flex-col gap-2">
                <div>
                  <h3 className="text-sm font-medium">{zh.settings.prices.observedTitle}</h3>
                  <p className="text-xs text-muted-foreground">
                    {zh.settings.prices.observedDescription}
                  </p>
                </div>
                {approximateModels.map((model, index) => (
                  <ObservedPriceRow
                    key={`${model.providerId}\u0000${model.modelId}`}
                    model={model}
                    status="approximate"
                    index={index}
                    onAdd={overrides.addObservedModel}
                  />
                ))}
                {unknownModels.map((model, index) => (
                  <ObservedPriceRow
                    key={`${model.providerId}\u0000${model.modelId}`}
                    model={model}
                    status="unknown"
                    index={index}
                    onAdd={overrides.addObservedModel}
                  />
                ))}
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
                          entries={entries}
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
