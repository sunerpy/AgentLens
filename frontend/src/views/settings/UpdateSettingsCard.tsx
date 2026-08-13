import { openUrl } from '@tauri-apps/plugin-opener'
import { useState } from 'react'

import { ErrorState } from '@/components/app-state'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { UpdateMetadata, UpdateProgress } from '@/generated'
import { zh } from '@/i18n/zh'
import { updaterCheck, updaterInstall } from '@/lib/ipc'

import { CONTROL_CLASS, SettingsField } from './SettingsField'
import type { UpdateProxyIssue } from './settingsKeys'
import type { useSettingsForm } from './useSettingsForm'

type Form = ReturnType<typeof useSettingsForm>

const RELEASE_URL = 'https://github.com/sunerpy/AgentLens/releases/latest'

const PROXY_ISSUE_TEXT: Record<UpdateProxyIssue, string> = {
  malformed: zh.settings.update.proxyMalformed,
  unsupportedScheme: zh.settings.update.proxyUnsupportedScheme,
  unsupportedShape: zh.settings.update.proxyUnsupportedShape,
}

function progressPercent(progress: UpdateProgress | null): number | null {
  if (progress?.event !== 'downloading') return null
  if (progress.data.total === null || progress.data.total <= 0) return 0
  return Math.min(100, Math.round((progress.data.downloaded / progress.data.total) * 100))
}

function progressText(progress: UpdateProgress | null): string | null {
  if (progress === null) return null
  if (progress.event === 'started') return zh.settings.update.progressPreparing
  if (progress.event === 'downloaded') return zh.settings.update.progressDownloaded
  return zh.settings.update.progressPercent(progressPercent(progress) ?? 0)
}

export function UpdateSettingsCard({ form }: { form: Form }) {
  const [metadata, setMetadata] = useState<UpdateMetadata | null>(null)
  const [progress, setProgress] = useState<UpdateProgress | null>(null)
  const [checking, setChecking] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const [openFailed, setOpenFailed] = useState(false)
  const values = form.values
  if (values === undefined) return null

  const updateAvailable = metadata?.version !== null && metadata?.version !== undefined
  const canInstall =
    updateAvailable && values.autoUpdateEnabled && metadata.autoInstallSupported && !form.dirty
  const advice = !updateAvailable
    ? null
    : !values.autoUpdateEnabled
      ? zh.settings.update.disabledAdvice
      : !metadata.autoInstallSupported
        ? zh.settings.update.unsupportedAdvice
        : form.dirty
          ? zh.settings.update.unsavedAdvice
          : null
  const percent = progressPercent(progress)
  const proxyIssue = form.issues.updateProxy
  const usingSystemProxy = values.updateProxyUrl.trim() === ''

  const check = async () => {
    setChecking(true)
    setError(null)
    setProgress(null)
    try {
      setMetadata(await updaterCheck())
    } catch (caught) {
      setError(caught)
    } finally {
      setChecking(false)
    }
  }

  const install = async () => {
    setInstalling(true)
    setError(null)
    setProgress(null)
    try {
      await updaterInstall(setProgress)
    } catch (caught) {
      setError(caught)
    } finally {
      setInstalling(false)
    }
  }

  const openRelease = async () => {
    setOpenFailed(false)
    try {
      await openUrl(RELEASE_URL)
    } catch {
      setOpenFailed(true)
    }
  }

  return (
    <Card data-testid="settings-update">
      <CardHeader>
        <CardTitle>{zh.settings.update.title}</CardTitle>
        <CardDescription>{zh.settings.update.description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <SettingsField
          id="settings-auto-update"
          label={zh.settings.update.autoInstall}
          hint={zh.settings.update.autoInstallHint}
        >
          <div className="flex items-center gap-2">
            <input
              id="settings-auto-update"
              data-testid="settings-auto-update"
              type="checkbox"
              className="size-4 accent-primary"
              checked={values.autoUpdateEnabled}
              onChange={(event) => form.update({ autoUpdateEnabled: event.target.checked })}
            />
            <span className="text-xs text-muted-foreground select-none">
              {values.autoUpdateEnabled
                ? zh.settings.update.autoInstallOn
                : zh.settings.update.autoInstallOff}
            </span>
          </div>
        </SettingsField>

        <SettingsField
          id="settings-update-proxy"
          label={zh.settings.update.proxy}
          hint={
            <span className="flex flex-col gap-1">
              {proxyIssue === null ? null : (
                <span
                  data-testid="settings-update-proxy-issue"
                  className="font-medium text-destructive"
                >
                  {PROXY_ISSUE_TEXT[proxyIssue]}
                </span>
              )}
              <span data-testid="settings-update-proxy-hint">{zh.settings.update.proxyHint}</span>
            </span>
          }
        >
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
            <input
              id="settings-update-proxy"
              data-testid="settings-update-proxy"
              type="text"
              inputMode="url"
              autoCapitalize="off"
              autoComplete="off"
              spellCheck={false}
              aria-invalid={proxyIssue !== null}
              className={`${CONTROL_CLASS} min-w-0 flex-1 font-mono`}
              placeholder={zh.settings.update.proxyPlaceholder}
              value={values.updateProxyUrl}
              onChange={(event) => {
                setMetadata(null)
                setError(null)
                form.update({ updateProxyUrl: event.target.value })
              }}
            />
            {proxyIssue === null ? (
              <span
                data-testid="settings-update-proxy-state"
                className="w-fit rounded-md bg-muted px-2 py-1 text-xs text-muted-foreground select-none"
              >
                {usingSystemProxy ? zh.settings.update.proxySystem : zh.settings.update.proxyCustom}
              </span>
            ) : null}
          </div>
        </SettingsField>

        <div className="flex flex-wrap items-center gap-3 border-t border-border pt-4">
          <Button
            type="button"
            variant="outline"
            data-testid="settings-update-check"
            disabled={checking || installing || form.dirty || form.hasIssue}
            onClick={() => void check()}
          >
            {checking ? zh.settings.update.checking : zh.settings.update.check}
          </Button>
          {metadata === null ? null : (
            <span className="text-xs text-muted-foreground">
              {zh.settings.update.currentVersion(metadata.currentVersion)}
            </span>
          )}
        </div>

        {metadata === null ? null : updateAvailable ? (
          <div className="flex flex-col gap-3 rounded-lg border border-primary/30 bg-primary/5 p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <strong data-testid="settings-update-version" className="text-sm">
                {zh.settings.update.available(metadata.version ?? '')}
              </strong>
              {metadata.date === null ? null : (
                <span className="text-xs text-muted-foreground">
                  {zh.settings.update.releaseDate(metadata.date)}
                </span>
              )}
            </div>
            {metadata.body === null || metadata.body === '' ? null : (
              <div className="flex flex-col gap-1 text-xs text-muted-foreground">
                <span className="font-medium text-foreground">
                  {zh.settings.update.releaseNotes}
                </span>
                <p className="whitespace-pre-wrap">{metadata.body}</p>
              </div>
            )}
            {advice === null ? null : (
              <p data-testid="settings-update-advice" className="text-xs text-muted-foreground">
                {advice}
              </p>
            )}
            <div className="flex flex-wrap items-center gap-3">
              {canInstall ? (
                <Button
                  type="button"
                  data-testid="settings-update-install"
                  disabled={installing}
                  onClick={() => void install()}
                >
                  {installing ? zh.settings.update.installing : zh.settings.update.install}
                </Button>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  data-testid="settings-update-release"
                  onClick={() => void openRelease()}
                >
                  {zh.settings.update.openRelease}
                </Button>
              )}
              {progressText(progress) === null ? null : (
                <span
                  data-testid="settings-update-progress"
                  role="status"
                  className="text-xs text-muted-foreground"
                >
                  {progressText(progress)}
                </span>
              )}
            </div>
            {percent === null ? null : (
              <progress
                aria-label={zh.settings.update.progressPercent(percent)}
                className="h-2 w-full accent-primary"
                max={100}
                value={percent}
              />
            )}
            {openFailed ? (
              <span className="text-xs text-destructive">
                {zh.settings.update.openReleaseFailed}
              </span>
            ) : null}
          </div>
        ) : (
          <p data-testid="settings-update-latest" className="text-xs text-muted-foreground">
            {zh.settings.update.latest}
          </p>
        )}

        {error === null ? null : <ErrorState error={error} onRetry={check} />}
      </CardContent>
    </Card>
  )
}
