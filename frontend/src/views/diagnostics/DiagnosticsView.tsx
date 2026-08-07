/**
 * Diagnostics view: the runtime log plus the feedback hand-off.
 *
 * A sixth top-level tab rather than a settings card. Two reasons decided it: a log list needs
 * vertical room and its own scroll region, which a card wedged between the price editor and the
 * archive path cannot give it; and a user hunting a failure should not have to walk through the
 * settings they were not trying to change. The nav row was already the widest element in the
 * header, so the label is the shortest one that still reads correctly.
 */
import { zh } from '@/i18n/zh'

import { FeedbackCard } from './FeedbackCard'
import { LogViewerCard } from './LogViewerCard'
import { useDiagnosticsReport, useLogTail } from './diagnosticsQueries'

export function DiagnosticsView() {
  const logs = useLogTail()
  const report = useDiagnosticsReport()

  return (
    <section data-testid="view-diagnostics" className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 className="text-2xl font-semibold tracking-tight">{zh.diagnostics.title}</h2>
        <p className="text-sm text-muted-foreground">{zh.diagnostics.subtitle}</p>
      </div>

      <LogViewerCard
        tail={logs.data}
        isPending={logs.isPending}
        error={logs.error}
        onRefresh={() => {
          void logs.refetch()
        }}
      />
      <FeedbackCard report={report.data} />
    </section>
  )
}
