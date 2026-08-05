# Claude Code 适配器契约（预留，本期不实现解析）

本文档把 Claude Code 的 transcript 日志字段映射到 AgentLens 归一化模型
`agentlens_core::archive::NormalizedUsageRecord`（源文件 `crates/agentlens-core/src/archive.rs`，
与归档表 `usage_record` 的列一一对应）。

相关文档：

- 姊妹文档：[`docs/adapters/codex.md`](./codex.md)
- 远程接入契约：[`docs/remote-source-api.md`](../remote-source-api.md)

本期不落地任何 Claude Code 解析代码，也不定义 `UsageSource` trait。

## 1. 数据面

| 项 | 值 |
| --- | --- |
| 根目录 | `~/.claude/projects/**/*.jsonl` |
| 其他目录 | `~/.claude/transcripts/`、`~/.claude/stats-cache.json`（不作权威源） |
| 行结构 | 每行一个事件对象，`type` 区分种类 |
| 携带用量的行 | **只有** `type == "assistant"` 的行有 `message.usage` |
| 保留期 | 按 `cleanupPeriodDays` 自动删除，**默认 30 天** |

> 30 天自动清理是 AgentLens 必须自建归档库的直接原因：源数据会消失，历史只能靠我们自己留。
> 这不是优化项，是数据保全的硬需求。

### 字段所在层级（容易踩错）

嵌套在 `message` 下：

- `message.id`
- `message.model`
- `message.usage.*`

顶层（不在 `message` 下）：

- `timestamp`
- `sessionId`
- `version`
- `requestId`
- `isSidechain`
- `costUSD`

### usage 结构

基础四项：

- `usage.input_tokens`
- `usage.output_tokens`
- `usage.cache_creation_input_tokens`
- `usage.cache_read_input_tokens`

较新版本额外提供细分对象：

- `usage.cache_creation.ephemeral_5m_input_tokens`
- `usage.cache_creation.ephemeral_1h_input_tokens`

**当 `usage.cache_creation` 对象存在时必须优先它**：
`tok_cache_write = ephemeral_5m_input_tokens + ephemeral_1h_input_tokens`，
仅在该对象缺失时回退到扁平的 `cache_creation_input_tokens`。两者同时相加会双计。

## 2. 增量算法要点

1. 逐文件按行读，只处理 `type == "assistant"` 且 `message.usage` 存在的行。
2. 去重键 `(message.id, requestId)`，**跨文件全局生效**——同一条 assistant 消息会在多个
   transcript 文件里重复出现（续写、分支、sidechain）。
3. 冲突时的替换优先级（先命中者胜出）：
   1. `isSidechain == false` 优于 `isSidechain == true`；
   2. token 总量更大的一条胜出；
   3. 携带 `speed` 字段的一条胜出。
   全部相同则保留已入库的那条，保证结果与扫描顺序无关（幂等）。
4. 因 30 天清理，扫描只能看到窗口内的文件；游标按 `(host_id, source)` 存
   `source_time_updated`，并把"源里已消失但归档里存在"的时间段记入 `coverage_interval`，
   在 UI 上显示为"无覆盖"而不是 0。
5. `costUSD` 在当前版本里普遍缺失或不可信，**一律不入 `cost`**。

## 3. `usage_record` 列映射（全列覆盖）

| 归档列 | Claude Code 来源 | 说明 |
| --- | --- | --- |
| `host_id` | 无对应 / 派生 | 采集端 machine-id 哈希，与源无关 |
| `source` | 无对应 / 派生 | 常量 `"claude-code"` |
| `message_id` | 派生 | `"<message.id>#<requestId>"`，即去重键的串联 |
| `session_id` | 顶层 `sessionId` | |
| `time_created_utc` | 顶层 `timestamp` | ISO-8601 解析为 UTC epoch ms |
| `time_completed_utc` | 无对应 | transcript 无完成时间戳，写 `NULL` |
| `source_time_updated` | 顶层 `timestamp` | 同 `time_created_utc`；行不可变 |
| `origin` | 派生 | `~/.claude/projects/` 下均为 `live` |
| `origin_priority` | 派生 | `Origin::priority` 的固定映射 |
| `agent_raw` | 派生自 `isSidechain` | `isSidechain == true` → `"sidechain"`，否则空串 |
| `agent_key` | 派生 | `normalize_agent_key(agent_raw)` |
| `provider_id` | 无对应 / 派生 | 常量 `"anthropic"`；transcript 不写 provider |
| `model_id` | `message.model` | |
| `variant` | 无对应 | 无 variant 概念，写 `NULL` |
| `tok_input` | `usage.input_tokens` | cache-miss 输入 |
| `tok_output` | `usage.output_tokens` | |
| `tok_reasoning` | 无对应 | transcript 不区分 reasoning，写 0 |
| `tok_cache_read` | `usage.cache_read_input_tokens` | |
| `tok_cache_write` | `usage.cache_creation.{ephemeral_5m,ephemeral_1h}_input_tokens` 之和，缺失时回退 `usage.cache_creation_input_tokens` | 两者不可相加 |
| `cost` | 无对应（`costUSD` 不可信） | 写 `NULL` |
| `cost_source` | 派生 | 恒为 `unavailable`；有本地价格覆盖表时才 `estimated` |
| `is_incomplete` | 派生 | 四项 token 全零即 `true`，排除出聚合 |
| `project_dir` | 派生自 transcript 文件所在的 `projects/<encoded>/` 目录名 | 目录名是编码后的路径，需反解；无法反解时写空串 |

> `version`（顶层）在 v1 归档里**没有对应列**，仅可用于解析期的兼容分支判断。

### 总输入是派生量

```
total_input = tok_input + tok_cache_read + tok_cache_write
```

Claude Code **根本没有** total 字段可用，缓存部分必须自己相加。这里要同时警惕相反方向的
错误：不要以为 `input_tokens` 就是总输入——它只是 cache-miss 部分。这与 OpenCode 上实测到的
同一类 bug 对称：那里存在 `tokens.total` 但它等于 `input + output`，照抄会既漏缓存又混入输出。
无论源里有没有 total，总输入都只能由三项缓存/非缓存输入相加得出。

## 4. 逆向工程风险

| 项 | 评估 |
| --- | --- |
| 稳定性档位 | **低**。transcript 格式完全未文档化，没有公开类型定义，纯逆向工程 |
| 最先崩的地方 | `usage` 的形状演进（`cache_creation` 细分对象就是后加的，很可能继续细分）；`type` 取值扩容 |
| 次先崩的地方 | `projects/<encoded>` 目录编码规则；`requestId` 的存在性（缺失会打破去重键） |
| 数据保全风险 | 默认 30 天清理。归档一旦漏跑，那段历史**永久丢失**，无法回补 |
| 防御姿态 | 未知 `type` 忽略；缺失 `requestId` 时退化为按 `message.id` 去重并计入 `skipped_count`；单行 JSON 解析失败只跳该行 |
| 遥测替代路径 | Claude Code 有**官方文档化的 OTEL metrics 通道**，长期比逆向 transcript 稳。代价是只有聚合指标、拿不到消息级 `agent`/session 明细，且需要用户显式开启。将来若 transcript 格式频繁破坏，OTEL 是可行的降级方案 —— 这是相对 Codex 的明显优势 |

## 5. 明确不做

- 不实现解析器、不接 OTEL、不定义 `UsageSource` trait。
- 不读取、不复制任何真实会话内容；本文档只引用字段名与结构。
