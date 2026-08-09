/**
 * EXCLUSIVE FILE BOUNDARY — todo 19 owns `src/views/settings/**`.
 *
 * Theme picker. The theme is persisted through the same `app_settings` upsert the rest of this
 * view uses, but it is written by `ThemeProvider` on click rather than batched behind 保存设置:
 * a colour choice is judged by looking at it, so deferring it would make the preview useless.
 */
import { ThemeOptionGrid } from '@/app/theme/ThemeOptionGrid'
import { useTheme } from '@/app/theme/ThemeContext'
import { ErrorState } from '@/components/app-state'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { zh } from '@/i18n/zh'

export function AppearanceCard() {
  const { error } = useTheme()

  return (
    <Card data-testid="settings-appearance">
      <CardHeader>
        <CardTitle>{zh.settings.appearance.title}</CardTitle>
        <CardDescription>{zh.settings.appearance.description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <ThemeOptionGrid columns={2} />
        <p className="text-xs text-muted-foreground">{zh.settings.appearance.persistHint}</p>
        {error === null ? null : <ErrorState error={error} />}
      </CardContent>
    </Card>
  )
}
