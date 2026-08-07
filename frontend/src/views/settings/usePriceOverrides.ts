/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Price-override editor state. Rows are edited as strings so a partially typed number never
 * collapses to `NaN`, then validated and converted once on save.
 *
 * The table is written through the `prices_set` command, which hands the payload to the core
 * `PriceTable::save` atomic rename. This module never touches `prices.json` itself. Per-entry
 * and document-level unknown fields ride along in `extra` so a round-trip cannot drop
 * provenance a future importer may add.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useState } from 'react'

import type { ObservedModelPrice, PriceEntry, PriceTable } from '@/generated'
import { priceCatalogGet, pricesGet, pricesSet } from '@/lib/ipc'

export const PRICES_QUERY_KEY = ['settings', 'prices'] as const
export const PRICE_CATALOG_QUERY_KEY = ['settings', 'price-catalog'] as const

const NUMERIC_FIELDS = [
  'inputPerMtok',
  'outputPerMtok',
  'cacheReadPerMtok',
  'cacheWritePerMtok',
] as const

export type PriceNumericField = (typeof NUMERIC_FIELDS)[number]

export interface PriceRowDraft {
  rowId: string
  providerId: string
  modelId: string
  inputPerMtok: string
  outputPerMtok: string
  cacheReadPerMtok: string
  cacheWritePerMtok: string
  extra: PriceEntry['extra']
}

export type PriceIssue = 'blank' | 'number' | 'duplicate'

function toRow(entry: PriceEntry, index: number): PriceRowDraft {
  return {
    rowId: `${index}-${entry.providerId}-${entry.modelId}`,
    providerId: entry.providerId,
    modelId: entry.modelId,
    inputPerMtok: String(entry.inputPerMtok),
    outputPerMtok: String(entry.outputPerMtok),
    cacheReadPerMtok: String(entry.cacheReadPerMtok),
    cacheWritePerMtok: String(entry.cacheWritePerMtok),
    extra: entry.extra,
  }
}

/** Mirrors `PricingError::{BlankIdentifier, InvalidPrice, DuplicateEntry}` so the user is told
 * what is wrong before a doomed IPC round-trip; the Rust layer re-checks all three. */
export function priceIssues(rows: PriceRowDraft[]): PriceIssue[] {
  const issues = new Set<PriceIssue>()
  const seen = new Set<string>()
  for (const row of rows) {
    if (row.providerId.trim() === '' || row.modelId.trim() === '') issues.add('blank')
    for (const field of NUMERIC_FIELDS) {
      const parsed = Number.parseFloat(row[field].trim())
      if (!Number.isFinite(parsed) || parsed < 0) issues.add('number')
    }
    const key = `${row.providerId.trim()}\u0000${row.modelId.trim()}`
    if (seen.has(key)) issues.add('duplicate')
    seen.add(key)
  }
  return [...issues]
}

function toTable(rows: PriceRowDraft[], loaded: PriceTable): PriceTable {
  return {
    schemaVersion: loaded.schemaVersion,
    extra: loaded.extra,
    entries: rows.map((row) => ({
      providerId: row.providerId.trim(),
      modelId: row.modelId.trim(),
      inputPerMtok: Number.parseFloat(row.inputPerMtok),
      outputPerMtok: Number.parseFloat(row.outputPerMtok),
      cacheReadPerMtok: Number.parseFloat(row.cacheReadPerMtok),
      cacheWritePerMtok: Number.parseFloat(row.cacheWritePerMtok),
      extra: row.extra,
    })),
  }
}

export function usePriceOverrides() {
  const queryClient = useQueryClient()
  const prices = useQuery({ queryKey: PRICES_QUERY_KEY, queryFn: pricesGet })
  const catalog = useQuery({ queryKey: PRICE_CATALOG_QUERY_KEY, queryFn: priceCatalogGet })
  const [draft, setDraft] = useState<PriceRowDraft[] | null>(null)
  const [nextRowId, setNextRowId] = useState(0)
  const [saved, setSaved] = useState(false)

  const rows: PriceRowDraft[] =
    draft ?? (prices.data === undefined ? [] : prices.data.entries.map(toRow))

  const mutate = useCallback((next: PriceRowDraft[]) => {
    setSaved(false)
    setDraft(next)
  }, [])

  const save = useMutation({
    mutationFn: (payload: PriceRowDraft[]) => {
      if (prices.data === undefined) throw new Error('prices are not loaded')
      return pricesSet(toTable(payload, prices.data))
    },
    onSuccess: (result) => {
      queryClient.setQueryData(PRICES_QUERY_KEY, result)
      void queryClient.invalidateQueries({ queryKey: PRICE_CATALOG_QUERY_KEY })
      setDraft(null)
      setSaved(true)
    },
  })

  const issues = priceIssues(rows)

  const addDraft = (entry?: ObservedModelPrice) => {
    const matched = entry?.matchedPrice
    mutate([
      ...rows,
      {
        rowId: `new-${nextRowId}`,
        providerId: entry?.providerId ?? '',
        modelId: entry?.modelId ?? '',
        inputPerMtok: String(matched?.inputPerMtok ?? 0),
        outputPerMtok: String(matched?.outputPerMtok ?? 0),
        cacheReadPerMtok: String(matched?.cacheReadPerMtok ?? 0),
        cacheWritePerMtok: String(matched?.cacheWritePerMtok ?? 0),
        extra: {},
      },
    ])
    setNextRowId((current) => current + 1)
  }

  return {
    rows,
    catalog: catalog.data,
    issues,
    dirty: draft !== null,
    saved: saved && draft === null,
    isPending: prices.isPending || catalog.isPending,
    isSaving: save.isPending,
    error: prices.error ?? catalog.error ?? save.error ?? null,
    refetch: () => {
      void prices.refetch()
      void catalog.refetch()
    },
    addRow: () => addDraft(),
    addObservedModel: addDraft,
    deleteRow: (rowId: string) => mutate(rows.filter((row) => row.rowId !== rowId)),
    editRow: (rowId: string, patch: Partial<Omit<PriceRowDraft, 'rowId' | 'extra'>>) =>
      mutate(rows.map((row) => (row.rowId === rowId ? { ...row, ...patch } : row))),
    submit: () => {
      if (issues.length === 0) save.mutate(rows)
    },
  }
}
