/**
 * Shared loading / empty / error primitives.
 *
 * Owner: W8 prep (shell/infrastructure). Views (todos 15-19) should reuse these instead
 * of hand-rolling their own states, so an IPC failure always renders a readable
 * `code` + `message` panel rather than a white screen or `[object Object]`.
 */
import type { ReactNode } from 'react'

import { Button } from '@/components/ui/button'
import type { IpcError } from '@/generated'
import { zh } from '@/i18n/zh'
import { toIpcError } from '@/lib/ipc'

export function LoadingState({ label = zh.common.loading }: { label?: string }) {
  return (
    <div
      data-testid="loading-state"
      role="status"
      aria-live="polite"
      className="flex min-h-32 items-center justify-center text-sm text-muted-foreground"
    >
      {label}
    </div>
  )
}

export function EmptyState({
  label = zh.common.empty,
  children,
}: {
  label?: string
  children?: ReactNode
}) {
  return (
    <div
      data-testid="empty-state"
      className="flex min-h-32 flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-border p-6 text-sm text-muted-foreground"
    >
      <span>{label}</span>
      {children}
    </div>
  )
}

/**
 * Renders a structured `IpcError`. Accepts `unknown` so callers can hand it whatever
 * TanStack Query put in `error` without narrowing first.
 */
export function ErrorState({
  error,
  onRetry,
  title = zh.common.errorTitle,
}: {
  error: unknown
  onRetry?: () => void
  title?: string
}) {
  const ipcError: IpcError = toIpcError(error)
  return (
    <div
      data-testid="error-state"
      role="alert"
      className="flex flex-col items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-6"
    >
      <div className="flex flex-col gap-1">
        <span className="text-sm font-semibold text-destructive">{title}</span>
        <span className="text-xs text-muted-foreground">
          {zh.common.errorCode}: <code data-testid="error-code">{ipcError.code}</code>
        </span>
        <span data-testid="error-message" className="text-sm text-foreground">
          {ipcError.message || zh.common.unknownError}
        </span>
      </div>
      {onRetry ? (
        <Button size="sm" variant="outline" onClick={onRetry}>
          {zh.common.retry}
        </Button>
      ) : null}
    </div>
  )
}
