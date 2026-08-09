# Hermes 适配器契约（已实现）

本文档把 Hermes 的 `state.db` 字段映射到 AgentLens 归一化模型
`agentlens_core::archive::NormalizedUsageRecord`（源文件 `crates/agentlens-core/src/archive.rs`，
与归档表 `usage_record` 的列一一对应）。

相关文档：

- 姊妹文档：[`docs/adapters/claude-code.md`](./claude-code.md)、[`docs/adapters/codex.md`](./codex.md)
- 远程接入契约：[`docs/remote-source-api.md`](../remote-source-api.md)

本契约已落地：解析代码在 `crates/agentlens-core/src/source/hermes.rs`，远端路径由采集器
`agentlens-collector --source hermes` 驱动，`SUPPORTED_SOURCES` 已含 `hermes`（现为四元），
调度器以 `(host_id, source)` 为键，同一主机可与 OpenCode、Claude Code、Codex 并行采集。

## 0. Hermes 是会话级源

这是 Hermes 与其他三个适配器的本质区别，先讲清楚，后面所有设计都由它推导：

**Hermes 的消息行不带 token。** 实测一个真实 `state.db`：`messages` 表 158 行，
`token_count` 非 NULL 的有 **0 行**；四种 role（assistant / user / tool / session_meta）
无一例外。五桶真值只存在于 `sessions` 表 —— 9 行会话、四桶合计 2038297，其中 6 行 token 非零。
`messages.token_count` 是一列死列，消息级数据无法重建会话级总量。

**这不是缺陷，而是数据源特性。** 因此归档层引入了记录粒度：`usage_record.granularity`
取 `'message'` 或 `'session'`。OpenCode、Claude Code、Codex 写 `'message'`，
Hermes 写 `'session'`，每个会话归一化成**一条**记录。

聚合层由此分裂成两组口径：

| 指标 | 口径 |
| --- | --- |
| `message_count` | 只数 `granularity='message'` 的记录 |
| `session_record_count` | 只数 `granularity='session'` 的记录 |
| token 五桶 | 跨粒度求和 |
| `cost` | 跨粒度求和 |
| `active_session_count` | 跨粒度 `count(DISTINCT session_id)` |

也就是说 **Hermes 的用量计入 token 与成本，但不计入「消息数」**，改为计入会话汇总记录数。
把这两个数相加是错的，它们计的是不同粒度的东西。

> **开发库必须删除重建。** `granularity` 声明在 `migration_v1` 的**基线 schema**，
> 带 `DEFAULT 'message'`，`LATEST_SCHEMA_VERSION` 仍为 **3**：项目未投产，
> 所以有意不写 v4 迁移。任何在此之前创建的开发用归档库缺少该列，
> 打开会失败，需删除后重建。

## 1. 数据面

| 项 | 值 |
| --- | --- |
| 根目录 | `$HERMES_HOME`，缺省 `~/.hermes/` |
| 数据库 | `state.db`（SQLite） |
| 打开方式 | **只读**（`OpenFlags` 只读位）。同目录常有 `state.db-wal`，说明库在活跃写入 |
| 用量表 | `sessions`，一行一会话 |
| 消息表 | `messages`，`token_count` 全 NULL，**不用于用量** |
| 成本字段 | 存在但不可用（见下） |

`sessions` 的用量列：`input_tokens`、`output_tokens`、`cache_read_tokens`、
`cache_write_tokens`、`reasoning_tokens`；辅助列 `id`、`source`、`model`、`started_at`、
`ended_at`、`billing_provider`、`billing_base_url`。

### 时间戳格式

`started_at` / `ended_at` / `messages.timestamp` 是**秒级浮点**（SQLite 里以形如
`"1778141335.6447966"` 的文本存放），必须换算成毫秒 epoch。整个换算集中在
`epoch_seconds_to_ms`，不可换算的行整条跳过并计入 `unparsable_timestamp`。

### 成本列不可信

Hermes 自己不算钱：实测 `estimated_cost_usd=0.0`、`actual_cost_usd=NULL`、
`cost_status='unknown'`。所以 `cost` 写 `None`、`cost_source` 写 `Unavailable`，
金额一律由 AgentLens 查询期定价解析产出。

## 2. 增量算法要点

1. 一条 SQL 取全部 `sessions`，按 `sessions.id` 排序，逐行归一化。
2. 游标（`source_time_updated`）取该会话 `max(messages.timestamp)`，
   **无 messages 行时才回退 `started_at`**。
3. 水位线窗口向前重叠 24 小时（`OVERLAP_WINDOW_MS`）；落在窗口之前的记录直接丢弃。
4. 批交付（`DEFAULT_BATCH_SIZE = 1000`）；sink 拒绝任一批即置
   `observed_max_time_updated = None` 并返回，游标不前进 —— 宁可下轮重扫，不可跳过。
5. 跳过原因按 `invalid_row` / `missing_session_id` / `unparsable_timestamp` /
   `invalid_tokens` 四类分别计数，任一坏行不中断整次扫描。

### 为什么游标不能用 `started_at`

会话是**可增长**的：实测 9 个会话里有 **3 个 `ended_at` 为 NULL 但已经有真实 token**。
若拿 `started_at` 当游标，一旦水位线越过会话开始时间，此后该会话的 token 增长就
**永久丢失**。`max(messages.timestamp)` 是递进的，实测领先 `started_at` 17s / 77s / 9028s，
能持续把活动会话拉回窗口内。

同理 `is_incomplete` **固定为 `false`**：活动会话与零 token 会话都是有效计量记录，
按其他适配器的「本轮增量全零即 incomplete」处理会把用户正在看的真实用量永久藏起来。

### 去重键

```
message_id = session_id
```

Hermes 一个会话只产一条记录，两者同值。依赖归档既有的
`UNIQUE(host_id, source, message_id)` 与 upsert：**同一会话重复采集时是覆盖，
而不是累加** —— 因为 `sessions` 里的 token 本身就是累计值。这正是会话级源需要的语义。

## 3. `usage_record` 列映射（全列覆盖）

| 归档列 | Hermes 来源 | 说明 |
| --- | --- | --- |
| `host_id` | 无对应 / 派生 | 采集端 machine-id 哈希，与源无关 |
| `source` | 无对应 / 派生 | 常量 `"hermes"` |
| `granularity` | 派生 | 恒为 `session` |
| `message_id` | `sessions.id` | 与 `session_id` 同值 |
| `session_id` | `sessions.id` | 空或全空白即跳过并计数 |
| `time_created_utc` | `sessions.started_at` | 秒级浮点 → UTC epoch ms |
| `time_completed_utc` | `sessions.ended_at` | 活动会话为 NULL，写 `NULL` |
| `source_time_updated` | `max(messages.timestamp)`，无消息行时回退 `started_at` | 游标，见上 |
| `origin` | 派生 | 正常扫描 `live` |
| `origin_priority` | 派生 | `Origin::priority` 的固定映射 |
| `agent_raw` | `sessions.source` | 空串归一成 `"unknown"` |
| `agent_key` | 派生 | `normalize_agent_key(agent_raw)` |
| `provider_id` | `sessions.model` 的云命名空间；否则由 base URL / provider / Ollama tag 判定 | 见下节 |
| `model_id` | `sessions.model` 去掉 provider 命名空间后的部分 | 缺失写 `"unknown"` |
| `variant` | 无对应 | 写 `NULL` |
| `tok_input` | `sessions.input_tokens` | 负数即整行跳过 |
| `tok_output` | `sessions.output_tokens` | |
| `tok_reasoning` | `sessions.reasoning_tokens` | |
| `tok_cache_read` | `sessions.cache_read_tokens` | |
| `tok_cache_write` | `sessions.cache_write_tokens` | 实测常为 0 |
| `cost` | 无可用对应 | 写 `NULL`，Hermes 自己没算 |
| `cost_source` | 派生 | 恒为 `unavailable`，金额由查询期定价解析 |
| `is_incomplete` | 派生 | 恒为 `false`，见上 |
| `project_dir` | 无对应 | 写空串 |

### provider 与 model 归一化

`billing_provider=custom` 同时覆盖云网关与本地 Ollama，不能直接拿来定价。判定顺序：

1. `sessions.model` 带云命名空间（`global.<provider>.…` 或 `<provider>.…`）→ 拆成
   `provider_id` + `model_id`。
2. 否则由 `billing_base_url`、`billing_provider`、`custom:<ollama-space>:` 前缀
   或无云命名空间的 Ollama tag 识别为本地模型 → `provider_id` 固定写 `ollama`。

**本地 Ollama 模型不定价，这是有意的。** `pricing_catalog.json` 里没有 `ollama` 这个
provider，而价格匹配的 exact、normalized、family 三层都要求 provider 相等，
所以本地模型不会误命中云端价格。实测对账里 6 条 ollama 记录全部未命中定价、
3 条 anthropic 记录正常命中 —— 这是正确结果，不是漏配。

## 4. 实测对账

一个真实 `state.db` 的全量采集：

```
collector: in=644007 out=28361 cr=1365929 cw=0 rz=0   合计=2038297
独立 SQL : 完全一致，9 条记录、granularity 全为 session、skipped=0
provider/model: 3 条 anthropic 云模型 + 6 条 ollama 本地模型
```

零缺失零多余。该对账跑在本机私人数据上，**数据不入库**，所以仓库内的 6 个 `#[test]`
仍基于合成数据 —— 与另外三个适配器同样如此。

## 5. 逆向工程风险

| 项 | 评估 |
| --- | --- |
| 稳定性档位 | **中**。`sessions` 的五个 token 列语义清晰、命名直白，但这是应用私有的 SQLite schema，没有任何兼容承诺 |
| 最先崩的地方 | `messages.token_count` 若某天开始真的写入 —— 那时 Hermes 就该变成消息级源，本适配器需要重写而不是修补 |
| 次先崩的地方 | 时间戳表示（秒级浮点文本）改成整数毫秒或 ISO 串；`billing_provider` 取值集合扩容 |
| 防御姿态 | 只读打开，绝不写源库；坏行按四类原因分别计数并继续；token 负数、时间戳不可换算、session id 缺失都只影响该行 |
| 并发写入 | `state.db-wal` 常在，说明 Hermes 可能正在写。只读连接接受读到某一时刻的一致快照，下一轮重叠窗口会补上 |

## 6. 明确不做

- 不从 `messages` 表推算 token。那一列没有数据，任何推算都是编造。
- 不给本地 Ollama 模型估价。没有价目就是没有价目。
- 不为 `granularity` 写 v4 迁移。项目未投产，开发库删除重建。
- 不读取、不复制任何真实会话内容；本文档只引用字段名、结构与聚合统计。
