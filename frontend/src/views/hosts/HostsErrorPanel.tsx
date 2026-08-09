/**
 * EXCLUSIVE FILE BOUNDARY — todo 18 owns `src/views/hosts/**`.
 *
 * Error panel that surfaces a structured `IpcError` **plus** the backend's own Chinese
 * remediation from `fields.remediation`.
 *
 * The shared `ErrorState` renders `code` + `message` only. Every typed SSH and credential
 * failure carries an actionable next step the user needs to see, so this panel adds that
 * line rather than dropping it.
 */
import { Button } from '@/components/ui/button'
import { zh } from '@/i18n/zh'

import { ipcMessage, ipcRemediation } from './hostsModel'

export function HostsErrorPanel({
  error,
  title = zh.common.errorTitle,
  testId,
  onRetry,
  remediationLabel = zh.hosts.list.remediationLabel,
}: {
  error: unknown
  title?: string
  testId: string
  onRetry?: () => void
  remediationLabel?: string
}) {
  const ipcError = ipcMessage(error)
  const remediation = ipcRemediation(error)
  return (
    <div
      data-testid={testId}
      role="alert"
      className="flex flex-col items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/5 p-4"
    >
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="text-sm font-semibold text-destructive">{title}</span>
        <span className="text-xs text-muted-foreground">
          {zh.common.errorCode}: <code data-testid={`${testId}-code`}>{ipcError.code}</code>
        </span>
      </div>
      <p data-testid={`${testId}-message`} className="text-sm text-foreground">
        {ipcError.message || zh.common.unknownError}
      </p>
      {remediation !== null ? (
        <p className="text-sm text-muted-foreground">
          <span className="font-medium text-foreground">{remediationLabel}：</span>
          <span data-testid={`${testId}-remediation`}>{remediation}</span>
        </p>
      ) : null}
      {onRetry !== undefined ? (
        <Button size="sm" variant="outline" onClick={onRetry}>
          {zh.common.retry}
        </Button>
      ) : null}
    </div>
  )
}
