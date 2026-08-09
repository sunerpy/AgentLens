# 四个源的计量口径（横向对照）

本文档回答一个问题：**AgentLens 支持的四个源，各自是怎么统计用量的？**

`docs/adapters/` 下的三份契约是**纵向**的 —— 每份把一个源的字段逐个映射到归一化模型。
本文档是**横向**的 —— 把四个源摆在一起对照，讲清它们在「一条记录代表什么」这件事上的分歧，
以及归一化模型为什么要这样设计。OpenCode 没有独立契约文档，它的计量口径以本文档为准。

相关文档：

- 适配器契约：[Claude Code](./adapters/claude-code.md)、[Codex](./adapters/codex.md)、
  [Hermes](./adapters/hermes.md)
- 归一化模型与归档表：[架构](./architecture.md)
- 远程接入契约：[Remote Source API v1](./remote-source-api.md)

> [!IMPORTANT]
> **本文档里的实测数字不能在本仓库复现。** 仓库内**没有任何真实 fixture**：四个源的自动化
> 测试全部基于合成数据（`claude_code` 14 个、`codex` 5 个、`hermes` 6 个 `#[test]`）。
> 下文标注「实测」的数字来自开发机上的私人数据目录，这些数据**不入库**，本文档只保留聚合
> 结论，不保留会话 ID、路径与项目名。

## 0. 一张总览表

| | OpenCode | Claude Code | Codex | Hermes |
| --- | --- | --- | --- | --- |
| 数据源形态 | SQLite 库 | `.jsonl` transcript | `rollout-*.jsonl` | SQLite 库 |
| 一条记录代表 | 一条消息 | 一条带 usage 的 assistant 行 | 一个 `token_count` 事件 | **一整个会话** |
| `granularity` | `message` | `message` | `message` | `session` |
| `message_id` 来源 | 源库自带 | 合成 `<id>#<request_id>` | 合成 `<相对路径>#<序号>` | `session_id` |
| 计量值语义 | 增量 | 增量 | **会话累计，须差分** | 会话汇总，upsert 覆盖 |
| 成本来源 | `Actual`（源库带金额） | `Unavailable` → 查询期估算 | `Unavailable` → 查询期估算 | `Unavailable` → 查询期估算 |
| 实测入库记录数 | 251737 | 17 | 20252 | 9 |
| 实测跳过数 | 17021 | 5205 | 132702 | 0 |

四个源都用同一个重叠窗口：`OVERLAP_WINDOW_MS = 24 * 60 * 60 * 1_000`，即游标回退 24 小时
再采集，避免边界时间戳的记录在增量采集中被整段跳过。重复采集靠去重键幂等，重叠不会重复计数。

## 1. OpenCode — 消息级，唯一有真实成本

数据源是 OpenCode 自己的 SQLite 库（本地数据目录下）。一条记录就是一条消息，
`message_id` **直接取源库主键**（`row.get(0)`），不需要合成 —— 四个源里只有它是这样。

实测一次全量采集：

- 入库 251737 条，跳过 17021 条，`granularity` 全为 `message`
- provider 分布：`kiro-auth` 188626 / `myopenai` 62526 / `openai` 281 / `amazon-bedrock` 121
- agentKey 分布（前三）：`sisyphus-junior` 154951 / `atlas-plan-executor` 49208 / `oracle` 9125
- 五桶合计：输入 51392151086、输出 49347721、推理 5785520、缓存读 7906679693、缓存写 802338347
- `incomplete` 标记：False 251482 / True 255

**它是唯一有真实成本的源。** OpenCode 的库里带每条消息的实际金额，因此归一化后
`cost_source = CostSource::Actual`。其余三个源都拿不到金额，只能在查询期按模型定价目录估算。

## 2. Claude Code — 消息级，合成去重键

数据源是 Claude Code 的项目目录下的 `.jsonl` transcript，一行一个事件。只有带
`message.usage` 的 assistant 行才是计量事件，其余全部跳过。

`message_id` 需要**合成**：`<message.id>#<requestId>`（`claude_code.rs:391`
`archive_message_id`），去重**跨文件全局生效** —— 同一条 assistant 消息会在多个 transcript
文件里重复出现（续写、分支、sidechain，见契约 `claude-code.md:77`）。`message.id` 本身
per API response 唯一（`claude_code.rs:380`）；`requestId` 对第三方网关与部分 sub-agent
路径缺失（`claude_code.rs:382`），缺失时退化为按 `message.id` 独自去重，并计入 skipped 统计。
实测上 `requestId` 并未起作用：21 → 17 的合并全部由 `message.id` 完成，4 组重复均无
`requestId`，也没有出现「同 id 不同 requestId」—— 它是为契约完备性保留的。

重复行保留哪一条由替换优先级决定：`isSidechain == false` 优于 `true` → token 总量更大者胜
→ 携带 `speed` 字段者胜，全同则保留已入库那条，因此结果与扫描顺序无关
（详见契约 `claude-code.md:78-83`）。

实测一次全量采集：

- 645 个 `.jsonl` 共 5222 行 → eligible 17、skipped 5205
- 5222 行里只有 21 行带 `message.usage`；21 → 17 是同一 `messageId` 的重复行被去重键吸收
- 五桶：输入 34925、输出 7123、缓存读 72196、缓存写 290010，合计 404254
  （与独立脚本逐桶提取一致）

一个需要注意的取值：`<synthetic>` 是 Claude Code 给本地合成消息用的占位模型名，
它不是一个真实模型，**不该被定价**。

## 3. Codex — 事件级，累计值必须差分

数据源是 `rollout-*.jsonl`（活跃与归档两个目录）。一条记录不是一条消息，而是一个
`token_count` 事件。Codex 没有稳定的消息 ID，所以 `message_id` 合成为
`<rollout 相对路径>#<ordinal 或行号>`。

实测一次全量采集：

- 220 个 rollout → eligible 20252、skipped 132702
  （其中 `non_usage_event` 132697、`missing_total_usage` 5）
- 五桶：输入 2494715106、输出 10995018、推理 5511044、缓存读 2151336078、缓存写 49821053
- 定价命中 19952 / 20252

**这是四个源里最容易算错的一个**，四个坑依次说明。

### 3.1 `total_token_usage` 是会话累计值

`token_count` 事件里有两个字段：`total_token_usage` 是**会话累计**、单调不减；
`last_token_usage` 是**本轮增量**。逐行把 `total` 相加会得到一个荒谬的数字 ——
实测单个文件：末值 79,877,325，逐行相加 21,159,774,382，**虚高 264.9 倍**。

采集算法是「**以 totals 为门控、以 last 为取值**」：totals 相对上一事件前进了，就取
`last_token_usage`；否则退回用 totals 的逐字段差值。这样既避免了 last 缺失时丢数据，
也避免了 totals 重置或倒退时算出负增量。

### 3.2 推理 token 是输出 token 的子集

`reasoning_output_tokens` 包含在 `output_tokens` 里，**绝不能相加**。
实测 20252 / 20252 条事件全部满足这个包含关系。归一化模型把它单独存一桶，
是为了让前端能拆开看，不是为了让它参与合计。

### 3.3 缓存写字段经常缺失

`cache_write_input_tokens` 只在 22% 的条目里存在（4650 / 20252）。**缺失是常态**，
不是解析失败，也不该当成 0 之外的异常处理。

### 3.4 provider 有意偏离原契约

`provider_id` 取 `turn_context.model` 的 namespace，例如 `openai.gpt-5.4` 拆成
provider `openai` + model `gpt-5.4`。这**有意偏离**了原契约里的
`session_meta.model_provider`。

理由是实测数据：17317 / 20252（85%）的事件其 `model_provider` 是转发通道
（Bedrock 之类）而非模型归属方。`model_provider` 记录的是「从哪条通道接入」，
照它取值会让 85% 的记录永久估不出成本，因为定价目录是按模型归属方组织的。

代价要说清楚，有两条：**接入通道信息在归档层不可见**（20252 条 Codex 记录的 provider
一律是 `openai`，看不出其中 17317 条走的是 Bedrock）；而且 **Bedrock 单价与直连并不相同**
（实测 5.5/27.5 与 5.0/25.0 美元每 Mtok），所以这部分成本估算带系统性偏差。
用 85% 覆盖率换一个已知方向的偏差，比让 85% 永久为空更有用，但它是偏差而不是精确值。

## 4. Hermes — 会话级，与其他三个本质不同

数据源是 Hermes 的 `state.db`，**只读打开**。`state.db-wal` 常在，说明库处于活跃写入状态，
只读打开是必须的。

一条记录代表**一整个会话**，`message_id` 直接用 `session_id`，靠
`UNIQUE(host_id, source, message_id)` + upsert 让后续采集覆盖累计值。

实测一次全量采集：

- 入库 9 条，跳过 0 条，`granularity` 全为 `session`
- 五桶：输入 644007、输出 28361、缓存读 1365929、缓存写 0、推理 0，合计 2038297
  （与独立 SQL 查询一致）
- provider 分布：`anthropic` 3 条；`ollama` 6 条（本地模型，无定价）

### 4.1 为什么只能是会话级

`messages` 表 158 行里，`token_count` 非 NULL 的有 **0 行** —— assistant 72、user 51、
tool 33、session_meta 2，四种 role 无一例外。计量只写在 `sessions` 表。
**消息级数据无法重建会话级总量**，所以 Hermes 只能按会话归一化。

这也意味着 Hermes 的用量**计入 token 与成本，但不计入「消息数」**，
改为计入 `session_record_count`。详见下一节的粒度设计。

### 4.2 游标为什么用 `max(messages.timestamp)`

不能用 `sessions.started_at`。实测 9 个会话里有 3 个 `ended_at` 为 NULL 却已有真实 token
（20526 / 9472 / 64432）—— 会话还在进行，token 还在长。用 `started_at` 当游标，
游标一旦越过这些会话的开始时间，后续增长就永久丢失。

`max(messages.timestamp)` 是递进的：实测它领先对应会话的 `started_at`
17 秒 / 77 秒 / 9028 秒，能跟上会话的实际推进。

本地 Ollama 模型不在定价目录里（目录没有 `ollama` 这个 provider），因此这部分成本是
`Unavailable`，不是 0。

## 5. 设计理念与优势

### 五个原子桶不折叠

输入 / 输出 / 推理 / 缓存读 / 缓存写各自独立存储，**不读取源库的 `tokens.total`**。
前端展示的「总输入 = 输入 + 缓存读取 + 缓存写入」是显式推导（`i18n/zh.ts:164`）。
好处有两个：源库改口径时不会污染已归档的历史；能分辨「缓存命中到底省了多少」。

### 归档库是权威历史，永不裁剪

源库轮转、备份被删、远端数据目录被清空，都不影响已归档的记录。这是整个项目的基本前提。

### 粒度显式建模

`usage_record.granularity` 取 `'message'` 或 `'session'`。聚合时 `message_count` 与
`session_record_count` 分开计，token 五桶、成本与 `active_session_count` 跨粒度求和。

这一列不是可选的洁癖。Hermes 用 9 条记录代表一批会话，如果混进「消息数」，
就会用 9 去代表 158 条真实消息 —— **少算 17 倍**，量纲直接错了。

### origin 优先级解决同一记录的多来源冲突

同一条记录可能同时出现在实时库、备份库和历史目录里。`origin` 优先级为
`live=3 / bak=2 / legacy=1`（`archive.rs:154`），upsert 时高优先级覆盖低优先级，
同级再比 `source_time_updated`。好处是从备份补录历史，不会把实时采集的更准值覆盖掉。

### 定价匹配允许跨 provider 回退

同一模型经不同网关接入时，价格条目往往只挂在归属方名下，因此 `(provider, model)`
未命中时允许回退到其他 provider 下同一模型的条目（优先 `anthropic` / `google` /
`openai`，其次 `amazon-bedrock`，再次其余），并剥离 8 个运行档位后缀：
`xhigh`、`high`、`medium`、`low`、`minimal`、`max`、`thinking`、`fast`。

`mini` 与 `nano` **有意不剥离** —— 它们是独立模型、独立定价（GPT-5.4 实测分别为
2.5/15、0.75/4.5、0.2/1.25 美元每 Mtok），误剥离会造成 3 倍以上偏差。`preview` /
`latest` 是发布通道或滚动别名，`free` 是计费层级，同样不能假定等价。

实测效果：251737 条 OpenCode 记录的可定价比例从 0.1% 提到 99.4%。仍估不出成本的是
6 个模型共 1617 条 —— `claude-haiku-4-5` 1510、`antigravity-gemini-3.1-pro` 75、
`claude-sonnet-4-5` 20、`gpt-5.6` 9、`big-pickle` 2、`auto` 1 —— 它们落在
`unavailable` 而不是被估成 0，可以在设置页手工补价。

手工覆盖价不参与上述回退：用户填的价格严格按 `(provider, model)` 精确匹配，
不会外溢到别的 provider 的记录上。

### 成本分层不相加

`actual` / `estimated` / `unavailable` 三态分列存储，**永不相加**。只有 OpenCode 有
`actual`，其余三源在查询期按定价目录估算，估不出来的落在 `unavailable`。
好处是用户能一眼分辨哪部分金额是真的。

### 去重键让重复采集天然幂等

`UNIQUE(host_id, source, message_id)`。源没有稳定 ID 时就合成一个：Codex 用「路径#序号」、
Claude Code 用「id#request_id」、Hermes 用 `session_id`。配合 24 小时重叠窗口，
重复采集不会重复计数，中断重跑也安全。

## 6. 限制

- 仓库内没有真实 fixture，所有自动化测试基于合成数据。本文档的实测数字无法在 CI 里复现。
- 三个源拿不到真实金额，估算精度取决于定价目录的覆盖面。本地模型（如 Ollama）不在目录里，
  成本为 `Unavailable` 而非 0。
- Codex 的 `provider_id` 取值偏离其原契约，这是权衡后的取舍，不是文档与实现不一致；
  代价是接入通道不可见，且 Bedrock 与直连单价不同导致的系统性偏差。
- 跨 provider 回退把可定价比例推到 99.4%，但它是「同一模型在不同 provider 下价格相同」
  这一假设的产物。假设不成立时（如 Bedrock 与直连），估算带偏差。
- `hosts.enabled_sources` 默认只有 `'opencode'`，另外三个源需要显式启用。
