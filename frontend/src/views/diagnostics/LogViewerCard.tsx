/**
 * Runtime log viewer.
 *
 * `select-text` is set explicitly on the record rows: the shell suppresses the page context
 * menu, so selecting a line and using Ctrl+C is a user's fallback when the copy button cannot
 * reach the clipboard (no permission, or a `vite dev` tab over plain HTTP). Log text must stay
 * selectable for that to work.
 */
import { useMemo, useState } from 'react'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { EmptyState, ErrorState, LoadingState } from '@/components/app-state'
import type { LogTail } from '@/generated'
import { zh } from '@/i18n/zh'
import { revealPath, type RevealOutcome } from '@/lib/revealPath'

import { entriesToText, filterEntries, formatTimestamp, LEVEL_CLASS, LOG_LEVELS } from './logLevels'
import type { LevelFilter } from './logLevels'

const REVEAL_NOTICE: Record<Exclude<RevealOutcome, 'revealed'>, string> = {
  unsupported: zh.diagnostics.logs.openUnsupported,
  failed: zh.diagnostics.logs.openFailed,
}

const LEVEL_LABEL: Record<LevelFilter, string> = {
  all: zh.diagnostics.logs.levelAll,
  error: 'ERROR',
  warn: 'WARN',
  info: 'INFO',
  debug: 'DEBUG',
  trace: 'TRACE',
}

const FILTERS: readonly LevelFilter[] = ['all', ...LOG_LEVELS] as const

export function LogViewerCard({
  tail,
  isPending,
  error,
  onRefresh,
}: {
  tail: LogTail | undefined
  isPending: boolean
  error: unknown
  onRefresh: () => void
}) {
  const [level, setLevel] = useState<LevelFilter>('all')
  const [copied, setCopied] = useState<boolean | null>(null)
  const [revealNotice, setRevealNotice] = useState<string | null>(null)

  const entries = useMemo(() => filterEntries(tail?.entries ?? [], level), [tail, level])
  const directory = tail?.directory ?? ''

  return (
    <Card data-testid="diagnostics-logs">
      <CardHeader>
        <CardTitle>{zh.diagnostics.logs.title}</CardTitle>
        <CardDescription>{zh.diagnostics.logs.description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs text-muted-foreground">{zh.diagnostics.logs.levelLabel}</span>
          <div role="group" aria-label={zh.diagnostics.logs.levelLabel} className="flex gap-1">
            {FILTERS.map((candidate) => (
              <Button
                key={candidate}
                type="button"
                size="sm"
                variant={candidate === level ? 'default' : 'ghost'}
                aria-pressed={candidate === level}
                data-testid={`diagnostics-level-${candidate}`}
                onClick={() => {
                  setLevel(candidate)
                  setCopied(null)
                }}
              >
                {LEVEL_LABEL[candidate]}
              </Button>
            ))}
          </div>
          <span aria-hidden className="h-5 w-px bg-border" />
          <Button
            type="button"
            size="sm"
            variant="outline"
            data-testid="diagnostics-refresh"
            onClick={() => {
              setCopied(null)
              onRefresh()
            }}
          >
            {zh.diagnostics.logs.refresh}
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            data-testid="diagnostics-copy"
            disabled={entries.length === 0}
            onClick={() => {
              navigator.clipboard.writeText(entriesToText(entries)).then(
                () => setCopied(true),
                () => setCopied(false),
              )
            }}
          >
            {zh.diagnostics.logs.copy}
          </Button>
          {copied === null ? null : (
            <span
              data-testid={copied ? 'diagnostics-copied' : 'diagnostics-copy-failed'}
              className="text-xs text-muted-foreground"
            >
              {copied ? zh.diagnostics.logs.copied : zh.diagnostics.logs.copyFailed}
            </span>
          )}
          <span data-testid="diagnostics-count" className="ml-auto text-xs text-muted-foreground">
            {entries.length} {zh.diagnostics.logs.count}
          </span>
        </div>

        {error !== null && error !== undefined ? (
          <ErrorState error={error} onRetry={onRefresh} />
        ) : isPending ? (
          <LoadingState />
        ) : entries.length === 0 ? (
          <EmptyState
            label={
              tail === undefined || tail.empty
                ? zh.diagnostics.logs.empty
                : zh.diagnostics.logs.emptyFiltered
            }
          />
        ) : (
          <ol
            data-testid="diagnostics-log-list"
            className="flex max-h-[28rem] flex-col divide-y divide-border overflow-y-auto rounded-lg border border-border bg-muted/30"
          >
            {entries.map((entry, index) => (
              <li
                key={`${entry.timestamp}-${index}`}
                data-testid="diagnostics-log-row"
                data-level={entry.level}
                className="flex flex-col gap-1 px-3 py-2 select-text"
              >
                <div className="flex flex-wrap items-center gap-2 font-mono text-[0.7rem] text-muted-foreground">
                  <span
                    data-testid="diagnostics-log-level"
                    className={`rounded px-1.5 py-0.5 font-semibold ${LEVEL_CLASS[entry.level]}`}
                  >
                    {entry.level.toUpperCase()}
                  </span>
                  <span>{formatTimestamp(entry.timestamp)}</span>
                  <span className="truncate">{entry.target}</span>
                </div>
                <span
                  data-testid="diagnostics-log-message"
                  className="font-mono text-xs break-words text-foreground"
                >
                  {entry.message}
                </span>
              </li>
            ))}
          </ol>
        )}

        <div className="flex flex-col gap-2 border-t border-border pt-3">
          <span className="text-xs text-muted-foreground">
            {zh.diagnostics.logs.directoryLabel}
          </span>
          <code
            data-testid="diagnostics-directory"
            className="block overflow-x-auto rounded-lg bg-muted px-3 py-2 text-xs text-foreground select-text"
          >
            {directory}
          </code>
          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="button"
              size="sm"
              variant="outline"
              data-testid="diagnostics-open-directory"
              disabled={directory === ''}
              onClick={() => {
                setRevealNotice(null)
                void revealPath(directory).then((outcome) => {
                  setRevealNotice(outcome === 'revealed' ? null : REVEAL_NOTICE[outcome])
                })
              }}
            >
              {zh.diagnostics.logs.openDirectory}
            </Button>
            {revealNotice === null ? null : (
              <span data-testid="diagnostics-open-notice" className="text-xs text-muted-foreground">
                {revealNotice}
              </span>
            )}
          </div>
          <p className="text-xs text-muted-foreground">{zh.diagnostics.logs.retention}</p>
          <p className="text-xs text-muted-foreground">{zh.diagnostics.logs.envHint}</p>
        </div>
      </CardContent>
    </Card>
  )
}
