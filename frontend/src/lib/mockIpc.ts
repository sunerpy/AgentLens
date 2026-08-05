/**
 * Mock IPC layer — the shared test harness contract for todos 15-19.
 *
 * Owner: W8 prep (shell/infrastructure). **Treat this file as an API, not as scratch data.**
 * The five view workers assert against the seeded dataset described below; changing a
 * number here silently breaks their Playwright specs.
 *
 * How it gets loaded: `main.tsx` dynamically imports and installs this module when
 * `import.meta.env.DEV` and the URL carries `?mockIpc=1`. It replaces
 * `window.__TAURI_INTERNALS__`, which is the object `@tauri-apps/api/core`'s `invoke`
 * delegates to, so `@/lib/ipc` needs no test-only branch. In a production build the
 * branch is statically false and the module is never bundled.
 *
 * Playwright usage (the template every view spec copies):
 *
 * ```ts
 * await page.addInitScript(() => {
 *   window.__AGENTLENS_MOCK_IPC_CONFIG__ = { errors: { get_summary: {...} } }
 * })
 * await page.goto('/?mockIpc=1')
 * const calls = await page.evaluate(() => window.__AGENTLENS_MOCK_IPC__.callsFor('query_messages'))
 * ```
 *
 * ### Seeded dataset (deterministic; report timezone is UTC, week start Monday)
 *
 * `trend` — seven day buckets 2026-01-01 .. 2026-01-07, deliberately covering every
 * coverage/cost shape the overview view must distinguish:
 * | bucket     | coverage  | tokens/cost/messageCount | why it exists |
 * | ---------- | --------- | ------------------------ | ------------- |
 * | 2026-01-01 | `none`    | all `null`               | must render as a BREAK, never as 0 |
 * | 2026-01-02 | `partial` | present                  | half-covered bucket styling |
 * | 2026-01-03 | `full`    | all zeros                | covered-but-idle, must render as 0 |
 * | 2026-01-04 | `full`    | `actualSum 0.0102`, `unavailableCount 1` | the mixed-cost day → `部分缺失` badge |
 * | 2026-01-05 | `full`    | `estimatedSum 0.0075`    | estimate-only day |
 * | 2026-01-06 | `full`    | actual only              | ordinary day |
 * | 2026-01-07 | `full`    | actual only              | ordinary day |
 *
 * `summary` is the exact aggregate of the covered buckets above.
 *
 * `breakdown` — 4 rows: two share `(kiro-auth, claude-opus-5-max)` and differ only by
 * `variant` (`"xhigh"` vs `null`) so variant expansion is assertable; the other two are
 * a `null`-variant row and a `"high"`-variant row on different agents.
 *
 * `messages` — {@link MOCK_MESSAGE_TOTAL} rows (> 50) generated deterministically, so
 * `query_messages` paging is real: the mock slices by `limit`/`offset` and always reports
 * `totalCount = MOCK_MESSAGE_TOTAL`.
 *
 * `hosts` / `refreshStatus` — two hosts: `local-host-000001` (`local`, idle) and
 * `ssh-host-0000002` (`ssh`, `state: "error"`) whose `last_error` carries a Chinese
 * remediation string mirroring `agentlens_core::transport::ssh` error text.
 */
import type {
  AppSettings,
  BreakdownRow,
  CostTotals,
  CoverageStatus,
  Host,
  IpcError,
  MessagePage,
  MessageRow,
  PriceTable,
  SeriesPoint,
  SourceStatus,
  Summary,
  TokenValues,
  TriggerRefreshResult,
} from '@/generated'
import { IPC_COMMANDS, type IpcCommand } from '@/lib/ipc'

export const MOCK_IPC_GLOBAL = '__AGENTLENS_MOCK_IPC__'
export const MOCK_IPC_CONFIG_GLOBAL = '__AGENTLENS_MOCK_IPC_CONFIG__'

/** Row count behind `query_messages`; deliberately above one page of 50. */
export const MOCK_MESSAGE_TOTAL = 137

/** The mock report timezone, mirrored into `settings` so the shell hydrates predictably. */
export const MOCK_TIMEZONE = 'UTC'

/**
 * Internal command names `@tauri-apps/api/event`'s `listen()` / `unlisten()` invoke. Not part
 * of {@link IPC_COMMANDS} — they belong to Tauri's event plugin, not to this application.
 */
const EVENT_LISTEN_COMMAND = 'plugin:event|listen'
const EVENT_UNLISTEN_COMMAND = 'plugin:event|unlisten'

type MockEventHandler = (event: { event: string; id: number; payload: unknown }) => void

interface MockEventListener {
  event: string
  handler: MockEventHandler
}

export interface MockIpcCall {
  index: number
  command: string
  args: Record<string, unknown>
}

export interface MockIpcDataset {
  summary: Summary
  trend: SeriesPoint[]
  breakdown: BreakdownRow[]
  messages: MessageRow[]
  hosts: Host[]
  refreshStatus: SourceStatus[]
  settings: AppSettings
  prices: PriceTable
}

export interface MockIpcConfig {
  /** Replace whole slices of the seeded dataset. */
  dataset?: Partial<MockIpcDataset>
  /** Force a literal return value for a command, bypassing the dataset handler. */
  responses?: Partial<Record<IpcCommand, unknown>>
  /** Force a command to reject with a structured `IpcError`. */
  errors?: Partial<Record<IpcCommand, IpcError>>
}

export interface MockIpcController {
  /** Every recorded call, in issue order. */
  calls(): MockIpcCall[]
  callsFor(command: IpcCommand | string): MockIpcCall[]
  /** Args of the most recent call to `command`, or `undefined` if never called. */
  lastArgs(command: IpcCommand | string): Record<string, unknown> | undefined
  resetCalls(): void
  setResponse(command: IpcCommand, value: unknown): void
  clearResponse(command: IpcCommand): void
  setError(command: IpcCommand, error: IpcError): void
  clearError(command: IpcCommand): void
  setDataset(patch: Partial<MockIpcDataset>): void
  dataset(): MockIpcDataset
  /** Deliver a Tauri event to every `listen()` subscriber; returns how many were notified. */
  emitEvent(event: string, payload?: unknown): number
}

declare global {
  interface Window {
    [MOCK_IPC_GLOBAL]?: MockIpcController
    [MOCK_IPC_CONFIG_GLOBAL]?: MockIpcConfig
  }
}

// ---------------------------------------------------------------------------
// Seeded dataset
// ---------------------------------------------------------------------------

const DAY_MS = 86_400_000
/** 2026-01-01T00:00:00Z */
const BASE_UTC_MS = Date.UTC(2026, 0, 1)

function tokens(
  tokInput: number,
  tokOutput: number,
  tokReasoning: number,
  tokCacheRead: number,
  tokCacheWrite: number,
): TokenValues {
  return {
    tokInput,
    tokOutput,
    tokReasoning,
    tokCacheRead,
    tokCacheWrite,
    totalInput: tokInput + tokCacheRead + tokCacheWrite,
  }
}

function cost(actualSum: number, estimatedSum: number, unavailableCount: number): CostTotals {
  return { actualSum, estimatedSum, unavailableCount }
}

function dayBucket(dayIndex: number) {
  const startUtcMs = BASE_UTC_MS + dayIndex * DAY_MS
  const isoDate = new Date(startUtcMs).toISOString().slice(0, 10)
  return { startUtcMs, endUtcMs: startUtcMs + DAY_MS, label: isoDate }
}

function seriesPoint(
  dayIndex: number,
  coverage: CoverageStatus,
  payload: { tokens: TokenValues; cost: CostTotals; messageCount: number } | null,
): SeriesPoint {
  return {
    bucket: dayBucket(dayIndex),
    coverage,
    tokens: payload?.tokens ?? null,
    cost: payload?.cost ?? null,
    messageCount: payload?.messageCount ?? null,
  }
}

const TREND: SeriesPoint[] = [
  // Uncovered: null payload → the chart must break the line, not plot 0.
  seriesPoint(0, 'none', null),
  // Half-covered: known aggregates are kept and the bucket is flagged partial.
  seriesPoint(1, 'partial', {
    tokens: tokens(41_000, 3_100, 0, 12_000, 900),
    cost: cost(0.004, 0, 0),
    messageCount: 12,
  }),
  // Covered but idle: zeros are real data and must plot as 0.
  seriesPoint(2, 'full', { tokens: tokens(0, 0, 0, 0, 0), cost: cost(0, 0, 0), messageCount: 0 }),
  // Mixed-cost day: one actual cost plus one row with no trustworthy cost.
  seriesPoint(3, 'full', {
    tokens: tokens(120_500, 9_400, 1_200, 88_000, 4_100),
    cost: cost(0.0102, 0, 1),
    messageCount: 31,
  }),
  // Estimate-only day.
  seriesPoint(4, 'full', {
    tokens: tokens(70_250, 5_800, 640, 40_500, 2_000),
    cost: cost(0, 0.0075, 0),
    messageCount: 18,
  }),
  seriesPoint(5, 'full', {
    tokens: tokens(96_000, 7_200, 0, 61_000, 3_300),
    cost: cost(0.0208, 0, 0),
    messageCount: 27,
  }),
  seriesPoint(6, 'full', {
    tokens: tokens(58_400, 4_050, 310, 29_700, 1_450),
    cost: cost(0.0134, 0, 0),
    messageCount: 21,
  }),
]

const SUMMARY: Summary = {
  tokens: tokens(386_150, 29_550, 2_150, 231_200, 11_750),
  cost: cost(0.0484, 0.0075, 1),
  messageCount: 109,
  activeSessionCount: 14,
}

const BREAKDOWN: BreakdownRow[] = [
  {
    source: 'opencode',
    agentKey: 'atlas-plan-executor',
    agentRaw: 'Atlas - Plan Executor',
    providerId: 'kiro-auth',
    modelId: 'claude-opus-5-max',
    variant: 'xhigh',
    tokens: tokens(180_000, 12_400, 1_500, 120_000, 5_200),
    cost: cost(0.0301, 0, 1),
    messageCount: 44,
    activeSessionCount: 6,
  },
  {
    source: 'opencode',
    agentKey: 'atlas-plan-executor',
    agentRaw: 'Atlas - Plan Executor',
    providerId: 'kiro-auth',
    modelId: 'claude-opus-5-max',
    variant: null,
    tokens: tokens(96_150, 7_050, 0, 61_200, 3_050),
    cost: cost(0.0119, 0, 0),
    messageCount: 30,
    activeSessionCount: 4,
  },
  {
    source: 'opencode',
    agentKey: 'build',
    agentRaw: 'build',
    providerId: 'openai',
    modelId: 'gpt-5-codex',
    variant: null,
    tokens: tokens(78_000, 6_300, 650, 35_000, 2_400),
    cost: cost(0.0064, 0.0075, 0),
    messageCount: 24,
    activeSessionCount: 3,
  },
  {
    source: 'opencode',
    agentKey: 'research-assistant',
    agentRaw: 'Research Assistant',
    providerId: 'anthropic',
    modelId: 'claude-sonnet-5',
    variant: 'high',
    tokens: tokens(32_000, 3_800, 0, 15_000, 1_100),
    cost: cost(0, 0, 0),
    messageCount: 11,
    activeSessionCount: 1,
  },
]

const MESSAGE_SHAPES = [
  { providerId: 'kiro-auth', modelId: 'claude-opus-5-max', variant: 'xhigh' as string | null },
  { providerId: 'kiro-auth', modelId: 'claude-opus-5-max', variant: null },
  { providerId: 'openai', modelId: 'gpt-5-codex', variant: null },
  { providerId: 'anthropic', modelId: 'claude-sonnet-5', variant: 'high' as string | null },
]

const MESSAGE_AGENTS = [
  { agentKey: 'atlas-plan-executor', agentRaw: 'Atlas - Plan Executor' },
  { agentKey: 'build', agentRaw: 'build' },
  { agentKey: 'research-assistant', agentRaw: 'Research Assistant' },
]

/**
 * Deterministic message rows: index-derived so a spec can predict any row.
 * Cost cycles actual → estimated → unavailable, and every 11th row is incomplete.
 */
function buildMessages(total: number): MessageRow[] {
  const rows: MessageRow[] = []
  for (let index = 0; index < total; index += 1) {
    const shape = MESSAGE_SHAPES[index % MESSAGE_SHAPES.length]
    const agent = MESSAGE_AGENTS[index % MESSAGE_AGENTS.length]
    const costMode = index % 3
    rows.push({
      hostId: index % 5 === 0 ? 'ssh-host-0000002' : 'local-host-000001',
      source: 'opencode',
      messageId: `msg_mock_${String(index).padStart(4, '0')}`,
      sessionId: `ses_mock_${String(Math.floor(index / 7)).padStart(3, '0')}`,
      timeCreatedUtc: BASE_UTC_MS + 3 * DAY_MS + index * 60_000,
      agentRaw: agent.agentRaw,
      agentKey: agent.agentKey,
      providerId: shape.providerId,
      modelId: shape.modelId,
      variant: shape.variant,
      tokens: tokens(1_000 + index * 13, 40 + index, index % 4 === 0 ? index : 0, index * 7, index),
      cost:
        costMode === 0
          ? { actual: 0.0004 + index / 100_000, estimated: null, unavailable: false }
          : costMode === 1
            ? { actual: null, estimated: 0.0002 + index / 200_000, unavailable: false }
            : { actual: null, estimated: null, unavailable: true },
      isIncomplete: index % 11 === 0,
      projectDir: index % 2 === 0 ? '/workspace/AgentLens' : '/workspace/other-project',
    })
  }
  return rows
}

const HOSTS: Host[] = [
  {
    hostId: 'local-host-000001',
    machineIdHash: 'a'.repeat(64),
    displayName: 'workstation',
    kind: 'local',
    sshTarget: null,
    remoteDataDir: null,
    lastSuccessUtc: BASE_UTC_MS + 6 * DAY_MS,
  },
  {
    hostId: 'ssh-host-0000002',
    machineIdHash: 'b'.repeat(64),
    displayName: 'build-box',
    kind: 'ssh',
    sshTarget: 'ci@build-box.internal',
    remoteDataDir: '/srv/opencode',
    lastSuccessUtc: BASE_UTC_MS + 4 * DAY_MS,
  },
]

/**
 * Chinese remediation text, mirroring `agentlens_core::transport::ssh`'s `AuthFailed`
 * remediation. It lives in fixture data (not in `zh.ts`) because at runtime the string
 * arrives from the backend; `scripts/check-i18n.mjs` allowlists this file for that reason.
 */
const SSH_AUTH_FAILED_REMEDIATION =
  'SSH 认证失败：Permission denied (publickey)。请检查密钥路径与远端 authorized_keys，或改用密钥文件登录。'

const REFRESH_STATUS: SourceStatus[] = [
  {
    hostId: 'local-host-000001',
    displayName: 'workstation',
    kind: 'local',
    state: { state: 'idle' },
    trigger: 'auto',
    lastError: null,
    lastSuccessUtc: BASE_UTC_MS + 6 * DAY_MS,
    lastCompletedUtc: BASE_UTC_MS + 6 * DAY_MS,
    lastDurationMs: 903,
    intervalMs: 300_000,
    nextDueUtc: BASE_UTC_MS + 6 * DAY_MS + 300_000,
    interrupted: false,
    cursorTimeUpdated: BASE_UTC_MS + 6 * DAY_MS,
  },
  {
    hostId: 'ssh-host-0000002',
    displayName: 'build-box',
    kind: 'ssh',
    state: {
      state: 'error',
      last_error: SSH_AUTH_FAILED_REMEDIATION,
      last_success: BASE_UTC_MS + 4 * DAY_MS,
    },
    trigger: 'manual',
    lastError: SSH_AUTH_FAILED_REMEDIATION,
    lastSuccessUtc: BASE_UTC_MS + 4 * DAY_MS,
    lastCompletedUtc: BASE_UTC_MS + 5 * DAY_MS,
    lastDurationMs: 23_303,
    intervalMs: 900_000,
    nextDueUtc: null,
    interrupted: true,
    cursorTimeUpdated: BASE_UTC_MS + 4 * DAY_MS,
  },
]

const SETTINGS: AppSettings = {
  values: {
    'report.timezone': MOCK_TIMEZONE,
    'report.weekStart': 'monday',
    'refresh.localIntervalMs': '300000',
    'refresh.remoteIntervalMs': '900000',
  },
}

const PRICES: PriceTable = {
  schemaVersion: 1,
  entries: [
    {
      providerId: 'kiro-auth',
      modelId: 'claude-opus-5-max',
      inputPerMtok: 3,
      outputPerMtok: 15,
      cacheReadPerMtok: 0.3,
      cacheWritePerMtok: 3.75,
      extra: {},
    },
    {
      providerId: 'openai',
      modelId: 'gpt-5-codex',
      inputPerMtok: 1.25,
      outputPerMtok: 10,
      cacheReadPerMtok: 0.125,
      cacheWritePerMtok: 1.5625,
      extra: {},
    },
  ],
  extra: {},
}

/** Deep-ish clone so a mutating consumer can never corrupt the shared seed. */
function cloneDataset(dataset: MockIpcDataset): MockIpcDataset {
  return structuredClone(dataset)
}

export function mockDataset(): MockIpcDataset {
  return cloneDataset({
    summary: SUMMARY,
    trend: TREND,
    breakdown: BREAKDOWN,
    messages: buildMessages(MOCK_MESSAGE_TOTAL),
    hosts: HOSTS,
    refreshStatus: REFRESH_STATUS,
    settings: SETTINGS,
    prices: PRICES,
  })
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

function asNumber(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function paginate(rows: MessageRow[], limit: number, offset: number): MessagePage {
  const clampedLimit = Math.min(Math.max(limit, 0), 200)
  const clampedOffset = Math.max(offset, 0)
  return {
    rows: rows.slice(clampedOffset, clampedOffset + clampedLimit),
    totalCount: rows.length,
    limit: clampedLimit,
    offset: clampedOffset,
  }
}

/**
 * Install the mock over `window.__TAURI_INTERNALS__` and publish the controller on
 * `window.__AGENTLENS_MOCK_IPC__`. Reads `window.__AGENTLENS_MOCK_IPC_CONFIG__` (set from a
 * Playwright `addInitScript`) so a spec can configure errors before the app's first render.
 */
function notFound(kind: string, id: string): IpcError {
  return { code: 'notFound', message: `${kind} ${id} not found`, fields: { kind, id } }
}

/** Mutable box so `set_settings` / `prices_set` can behave like real writes. */
interface MockState {
  dataset: MockIpcDataset
}

/**
 * One handler per registered command. Typed as `Record<IpcCommand, …>`, so adding a
 * command to `IPC_COMMANDS` without a mock handler is a **compile error** rather than a
 * runtime surprise for the five view specs.
 */
const HANDLERS: Record<IpcCommand, (state: MockState, args: Record<string, unknown>) => unknown> = {
  get_summary: (state) => state.dataset.summary,
  get_trend: (state) => state.dataset.trend,
  get_breakdown: (state) => state.dataset.breakdown,
  query_messages: (state, args) =>
    paginate(state.dataset.messages, asNumber(args.limit, 50), asNumber(args.offset, 0)),
  hosts_list: (state) => state.dataset.hosts,
  hosts_get: (state, args) => {
    const host = state.dataset.hosts.find((candidate) => candidate.hostId === args.hostId)
    if (host === undefined) throw notFound('host', String(args.hostId))
    return host
  },
  hosts_create: (state, args) => ({ ...state.dataset.hosts[0], ...(args.input as object) }),
  hosts_update: (state, args) => ({ ...state.dataset.hosts[0], ...(args.input as object) }),
  hosts_delete: () => null,
  trigger_refresh: (_state, args) =>
    ({
      outcome: 'started',
      host_id: String(args.hostId),
      started_at_utc: BASE_UTC_MS + 7 * DAY_MS,
    }) satisfies TriggerRefreshResult,
  get_refresh_status: (state) => state.dataset.refreshStatus,
  get_settings: (state) => state.dataset.settings,
  set_settings: (state, args) => {
    const incoming = (args.settings as AppSettings | undefined)?.values ?? {}
    state.dataset.settings = { values: { ...state.dataset.settings.values, ...incoming } }
    return state.dataset.settings
  },
  prices_get: (state) => state.dataset.prices,
  prices_set: (state, args) => {
    state.dataset.prices = args.prices as PriceTable
    return state.dataset.prices
  },
}

function isIpcCommand(command: string): command is IpcCommand {
  return (IPC_COMMANDS as readonly string[]).includes(command)
}

export function installMockIpc(config: MockIpcConfig = {}): MockIpcController {
  const merged: MockIpcConfig = { ...window[MOCK_IPC_CONFIG_GLOBAL], ...config }
  const state: MockState = { dataset: { ...mockDataset(), ...merged.dataset } }
  const responses = new Map<string, unknown>(Object.entries(merged.responses ?? {}))
  const errors = new Map<string, IpcError>(
    Object.entries(merged.errors ?? {}) as [string, IpcError][],
  )
  let calls: MockIpcCall[] = []

  const listeners = new Map<number, MockEventListener>()
  let lastListenerId = 0

  /**
   * The only removal path, shared by `plugin:event|unlisten` and by the event plugin global
   * installed below. `@tauri-apps/api`'s `_unlisten()` drives **both** in sequence, so if the
   * mock kept two registries they could disagree about who is still subscribed.
   */
  const unregisterListener = (eventId: number) => {
    listeners.delete(eventId)
  }

  const invoke = async (command: string, args: Record<string, unknown> = {}): Promise<unknown> => {
    // The event plugin is served before the recording below: `listen()` hands the subscriber
    // function straight through the mock `transformCallback`, and `structuredClone` throws a
    // DataCloneError on a function.
    if (command === EVENT_LISTEN_COMMAND) {
      lastListenerId += 1
      if (typeof args.handler === 'function') {
        listeners.set(lastListenerId, {
          event: String(args.event),
          handler: args.handler as MockEventHandler,
        })
      }
      return lastListenerId
    }
    if (command === EVENT_UNLISTEN_COMMAND) {
      unregisterListener(asNumber(args.eventId, -1))
      return null
    }

    calls.push({ index: calls.length, command, args: structuredClone(args) })
    const forcedError = errors.get(command)
    if (forcedError !== undefined) {
      throw forcedError
    }
    if (responses.has(command)) {
      return responses.get(command)
    }
    if (!isIpcCommand(command)) {
      throw {
        code: 'internal',
        message: `mock IPC has no handler for command "${command}"`,
        fields: { command },
      } satisfies IpcError
    }
    return HANDLERS[command](state, args)
  }

  const internals = {
    invoke,
    transformCallback: (callback: unknown) => callback,
    unregisterCallback: () => undefined,
    convertFileSrc: (filePath: string) => filePath,
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    plugins: {},
  }
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = internals

  /**
   * `@tauri-apps/api/event` reads a **second, differently named** global before it invokes
   * `plugin:event|unlisten`: `window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener(event,
   * eventId)` (`node_modules/@tauri-apps/api/event.js:43`). It is injected by Rust in a real
   * webview and is never part of `__TAURI_INTERNALS__`, so installing only that one made
   * `listen()` resolve while every returned `UnlistenFn` rejected with `TypeError: Cannot read
   * properties of undefined (reading 'unregisterListener')` — a failure that existed only under
   * this mock. The declared surface is exactly this one synchronous member (`event.d.ts:1-7`),
   * and Tauri's own `mockIPC` ignores the event name and removes by id (`mocks.js:157`), so this
   * mirrors it and shares {@link unregisterListener} with the command branch above: same order
   * as the real plugin, and `Map.delete` is idempotent, so the pair never double-frees.
   */
  const eventPluginInternals: { unregisterListener: (event: string, eventId: number) => void } = {
    unregisterListener: (_event, eventId) => {
      unregisterListener(eventId)
    },
  }
  ;(window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ =
    eventPluginInternals

  const controller: MockIpcController = {
    calls: () => [...calls],
    callsFor: (command) => calls.filter((call) => call.command === command),
    lastArgs: (command) => {
      const matching = calls.filter((call) => call.command === command)
      return matching.length === 0 ? undefined : matching[matching.length - 1].args
    },
    resetCalls: () => {
      calls = []
    },
    setResponse: (command, value) => {
      responses.set(command, value)
    },
    clearResponse: (command) => {
      responses.delete(command)
    },
    setError: (command, error) => {
      errors.set(command, error)
    },
    clearError: (command) => {
      errors.delete(command)
    },
    setDataset: (patch) => {
      state.dataset = { ...state.dataset, ...patch }
    },
    dataset: () => cloneDataset(state.dataset),
    emitEvent: (event, payload) => {
      let notified = 0
      for (const [id, listener] of listeners) {
        if (listener.event !== event) continue
        notified += 1
        listener.handler({ event, id, payload })
      }
      return notified
    },
  }

  window[MOCK_IPC_GLOBAL] = controller
  return controller
}
