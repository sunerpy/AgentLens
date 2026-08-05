/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Manual price-override table: add / edit / delete rows, saved through `prices_set`.
 */
import { Trash2 } from 'lucide-react'

import { EmptyState, ErrorState, LoadingState } from '@/components/app-state'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { zh } from '@/i18n/zh'

import { CONTROL_CLASS } from './SettingsField'
import type { PriceIssue, PriceNumericField, usePriceOverrides } from './usePriceOverrides'

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

export function PriceOverrideEditor({ overrides }: { overrides: Overrides }) {
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
                        <td className="py-2 pr-3">
                          <input
                            aria-label={zh.settings.prices.columnProvider}
                            data-testid={`price-provider-${index}`}
                            className={`${CONTROL_CLASS} w-40`}
                            value={row.providerId}
                            onChange={(event) =>
                              overrides.editRow(row.rowId, { providerId: event.target.value })
                            }
                          />
                        </td>
                        <td className="py-2 pr-3">
                          <input
                            aria-label={zh.settings.prices.columnModel}
                            data-testid={`price-model-${index}`}
                            className={`${CONTROL_CLASS} w-48`}
                            value={row.modelId}
                            onChange={(event) =>
                              overrides.editRow(row.rowId, { modelId: event.target.value })
                            }
                          />
                        </td>
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
