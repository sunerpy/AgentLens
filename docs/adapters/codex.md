# Codex 适配器契约（预留，本期不实现解析）

本文档把 Codex CLI 的 rollout 日志字段映射到 AgentLens 归一化模型
`agentlens_core::archive::NormalizedUsageRecord`（源文件 `crates/agentlens-core/src/archive.rs`，
与归档表 `usage_record` 的列一一对应）。

相关文档：

- 姊妹文档：[`docs/adapters/claude-code.md`](./claude-code.md)
- 远程接入契约：[`docs/remote-source-api.md`](../remote-source-api.md)

本期（OpenCode 阶段）**不落地任何 Codex 解析代码**，也不定义 `UsageSource` trait。
这份文档的目的是让后续实现变成机械翻译，而不是重新做一次调研。

## 1. 数据面

| 项 | 值 |
| --- | --- |
| 根目录 | `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl` |
| 压缩 | 约 7 天后同名文件变为 `rollout-<ts>-<uuid>.jsonl.zst`（zstd 帧） |
| 归档树 | `~/.codex/archived_sessions/`，相对路径与 `sessions/` 同构 |
| 其他文件 | `session_index.jsonl`、`history.jsonl`（不含 token 计数，不用于用量） |
| 行结构 | `{timestamp, ordinal?, type, payload}` |
| 成本字段 | **不存在**，Codex 全程不写任何金额 |

同一相对路径在 `sessions/` 与 `archived_sessions/` 同时存在时，**必须优先 `sessions/` 副本、
丢弃 `archived_sessions/` 副本**，否则整个会话会被重复计数。归档树只用于补齐 `sessions/`
中已消失的相对路径。

### 用量所在的行

用量只出现在：

`type == "event_msg"` → `payload.type == "token_count"` → `payload.info`：

- `info.total_token_usage`：会话开始至今的**累计** `TokenUsage`
- `info.last_token_usage`：最近一轮的**增量** `TokenUsage`
- `info.model_context_window`：上下文窗口大小（用量无关，仅可用于 UI 提示）

`TokenUsage` 字段：

- `input_tokens` —— cache-miss 输入
- `cached_input_tokens` —— 缓存读取输入
- `cache_write_input_tokens` —— 较新版本才有，可选
- `output_tokens`
- `reasoning_output_tokens`
- `total_tokens`

> **陷阱（必读）**：`reasoning_output_tokens` 是 `output_tokens` 的**子集**，绝不能加到
> `output_tokens` 之上，否则输出 token 被双计。归一化时 `tok_output` 直接取 `output_tokens`，
> `tok_reasoning` 单列存 `reasoning_output_tokens`，聚合层只展示不相加。

### 模型与 provider

- 模型来自最近一次 `turn_context` 事件的 `turn_context.model`（滚动状态），
  **不在 `token_count` 事件里**。解析器必须顺序扫描文件并维护"当前模型"。
- `session_meta` 携带 `model_provider`，但**不带 model**。
- `thread_settings_applied` 的 `service_tier` 影响计价档位，需保留以便将来估算成本。

## 2. 增量算法要点

1. 顺序读一个 rollout 文件，维护 `prev_total`（上一次见到的 `total_token_usage`）。
2. 遇到 `token_count` 事件时：
   - 若 `total_token_usage` 相对 `prev_total` **确实前进了**，采用 `last_token_usage` 作为本轮增量；
   - 否则（totals 未前进、或 `last_token_usage` 缺失/为零）改用 `total_token_usage - prev_total` 的
     逐字段差值；差值为负则按 0 处理并计入 `skipped_count`。
3. 更新 `prev_total`。这一步"以 totals 为门控、以 last 为取值"的写法是 ccusage 的参照实现，
   目的是抵御重复投递与乱序的 `last_token_usage`。
4. `.jsonl.zst` 需先流式解压再按行解析；解压失败的文件整体跳过并计入 `skipped_count`，
   不得部分入库。
5. 增量游标：文件级 mtime 不可靠（压缩会改写 mtime），以 `timestamp` 为 `source_time_updated`
   并按 `(host_id, source)` 维护游标（见 `source_cursor` 表）。

### 去重键

Codex 没有稳定的消息 ID。归一化必须**合成** `message_id`：

```
message_id = "<rollout 文件相对路径>#<ordinal 或行号>"
```

相对路径以 `sessions/` 为根（归档副本折算成同一相对路径），确保 `sessions/` 与
`archived_sessions/` 的同一逻辑事件生成同一个键，从而由
`UNIQUE(host_id, source, message_id)` 天然去重。

## 3. `usage_record` 列映射（全列覆盖）

| 归档列 | Codex 来源 | 说明 |
| --- | --- | --- |
| `host_id` | 无对应 / 派生 | 采集端 machine-id 哈希，与源无关 |
| `source` | 无对应 / 派生 | 常量 `"codex"` |
| `message_id` | 派生 | `<rollout 相对路径>#<ordinal 或行号>` |
| `session_id` | 派生 | rollout 文件名中的 `<uuid>`；`session_meta` 存在时优先取其会话 id |
| `time_created_utc` | 行级 `timestamp` | 解析为 UTC epoch ms |
| `time_completed_utc` | 无对应 | Codex 无 per-turn 完成时间戳，写 `NULL` |
| `source_time_updated` | 行级 `timestamp` | 与 `time_created_utc` 同值；Codex 行不可变 |
| `origin` | 派生 | `sessions/` → `live`；仅存在于 `archived_sessions/` → `bak` |
| `origin_priority` | 派生 | `Origin::priority` 的固定映射，不从源读取 |
| `agent_raw` | 无对应 | Codex 无 subagent 概念，写空串 |
| `agent_key` | 派生 | `normalize_agent_key(agent_raw)`，空串输入即空串 |
| `provider_id` | `session_meta.model_provider` | 缺失时写 `"codex"` |
| `model_id` | 最近 `turn_context.model` | **不取自 `token_count` 事件** |
| `variant` | 无对应 | Codex 无 variant 概念，写 `NULL`（推理档位见下行） |
| `tok_input` | `TokenUsage.input_tokens` | cache-miss 输入 |
| `tok_output` | `TokenUsage.output_tokens` | 已含 reasoning，不再加 |
| `tok_reasoning` | `TokenUsage.reasoning_output_tokens` | `tok_output` 的子集，仅展示 |
| `tok_cache_read` | `TokenUsage.cached_input_tokens` | |
| `tok_cache_write` | `TokenUsage.cache_write_input_tokens` | 旧版本字段缺失 → 0 |
| `cost` | 无对应 | Codex 无成本字段，写 `NULL` |
| `cost_source` | 派生 | 恒为 `unavailable`；有本地价格覆盖表时才 `estimated` |
| `is_incomplete` | 派生 | 本轮增量四项 token 全零即 `true`，排除出聚合 |
| `project_dir` | `turn_context.cwd`（若存在） | 缺失写空串 |

> `service_tier`（来自 `thread_settings_applied`）在 v1 归档里**没有对应列**。将来做分档计价时
> 需要新增列或迁移；本期只在文档留痕。

### 总输入是派生量

```
total_input = tok_input + tok_cache_read + tok_cache_write
```

**永远不要用源里的 `total_tokens` 当总输入。** Codex 的 `TokenUsage.total_tokens` 是
"输入 + 输出"的会话累计口径，语义完全不同。这与已在 OpenCode 数据上实测到的同一类 bug
相同：OpenCode 的 `tokens.total` 也等于 `input + output`，直接拿来当输入会低估缓存部分、
又把输出混进输入。

## 4. 逆向工程风险

| 项 | 评估 |
| --- | --- |
| 稳定性档位 | **中**。rollout 行结构与 `TokenUsage` 在 Codex 仓库有开源 Rust 类型定义，字段名可核对，但**没有任何兼容承诺**，属内部日志格式 |
| 最先崩的地方 | 新增可选字段（`cache_write_input_tokens` 就是这么加进来的）；`payload.type` 取值集合扩容；`turn_context` 事件改名或改成嵌套 |
| 次先崩的地方 | 目录布局（压缩阈值、`archived_sessions/` 的存在与命名）与 `.zst` 压缩策略 |
| 防御姿态 | 未知 `payload.type` 一律忽略而不报错；缺失可选 token 字段按 0；解析失败的**单文件**跳过并计入 `skipped_count`，绝不因一个坏文件中断整次扫描 |
| 遥测替代路径 | Codex 的 OTEL 支持是**实验性且无文档**的，本期不作为备选。这与 Claude Code 形成不对称：Claude Code 有官方文档化的 OTEL metrics 通道可作更稳的长期方案（见姊妹文档），Codex 没有 |

## 5. 明确不做

- 不实现解析器、不实现 zstd 解压路径、不定义 `UsageSource` trait。
- 不读取、不复制任何真实会话内容；本文档只引用字段名与结构。
