/**
 * Application shell: header, navigation and the mount point for the five views.
 *
 * Owner: W8 prep (shell/infrastructure). Todos 15-19 must NOT edit this file; each owns
 * only its own `src/views/<view>/**` directory and `zh.<view>` dictionary section.
 */
import { useState, type JSX } from 'react'

import { AppNav } from '@/app/AppNav'
import { ReportRangeProvider } from '@/app/ReportRangeProvider'
import { TitleBar } from '@/app/titlebar/TitleBar'
import type { ViewKey } from '@/app/views'
import { AppErrorBoundary } from '@/components/app-error-boundary'
import { zh } from '@/i18n/zh'
import { DetailView } from '@/views/detail/DetailView'
import { DrilldownView } from '@/views/drilldown/DrilldownView'
import { HostsView } from '@/views/hosts/HostsView'
import { OverviewView } from '@/views/overview/OverviewView'
import { SettingsView } from '@/views/settings/SettingsView'

const VIEWS: Record<ViewKey, () => JSX.Element> = {
  overview: OverviewView,
  drilldown: DrilldownView,
  detail: DetailView,
  hosts: HostsView,
  settings: SettingsView,
}

function App() {
  const [view, setView] = useState<ViewKey>('overview')
  const ActiveView = VIEWS[view]

  return (
    <AppErrorBoundary>
      <div className="flex min-h-screen flex-col bg-background text-foreground">
        <TitleBar />
        <header className="sticky top-titlebar z-40 border-b border-border bg-background/85 shadow-panel backdrop-blur">
          <div className="mx-auto flex max-w-6xl flex-wrap items-end justify-between gap-x-6 gap-y-3 px-6 pt-4 pb-3">
            <div className="flex flex-col gap-0.5">
              <h1 className="text-xl leading-tight font-semibold tracking-tight">{zh.appName}</h1>
              <p className="text-xs text-muted-foreground">{zh.tagline}</p>
            </div>
            <AppNav active={view} onSelect={setView} />
          </div>
        </header>
        <main className="mx-auto w-full max-w-6xl flex-1 px-6 py-6">
          <ReportRangeProvider>
            <ActiveView />
          </ReportRangeProvider>
        </main>
      </div>
    </AppErrorBoundary>
  )
}

export default App
