/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Archive location, read out of the `archive.path` key the Rust shell publishes at boot.
 *
 * "Open the containing folder" is not offered: this Tauri 2 setup has neither the opener nor the
 * shell plugin installed, and adding one would require editing `frontend/package.json` plus
 * `frontend/src/lib/ipc.ts`, both of which are outside this worker's file boundary. The path is
 * therefore rendered selectable with a copy button, and the reason is stated in the UI.
 */
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { zh } from '@/i18n/zh'

export function ArchiveLocationCard({ path }: { path: string }) {
  const [copied, setCopied] = useState(false)

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
        <div className="flex items-center gap-3">
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
        <span
          data-testid="settings-archive-open-unavailable"
          className="text-xs text-muted-foreground"
        >
          {zh.settings.archive.openUnavailable}
        </span>
      </CardContent>
    </Card>
  )
}
