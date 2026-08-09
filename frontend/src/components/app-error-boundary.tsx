/**
 * Top-level error boundary so a render-time throw inside any view degrades to a
 * readable error panel instead of a blank window.
 *
 * Owner: W8 prep (shell/infrastructure).
 */
import { Component, type ErrorInfo, type ReactNode } from 'react'

import { ErrorState } from '@/components/app-state'

interface AppErrorBoundaryProps {
  children: ReactNode
}

interface AppErrorBoundaryState {
  error: unknown
}

export class AppErrorBoundary extends Component<AppErrorBoundaryProps, AppErrorBoundaryState> {
  state: AppErrorBoundaryState = { error: null }

  static getDerivedStateFromError(error: unknown): AppErrorBoundaryState {
    return { error }
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    // Kept as console output on purpose: the desktop shell has no remote log sink.
    console.error('AgentLens render error', error, info.componentStack)
  }

  private readonly reset = () => {
    this.setState({ error: null })
  }

  render() {
    if (this.state.error !== null) {
      return (
        <div className="p-8">
          <ErrorState error={this.state.error} onRetry={this.reset} />
        </div>
      )
    }
    return this.props.children
  }
}
