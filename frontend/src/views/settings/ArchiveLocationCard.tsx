/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Archive location, read out of the `archive.path` key the Rust shell publishes at boot.
 *
 * Both a reveal action and a copy action are offered, because neither covers every
 * environment: revealing needs a desktop shell with a reachable file manager, and outside one
 * (a `vite dev` tab, the Playwright QA run, or a Linux box with no `org.freedesktop.FileManager1`
 * on the session bus) copying is the only thing that works. A reveal that cannot happen
 * therefore says which of the two cases it hit instead of throwing.
 */
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { zh } from '@/i18n/zh'
import { revealPath, type RevealOutcome } from '@/lib/revealPath'

const REVEAL_NOTICE: Record<Exclude<RevealOutcome, 'revealed'>, string> = {
  unsupported: zh.settings.archive.openUnsupported,
  failed: zh.settings.archive.openFailed,
}

export function ArchiveLocationCard({ path }: { path: string }) {
  const [copied, setCopied] = useState(false)
  const [revealNotice, setRevealNotice] = useState<string | null>(null)

  return (
    <Card data-testid="settings-archive">
      <CardHeader>
        <CardTitle>{zh.settings.archive.title}</CardTitle>
        <CardDescription>{zh.settings.archive.description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <code
          data-testid="settings-archive-path"
          className="block overflow-x-auto rounded-lg bg-muted px-3 py-2 text-xs text-foreground"
        >
          {path === '' ? zh.settings.archive.unavailable : path}
        </code>
        <div className="flex flex-wrap items-center gap-3">
          <Button
            type="button"
            size="sm"
            variant="outline"
            data-testid="settings-archive-open"
            disabled={path === ''}
            onClick={() => {
              setRevealNotice(null)
              void revealPath(path).then((outcome) => {
                setRevealNotice(outcome === 'revealed' ? null : REVEAL_NOTICE[outcome])
              })
            }}
          >
            {zh.settings.archive.open}
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            data-testid="settings-archive-copy"
            disabled={path === ''}
            onClick={() => {
              navigator.clipboard.writeText(path).then(
                () => setCopied(true),
                () => setCopied(false),
              )
            }}
          >
            {zh.settings.archive.copy}
          </Button>
          {copied ? (
            <span data-testid="settings-archive-copied" className="text-xs text-muted-foreground">
              {zh.settings.archive.copied}
            </span>
          ) : null}
        </div>
        {revealNotice === null ? null : (
          <span
            data-testid="settings-archive-open-notice"
            className="text-xs text-muted-foreground"
          >
            {revealNotice}
          </span>
        )}
      </CardContent>
    </Card>
  )
}
