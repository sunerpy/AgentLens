/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Report timezone / week start and the two refresh intervals, plus the single save button that
 * persists all four keys into `app_settings`.
 */
import { useMemo } from 'react'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import type { WeekStart } from '@/generated'
import { zh } from '@/i18n/zh'

import { CONTROL_CLASS, SettingsField } from './SettingsField'
import { MIN_INTERVAL_SECONDS, type IntervalIssue } from './settingsKeys'
import { timezoneOptions } from './timezones'
import type { useSettingsForm } from './useSettingsForm'

type Form = ReturnType<typeof useSettingsForm>

const ISSUE_TEXT: Record<IntervalIssue, string> = {
  malformed: zh.settings.refresh.malformed,
  belowFloor: zh.settings.refresh.belowFloor,
}

/**
 * The floor is surfaced as an error that disables the save, not as a silent correction: the
 * backend refuses a sub-floor write, so correcting the value here would persist something the
 * user never asked for and hide the refusal.
 */
function IntervalField({
  id,
  label,
  testId,
  value,
  issue,
  disabled,
  onChange,
}: {
  id: string
  label: string
  testId: string
  value: string
  issue: IntervalIssue | null
  disabled: boolean
  onChange: (value: string) => void
}) {
  return (
    <SettingsField
      id={id}
      label={label}
      hint={
        issue === null ? (
          zh.settings.refresh.minHint
        ) : (
          <span data-testid={`${testId}-issue`} className="text-destructive">
            {ISSUE_TEXT[issue]}
          </span>
        )
      }
    >
      <div className="flex items-center gap-2">
        <input
          id={id}
          data-testid={testId}
          type="number"
          inputMode="numeric"
          min={MIN_INTERVAL_SECONDS}
          step={1}
          disabled={disabled}
          aria-invalid={issue !== null}
          className={`${CONTROL_CLASS} w-28`}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
        <span className="text-xs text-muted-foreground select-none">
          {zh.settings.refresh.unitSeconds}
        </span>
      </div>
    </SettingsField>
  )
}

export function ReportSettingsCard({ form }: { form: Form }) {
  const values = form.values
  const timezones = useMemo(() => timezoneOptions(values?.timezone ?? ''), [values?.timezone])
  if (values === undefined) return null

  return (
    <Card data-testid="settings-report">
      <CardHeader>
        <CardTitle>{zh.settings.report.title}</CardTitle>
        <CardDescription>{zh.settings.report.description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex flex-wrap items-start gap-6">
          <SettingsField
            id="settings-timezone"
            label={zh.settings.report.timezone}
            hint={
              <>
                {zh.settings.report.timezoneHint}
                <br />
                {zh.settings.report.timezoneEffect}
              </>
            }
          >
            <select
              id="settings-timezone"
              data-testid="settings-timezone"
              className={`${CONTROL_CLASS} min-w-56`}
              value={values.timezone}
              onChange={(event) => form.update({ timezone: event.target.value })}
            >
              {timezones.map((zone) => (
                <option key={zone} value={zone}>
                  {zone}
                </option>
              ))}
            </select>
          </SettingsField>

          <SettingsField
            id="settings-week-start"
            label={zh.settings.report.weekStart}
            hint={zh.settings.report.weekStartHint}
          >
            <select
              id="settings-week-start"
              data-testid="settings-week-start"
              className={`${CONTROL_CLASS} min-w-28`}
              value={values.weekStart}
              onChange={(event) => form.update({ weekStart: event.target.value as WeekStart })}
            >
              <option value="monday">{zh.settings.report.weekStartMonday}</option>
              <option value="sunday">{zh.settings.report.weekStartSunday}</option>
            </select>
          </SettingsField>
        </div>

        <div className="flex flex-col gap-3 border-t border-border pt-4">
          <div className="flex flex-col gap-1">
            <span className="text-sm font-medium">{zh.settings.refresh.title}</span>
            <span className="text-xs text-muted-foreground">{zh.settings.refresh.description}</span>
          </div>

          <SettingsField
            id="settings-auto-refresh"
            label={zh.settings.refresh.autoRefresh}
            hint={zh.settings.refresh.autoRefreshHint}
          >
            <div className="flex items-center gap-2">
              <input
                id="settings-auto-refresh"
                data-testid="settings-auto-refresh"
                type="checkbox"
                className="size-4 accent-primary outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
                checked={values.autoRefreshEnabled}
                onChange={(event) => form.update({ autoRefreshEnabled: event.target.checked })}
              />
              <span
                data-testid="settings-auto-refresh-state"
                className="text-xs text-muted-foreground select-none"
              >
                {values.autoRefreshEnabled
                  ? zh.settings.refresh.autoRefreshOn
                  : zh.settings.refresh.autoRefreshOff}
              </span>
            </div>
          </SettingsField>

          {/* Intervals only matter while the timer runs, so they are disabled — not hidden —
              when auto-refresh is off: hiding them would lose the configured values from view
              and make the toggle look destructive. */}
          <div className="flex flex-wrap items-start gap-6">
            <IntervalField
              id="settings-local-interval"
              testId="settings-local-interval"
              label={zh.settings.refresh.local}
              value={values.localIntervalSeconds}
              issue={form.issues.local}
              disabled={!values.autoRefreshEnabled}
              onChange={(next) => form.update({ localIntervalSeconds: next })}
            />
            <IntervalField
              id="settings-remote-interval"
              testId="settings-remote-interval"
              label={zh.settings.refresh.remote}
              value={values.remoteIntervalSeconds}
              issue={form.issues.remote}
              disabled={!values.autoRefreshEnabled}
              onChange={(next) => form.update({ remoteIntervalSeconds: next })}
            />
          </div>
          <span className="text-xs text-muted-foreground">{zh.settings.refresh.applyHint}</span>
        </div>

        <div className="flex items-center gap-3 border-t border-border pt-4">
          <Button
            type="button"
            data-testid="settings-save"
            disabled={!form.dirty || form.isSaving || form.hasIssue}
            onClick={form.submit}
          >
            {zh.settings.save}
          </Button>
          {form.dirty ? (
            <span data-testid="settings-dirty" className="text-xs text-muted-foreground">
              {zh.settings.dirty}
            </span>
          ) : null}
          {form.saved ? (
            <span data-testid="settings-saved" className="text-xs text-muted-foreground">
              {zh.settings.saved}
            </span>
          ) : null}
        </div>
      </CardContent>
    </Card>
  )
}
