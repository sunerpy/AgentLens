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
 * Every seeded `sessionRecordCount` is 0 — the common installation, where `enabled_sources`
 * carries only `'opencode'`. {@link SESSION_GRANULARITY_DATASET} is the opt-in overlay for the
 * mixed-granularity case; apply it with `setDataset` instead of editing the numbers above.
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
  AggregateFilters,
  AppSettings,
  BreakdownRow,
  CostTotals,
  CoverageNote,
  CoverageStatus,
  DiagnosticsReport,
  Host,
  IpcError,
  LogEntry,
  LogTail,
  MessagePage,
  MessageRow,
  PriceCatalog,
  PriceTable,
  RefreshEvent,
  SeriesGroup,
  SeriesGroupDimension,
  SeriesPoint,
  SeriesQueryResult,
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
  callbackId: number
  handler: MockEventHandler
}

export interface MockIpcCall {
  index: number
  command: string
  args: Record<string, unknown>
}

export interface MockIpcDataset {
  summary: Summary
  trend: SeriesQueryResult
  breakdown: BreakdownRow[]
  messages: MessageRow[]
  hosts: Host[]
  refreshStatus: SourceStatus[]
  settings: AppSettings
  priceCatalog: PriceCatalog
  prices: PriceTable
  logs: LogTail
  diagnostics: DiagnosticsReport
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

/**
 * `sessionRecordCount` defaults to 0 across the base seed on purpose: the archive only carries
 * session-granularity records when a session-only source is enabled, and `enabled_sources`
 * defaults to `'opencode'` alone. The seed therefore models the common installation, and the
 * mixed-granularity shape lives in {@link SESSION_GRANULARITY_DATASET} instead of perturbing
 * the numbers the five view specs already assert.
 */
function seriesPoint(
  dayIndex: number,
  coverage: CoverageStatus,
  payload: {
    tokens: TokenValues
    cost: CostTotals
    messageCount: number
    sessionRecordCount?: number
  } | null,
): SeriesPoint {
  return {
    bucket: dayBucket(dayIndex),
    coverage,
    tokens: payload?.tokens ?? null,
    cost: payload?.cost ?? null,
    messageCount: payload?.messageCount ?? null,
    sessionRecordCount: payload === null ? null : (payload.sessionRecordCount ?? 0),
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
  costCoverage: {
    actual: { recordCount: 90, billableTokens: 538_000 },
    estimated: { recordCount: 18, billableTokens: 118_550 },
    unavailable: { recordCount: 1, billableTokens: 2_100 },
  },
  messageCount: 109,
  sessionRecordCount: 0,
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
    sessionRecordCount: 0,
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
    sessionRecordCount: 0,
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
    sessionRecordCount: 0,
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
    sessionRecordCount: 0,
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
    enabledSources: ['opencode'],
  },
  {
    hostId: 'ssh-host-0000002',
    machineIdHash: 'b'.repeat(64),
    displayName: 'build-box',
    kind: 'ssh',
    sshTarget: 'ci@build-box.internal',
    remoteDataDir: '/srv/opencode',
    lastSuccessUtc: BASE_UTC_MS + 4 * DAY_MS,
    // Two sources so the seeded fixture exercises the mixed per-source state the scheduler's
    // `(host_id, source)` keying makes possible: one idle slot beside one failing slot.
    enabledSources: ['opencode', 'claude-code'],
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
    source: 'opencode',
    displayName: 'workstation',
    kind: 'local',
    state: { state: 'idle' },
    trigger: 'auto',
    lastError: null,
    lastSuccessUtc: BASE_UTC_MS + 6 * DAY_MS,
    lastCompletedUtc: BASE_UTC_MS + 6 * DAY_MS,
    lastDurationMs: 903,
    intervalMs: 600_000,
    nextDueUtc: BASE_UTC_MS + 6 * DAY_MS + 600_000,
    interrupted: false,
    cursorTimeUpdated: BASE_UTC_MS + 6 * DAY_MS,
  },
  // The build box's OpenCode slot is healthy while its Claude Code slot below is failing. Both
  // must render, which is what makes "OpenCode 空闲 / Claude Code 出错" assertable.
  {
    hostId: 'ssh-host-0000002',
    source: 'opencode',
    displayName: 'build-box',
    kind: 'ssh',
    state: { state: 'idle' },
    trigger: 'auto',
    lastError: null,
    lastSuccessUtc: BASE_UTC_MS + 4 * DAY_MS,
    lastCompletedUtc: BASE_UTC_MS + 4 * DAY_MS,
    lastDurationMs: 1_204,
    intervalMs: 900_000,
    nextDueUtc: BASE_UTC_MS + 4 * DAY_MS + 900_000,
    interrupted: false,
    cursorTimeUpdated: BASE_UTC_MS + 4 * DAY_MS,
  },
  {
    hostId: 'ssh-host-0000002',
    source: 'claude-code',
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

const PRICE_CATALOG: PriceCatalog = {
  schemaVersion: 1,
  catalogVersion: '2026-08-07.1',
  updatedAt: '2026-08-07',
  currency: 'USD',
  entries: [
    {
      providerId: 'anthropic',
      modelId: 'claude-sonnet-4-5-20250929',
      inputPerMtok: 3,
      outputPerMtok: 15,
      cacheReadPerMtok: 0.3,
      cacheWritePerMtok: 3.75,
      extra: {},
    },
    {
      providerId: 'anthropic',
      modelId: 'claude-opus-5',
      inputPerMtok: 5,
      outputPerMtok: 25,
      cacheReadPerMtok: 0.5,
      cacheWritePerMtok: 6.25,
      extra: {},
    },
    {
      providerId: 'openai',
      modelId: 'gpt-5',
      inputPerMtok: 1.25,
      outputPerMtok: 10,
      cacheReadPerMtok: 0.125,
      cacheWritePerMtok: 1.25,
      extra: {},
    },
    {
      providerId: 'google',
      modelId: 'gemini-2.5-pro',
      inputPerMtok: 1.25,
      outputPerMtok: 10,
      cacheReadPerMtok: 0.125,
      cacheWritePerMtok: 1.25,
      extra: {},
    },
    {
      providerId: 'amazon-bedrock',
      modelId: 'anthropic.claude-sonnet-4-5-20250929-v1:0',
      inputPerMtok: 3,
      outputPerMtok: 15,
      cacheReadPerMtok: 0.3,
      cacheWritePerMtok: 3.75,
      extra: {},
    },
  ],
  observedModels: [
    {
      providerId: 'kiro-auth',
      modelId: 'claude-opus-5-high',
      usageCount: 30_904,
      matchKind: 'crossProvider',
      matchedPrice: {
        providerId: 'anthropic',
        modelId: 'claude-opus-5',
        inputPerMtok: 5,
        outputPerMtok: 25,
        cacheReadPerMtok: 0.5,
        cacheWritePerMtok: 6.25,
        extra: {},
      },
    },
    {
      providerId: 'aws',
      modelId: 'us.anthropic.claude-sonnet-4-5-20250929-v1:0',
      usageCount: 12,
      matchKind: 'normalized',
      matchedPrice: {
        providerId: 'amazon-bedrock',
        modelId: 'anthropic.claude-sonnet-4-5-20250929-v1:0',
        inputPerMtok: 3,
        outputPerMtok: 15,
        cacheReadPerMtok: 0.3,
        cacheWritePerMtok: 3.75,
        extra: {},
      },
    },
    {
      providerId: 'private-provider',
      modelId: 'private-model-v7',
      usageCount: 3,
      matchKind: 'unknown',
      matchedPrice: null,
    },
  ],
}

/**
 * Log seed — one record per level, newest last so `logs_tail`'s newest-first contract is
 * assertable, plus a WARN whose message contains a colon and a brace to prove the viewer does
 * not re-split an already-parsed record.
 */
const LOG_ENTRIES: LogEntry[] = [
  {
    timestamp: '2026-08-07T09:58:01.004+08:00',
    level: 'trace',
    target: 'agentlens_tauri_lib::state',
    message: 'scheduler tick admitted 0 actions',
  },
  {
    timestamp: '2026-08-07T09:58:02.117+08:00',
    level: 'debug',
    target: 'agentlens_tauri_lib::commands',
    message: 'unable to send refresh progress: channel closed',
  },
  {
    timestamp: '2026-08-07T09:58:03.220+08:00',
    level: 'info',
    target: 'agentlens_tauri_lib::tray',
    message: 'tray icon installed (open / refresh / quit)',
  },
  {
    timestamp: '2026-08-07T09:58:04.331+08:00',
    level: 'warn',
    target: 'agentlens_tauri_lib::tray',
    message: 'unable to apply refresh interval: {clamped: 300000}',
  },
  {
    timestamp: '2026-08-07T09:58:05.442+08:00',
    level: 'error',
    target: 'agentlens_tauri_lib::tray',
    message: 'archive unavailable: database is locked',
  },
]

const LOGS: LogTail = {
  directory: '/home/mock/.local/share/top.onethinker.agentlens/logs',
  entries: [...LOG_ENTRIES].reverse(),
  empty: false,
}

const DIAGNOSTICS: DiagnosticsReport = {
  appVersion: '0.1.0',
  os: 'linux',
  arch: 'x86_64',
  webviewVersion: '2.48.1',
}

/**
 * Mixed-granularity overlay applied through `setDataset`: the base seed's message-level rows plus
 * a `hermes` source whose rows are session rollups (`messageCount 0`, `sessionRecordCount 7`).
 * That row shape — non-zero tokens and cost that the message count does not account for — is
 * exactly what the granularity copy has to explain, and scaling the base seed cannot produce it.
 * Totals reconcile: the extra `tokens(64_000, 8_800, 0, 22_400, 1_600)` and `0.0127` actual cost
 * are folded into both `summary` and buckets 3-4 of `trend`.
 */
const SESSION_TREND: SeriesPoint[] = [
  seriesPoint(0, 'none', null),
  seriesPoint(1, 'partial', {
    tokens: tokens(41_000, 3_100, 0, 12_000, 900),
    cost: cost(0.004, 0, 0),
    messageCount: 12,
  }),
  seriesPoint(2, 'full', {
    tokens: tokens(0, 0, 0, 0, 0),
    cost: cost(0, 0, 0),
    messageCount: 0,
  }),
  seriesPoint(3, 'full', {
    tokens: tokens(152_500, 13_800, 1_200, 99_200, 4_900),
    cost: cost(0.0166, 0, 1),
    messageCount: 31,
    sessionRecordCount: 3,
  }),
  seriesPoint(4, 'full', {
    tokens: tokens(102_250, 10_200, 640, 51_700, 2_800),
    cost: cost(0.0063, 0.0075, 0),
    messageCount: 18,
    sessionRecordCount: 4,
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

const SESSION_BREAKDOWN: BreakdownRow[] = [
  ...BREAKDOWN,
  {
    source: 'hermes',
    agentKey: 'hermes-session',
    agentRaw: 'Hermes Session',
    providerId: 'anthropic',
    modelId: 'claude-sonnet-5',
    variant: null,
    tokens: tokens(64_000, 8_800, 0, 22_400, 1_600),
    cost: cost(0.0127, 0, 0),
    messageCount: 0,
    sessionRecordCount: 7,
    activeSessionCount: 7,
  },
]

export const SESSION_GRANULARITY_DATASET: Pick<MockIpcDataset, 'summary' | 'trend' | 'breakdown'> =
  {
    summary: {
      tokens: tokens(450_150, 38_350, 2_150, 253_600, 13_350),
      cost: cost(0.0611, 0.0075, 1),
      costCoverage: {
        actual: { recordCount: 97, billableTokens: 634_800 },
        estimated: { recordCount: 18, billableTokens: 118_550 },
        unavailable: { recordCount: 1, billableTokens: 2_100 },
      },
      messageCount: 109,
      sessionRecordCount: 7,
      activeSessionCount: 21,
    },
    trend: buildTrendResult(SESSION_TREND, SESSION_BREAKDOWN),
    breakdown: SESSION_BREAKDOWN,
  }

/** Deep-ish clone so a mutating consumer can never corrupt the shared seed. */
function cloneDataset(dataset: MockIpcDataset): MockIpcDataset {
  return structuredClone(dataset)
}

export function mockDataset(): MockIpcDataset {
  return cloneDataset({
    summary: SUMMARY,
    trend: buildTrendResult(TREND, BREAKDOWN),
    breakdown: BREAKDOWN,
    messages: buildMessages(MOCK_MESSAGE_TOTAL),
    hosts: HOSTS,
    refreshStatus: REFRESH_STATUS,
    settings: SETTINGS,
    priceCatalog: PRICE_CATALOG,
    prices: PRICES,
    logs: LOGS,
    diagnostics: DIAGNOSTICS,
  })
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

function asNumber(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function tokenWeight(tokens: TokenValues): number {
  return (
    tokens.tokInput +
    tokens.tokOutput +
    tokens.tokReasoning +
    tokens.tokCacheRead +
    tokens.tokCacheWrite
  )
}

function matchesFilters(row: BreakdownRow, filters: AggregateFilters): boolean {
  return (
    (filters.source === null || row.source === filters.source) &&
    (filters.agentKey === null || row.agentKey === filters.agentKey) &&
    (filters.providerId === null || row.providerId === filters.providerId) &&
    (filters.modelId === null || row.modelId === filters.modelId)
  )
}

function filterShare(rows: BreakdownRow[], filters: unknown): number {
  if (filters === null || typeof filters !== 'object') return 1
  const narrowed = filters as AggregateFilters
  if (
    narrowed.source === null &&
    narrowed.agentKey === null &&
    narrowed.providerId === null &&
    narrowed.modelId === null
  ) {
    return 1
  }
  const total = rows.reduce((sum, row) => sum + tokenWeight(row.tokens), 0)
  if (total <= 0) return 0
  const matched = rows
    .filter((row) => matchesFilters(row, narrowed))
    .reduce((sum, row) => sum + tokenWeight(row.tokens), 0)
  return matched / total
}

function scaleSeries(points: SeriesPoint[], share: number): SeriesPoint[] {
  if (share === 1) return points
  return points.map((point) => ({
    ...point,
    tokens:
      point.tokens === null
        ? null
        : tokens(
            Math.round(point.tokens.tokInput * share),
            Math.round(point.tokens.tokOutput * share),
            Math.round(point.tokens.tokReasoning * share),
            Math.round(point.tokens.tokCacheRead * share),
            Math.round(point.tokens.tokCacheWrite * share),
          ),
    cost:
      point.cost === null
        ? null
        : cost(
            point.cost.actualSum * share,
            point.cost.estimatedSum * share,
            Math.round(point.cost.unavailableCount * share),
          ),
    messageCount: point.messageCount === null ? null : Math.round(point.messageCount * share),
    sessionRecordCount:
      point.sessionRecordCount === null ? null : Math.round(point.sessionRecordCount * share),
  }))
}

function buildTrendResult(total: SeriesPoint[], rows: BreakdownRow[]): SeriesQueryResult {
  const groups = new Map<
    string,
    {
      dimension: SeriesGroupDimension
      id: string
      label: string
      filters: AggregateFilters
    }
  >()
  const noFilters: AggregateFilters = {
    hostId: null,
    source: null,
    agentKey: null,
    providerId: null,
    modelId: null,
  }
  const add = (
    dimension: SeriesGroupDimension,
    id: string,
    label: string,
    filters: AggregateFilters,
  ) => groups.set(`${dimension}\u0000${id}`, { dimension, id, label, filters })

  for (const row of rows) {
    add('source', row.source, row.source, { ...noFilters, source: row.source })
    add('agent', row.agentKey, row.agentRaw, { ...noFilters, agentKey: row.agentKey })
    add('provider', row.providerId, row.providerId, { ...noFilters, providerId: row.providerId })
    add('model', `${row.providerId}\u0000${row.modelId}`, `${row.providerId} / ${row.modelId}`, {
      ...noFilters,
      providerId: row.providerId,
      modelId: row.modelId,
    })
  }

  const grouped: SeriesGroup[] = [...groups.values()].map((group) => ({
    dimension: group.dimension,
    id: group.id,
    label: group.label,
    series: scaleSeries(total, filterShare(rows, group.filters)),
  }))
  return { total, groups: grouped, coverageNotes: buildCoverageNotes(total) }
}

/**
 * Mirrors what a real `CoverageStore` reports for the seeded gap and partial buckets: the partial
 * bucket has one host still collecting and one that never did, the gap bucket has neither.
 */
function buildCoverageNotes(total: SeriesPoint[]): CoverageNote[] {
  return total
    .filter((point) => point.coverage !== 'full')
    .map((point) => ({
      label: point.bucket.label,
      shortfalls:
        point.coverage === 'partial'
          ? [
              { hostId: 'local', source: 'opencode', partial: true },
              { hostId: 'build-box', source: 'codex', partial: false },
            ]
          : [
              { hostId: 'local', source: 'opencode', partial: false },
              { hostId: 'build-box', source: 'codex', partial: false },
            ],
    }))
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
  sendChannel(channel: unknown, message: RefreshEvent): void
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
  /**
   * A real write, not an echo: the updated host replaces its row in the dataset so the
   * following `hosts_list` reports the new value. An echo would let a view that never
   * refetches — or one that reads back the wrong field — still look correct on screen.
   */
  hosts_update: (state, args) => {
    const input = args.input as Partial<Host> & { hostId: string }
    const existing = state.dataset.hosts.find((host) => host.hostId === input.hostId)
    if (existing === undefined) throw notFound('host', input.hostId)
    // `null` is the contract's "leave the stored set alone", so it must not overwrite.
    const { enabledSources, ...rest } = input
    const updated: Host = {
      ...existing,
      ...rest,
      enabledSources: enabledSources ?? existing.enabledSources,
    }
    state.dataset.hosts = state.dataset.hosts.map((host) =>
      host.hostId === updated.hostId ? updated : host,
    )
    return updated
  },
  /** Mirrors `agentlens_core::host::SUPPORTED_SOURCES`, which the command exports verbatim. */
  hosts_supported_sources: () => ['opencode', 'claude-code', 'codex', 'hermes'],
  hosts_delete: () => null,
  /**
   * One round per registered `(hostId, source)` slot, mirroring the real command's
   * `Vec<TriggerRefreshResult>`. Each slot gets its own started/finished pair so a frontend
   * that keys events on `hostId` alone visibly loses one of them.
   */
  trigger_refresh: (state, args) => {
    const hostId = String(args.hostId)
    const slots = state.dataset.refreshStatus.filter((status) => status.hostId === hostId)
    if (slots.length === 0) throw notFound('refresh source', hostId)

    const finishedAt = BASE_UTC_MS + 7 * DAY_MS
    const results: TriggerRefreshResult[] = []

    for (const slot of slots) {
      const matches = (status: SourceStatus) =>
        status.hostId === hostId && status.source === slot.source

      const started: SourceStatus = { ...slot, state: { state: 'running' } }
      state.dataset.refreshStatus = state.dataset.refreshStatus.map((status) =>
        matches(status) ? started : status,
      )
      state.sendChannel(args.onEvent, { event: 'started', data: { status: started } })

      const finished: SourceStatus = {
        ...started,
        state: { state: 'idle' },
        lastError: null,
        lastSuccessUtc: finishedAt,
        lastCompletedUtc: finishedAt,
        interrupted: false,
      }
      state.dataset.refreshStatus = state.dataset.refreshStatus.map((status) =>
        matches(status) ? finished : status,
      )
      state.sendChannel(args.onEvent, {
        event: 'finished',
        data: { hostId, source: slot.source, status: finished },
      })

      results.push({
        outcome: 'started',
        host_id: hostId,
        source: slot.source,
        started_at_utc: finishedAt,
      })
    }

    return results
  },
  get_refresh_status: (state) => state.dataset.refreshStatus,
  get_settings: (state) => state.dataset.settings,
  set_settings: (state, args) => {
    const incoming = (args.settings as AppSettings | undefined)?.values ?? {}
    state.dataset.settings = { values: { ...state.dataset.settings.values, ...incoming } }
    return state.dataset.settings
  },
  price_catalog_get: (state) => state.dataset.priceCatalog,
  prices_get: (state) => state.dataset.prices,
  prices_set: (state, args) => {
    state.dataset.prices = args.prices as PriceTable
    return state.dataset.prices
  },
  logs_tail: (state, args) => {
    const limit = asNumber(args.limit, state.dataset.logs.entries.length)
    const entries = state.dataset.logs.entries.slice(0, Math.max(1, limit))
    return { ...state.dataset.logs, entries, empty: entries.length === 0 }
  },
  diagnostics_report: (state) => state.dataset.diagnostics,
}

function isIpcCommand(command: string): command is IpcCommand {
  return (IPC_COMMANDS as readonly string[]).includes(command)
}

export function installMockIpc(config: MockIpcConfig = {}): MockIpcController {
  const merged: MockIpcConfig = { ...window[MOCK_IPC_CONFIG_GLOBAL], ...config }
  const responses = new Map<string, unknown>(Object.entries(merged.responses ?? {}))
  const errors = new Map<string, IpcError>(
    Object.entries(merged.errors ?? {}) as [string, IpcError][],
  )
  let calls: MockIpcCall[] = []

  const listeners = new Map<number, MockEventListener>()
  let lastListenerId = 0
  const callbacks = new Map<number, { handler: (payload: unknown) => void; once: boolean }>()
  let lastCallbackId = 0
  const channelIndexes = new Map<number, number>()

  const transformCallback = (callback: unknown, once = false): number => {
    lastCallbackId += 1
    if (typeof callback === 'function') {
      callbacks.set(lastCallbackId, {
        handler: callback as (payload: unknown) => void,
        once,
      })
    }
    return lastCallbackId
  }

  const runCallback = (callbackId: number, payload: unknown) => {
    const callback = callbacks.get(callbackId)
    if (callback === undefined) return
    callback.handler(payload)
    if (callback.once) callbacks.delete(callbackId)
  }

  const channelId = (channel: unknown): number | null => {
    const serialized =
      typeof channel === 'string'
        ? channel
        : typeof channel === 'object' && channel !== null && 'toJSON' in channel
          ? String((channel as { toJSON(): unknown }).toJSON())
          : ''
    const match = /^__CHANNEL__:(\d+)$/.exec(serialized)
    return match === null ? null : Number(match[1])
  }

  const sendChannel = (channel: unknown, message: RefreshEvent) => {
    const callbackId = channelId(channel)
    if (callbackId === null) return
    const index = channelIndexes.get(callbackId) ?? 0
    channelIndexes.set(callbackId, index + 1)
    runCallback(callbackId, { index, message })
  }

  const endChannel = (channel: unknown) => {
    const callbackId = channelId(channel)
    if (callbackId === null) return
    runCallback(callbackId, { end: true, index: channelIndexes.get(callbackId) ?? 0 })
    channelIndexes.delete(callbackId)
  }

  const state: MockState = {
    dataset: { ...mockDataset(), ...merged.dataset },
    sendChannel,
  }

  /**
   * The only removal path, shared by `plugin:event|unlisten` and by the event plugin global
   * installed below. `@tauri-apps/api`'s `_unlisten()` drives **both** in sequence, so if the
   * mock kept two registries they could disagree about who is still subscribed.
   */
  const unregisterListener = (eventId: number) => {
    const listener = listeners.get(eventId)
    if (listener !== undefined) callbacks.delete(listener.callbackId)
    listeners.delete(eventId)
  }

  const invoke = async (command: string, args: Record<string, unknown> = {}): Promise<unknown> => {
    // The event plugin is served before the recording below: `listen()` hands the subscriber
    // function straight through the mock `transformCallback`, and `structuredClone` throws a
    // DataCloneError on a function.
    if (command === EVENT_LISTEN_COMMAND) {
      lastListenerId += 1
      const callbackId = asNumber(args.handler, -1)
      const callback = callbacks.get(callbackId)?.handler
      if (callback !== undefined) {
        listeners.set(lastListenerId, {
          event: String(args.event),
          callbackId,
          handler: callback as MockEventHandler,
        })
      }
      return lastListenerId
    }
    if (command === EVENT_UNLISTEN_COMMAND) {
      unregisterListener(asNumber(args.eventId, -1))
      return null
    }

    calls.push({ index: calls.length, command, args: JSON.parse(JSON.stringify(args)) })
    try {
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
    } finally {
      if (command === 'trigger_refresh') endChannel(args.onEvent)
    }
  }

  const internals = {
    invoke,
    transformCallback,
    runCallback,
    unregisterCallback: (callbackId: number) => callbacks.delete(callbackId),
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
