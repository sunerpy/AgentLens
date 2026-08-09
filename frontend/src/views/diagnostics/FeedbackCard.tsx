/**
 * GitHub feedback entry point.
 *
 * The environment block is rendered before the button on purpose: what gets published has to be
 * visible before it is published. Only the four values shown here travel with the issue — see
 * `@/lib/openIssue` for why the log body deliberately does not.
 */
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { LoadingState } from '@/components/app-state'
import type { DiagnosticsReport } from '@/generated'
import { zh } from '@/i18n/zh'
import { buildIssueUrl, openIssue, type OpenIssueOutcome } from '@/lib/openIssue'

const OPEN_NOTICE: Record<Exclude<OpenIssueOutcome, 'opened'>, string> = {
  unsupported: zh.diagnostics.feedback.openUnsupported,
  failed: zh.diagnostics.feedback.openFailed,
}

export function FeedbackCard({ report }: { report: DiagnosticsReport | undefined }) {
  const [notice, setNotice] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  return (
    <Card data-testid="diagnostics-feedback">
      <CardHeader>
        <CardTitle>{zh.diagnostics.feedback.title}</CardTitle>
        <CardDescription>{zh.diagnostics.feedback.description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {report === undefined ? (
          <LoadingState />
        ) : (
          <>
            <p data-testid="diagnostics-privacy" className="text-xs text-muted-foreground">
              {zh.diagnostics.feedback.privacyNotice}
            </p>
            <p className="text-xs font-medium text-foreground">
              {zh.diagnostics.feedback.environmentTitle}
            </p>
            <dl
              data-testid="diagnostics-environment"
              aria-label={zh.diagnostics.feedback.environmentTitle}
              className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 rounded-lg bg-muted/40 px-3 py-2 text-xs"
            >
              <dt className="text-muted-foreground">{zh.diagnostics.feedback.appVersion}</dt>
              <dd data-testid="diagnostics-app-version" className="font-mono select-text">
                {report.appVersion}
              </dd>
              <dt className="text-muted-foreground">{zh.diagnostics.feedback.os}</dt>
              <dd data-testid="diagnostics-os" className="font-mono select-text">
                {report.os}
              </dd>
              <dt className="text-muted-foreground">{zh.diagnostics.feedback.arch}</dt>
              <dd data-testid="diagnostics-arch" className="font-mono select-text">
                {report.arch}
              </dd>
              <dt className="text-muted-foreground">{zh.diagnostics.feedback.webview}</dt>
              <dd data-testid="diagnostics-webview" className="font-mono select-text">
                {report.webviewVersion ?? zh.diagnostics.feedback.webviewUnknown}
              </dd>
            </dl>
            <div className="flex flex-wrap items-center gap-3">
              <Button
                type="button"
                size="sm"
                data-testid="diagnostics-open-issue"
                onClick={() => {
                  setNotice(null)
                  void openIssue(report).then((outcome) => {
                    setNotice(outcome === 'opened' ? null : OPEN_NOTICE[outcome])
                  })
                }}
              >
                {zh.diagnostics.feedback.open}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                data-testid="diagnostics-copy-link"
                onClick={() => {
                  navigator.clipboard.writeText(buildIssueUrl(report)).then(
                    () => setCopied(true),
                    () => setCopied(false),
                  )
                }}
              >
                {zh.diagnostics.feedback.copyLink}
              </Button>
              {copied ? (
                <span
                  data-testid="diagnostics-link-copied"
                  className="text-xs text-muted-foreground"
                >
                  {zh.diagnostics.feedback.copied}
                </span>
              ) : null}
              {notice === null ? null : (
                <span
                  data-testid="diagnostics-issue-notice"
                  className="text-xs text-muted-foreground"
                >
                  {notice}
                </span>
              )}
            </div>
          </>
        )}
      </CardContent>
    </Card>
  )
}
