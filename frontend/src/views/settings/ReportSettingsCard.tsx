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
import { MIN_INTERVAL_SECONDS } from './settingsKeys'
import { timezoneOptions } from './timezones'
import type { useSettingsForm } from './useSettingsForm'

type Form = ReturnType<typeof useSettingsForm>

function IntervalField({
  id,
  label,
  testId,
  value,
  clamped,
  onChange,
  onCommit,
}: {
  id: string
  label: string
  testId: string
  value: string
  clamped: boolean
  onChange: (value: string) => void
  onCommit: (value: string) => void
}) {
  return (
    <SettingsField
      id={id}
      label={label}
      hint={
        clamped ? (
          <span data-testid={`${testId}-clamped`} className="text-destructive">
            {zh.settings.refresh.clamped}
          </span>
        ) : (
          zh.settings.refresh.minHint
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
          className={`${CONTROL_CLASS} w-28`}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          onBlur={(event) => onCommit(event.target.value)}
        />
        <span className="text-xs text-muted-foreground">{zh.settings.refresh.unitSeconds}</span>
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
          <div className="flex flex-wrap items-start gap-6">
            <IntervalField
              id="settings-local-interval"
              testId="settings-local-interval"
              label={zh.settings.refresh.local}
              value={values.localIntervalSeconds}
              clamped={form.clamped.local}
              onChange={(next) => form.update({ localIntervalSeconds: next })}
              onCommit={(next) => form.commitInterval('local', next)}
            />
            <IntervalField
              id="settings-remote-interval"
              testId="settings-remote-interval"
              label={zh.settings.refresh.remote}
              value={values.remoteIntervalSeconds}
              clamped={form.clamped.remote}
              onChange={(next) => form.update({ remoteIntervalSeconds: next })}
              onCommit={(next) => form.commitInterval('remote', next)}
            />
          </div>
          <span className="text-xs text-muted-foreground">{zh.settings.refresh.applyHint}</span>
        </div>

        <div className="flex items-center gap-3 border-t border-border pt-4">
          <Button
            type="button"
            data-testid="settings-save"
            disabled={!form.dirty || form.isSaving}
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
