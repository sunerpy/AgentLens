# AgentLens Remote Source API v1

一个远程服务实现本文档即可被 AgentLens 桌面 app 以 `RemoteService` 形态接入。
本文档面向**从未见过本仓库的第三方实现者**：读完它就能写出一个可被接入的服务，无需读 Rust 代码。

相关文档：

- 归一化模型：`agentlens_core::archive::NormalizedUsageRecord`（`crates/agentlens-core/src/archive.rs`）
- 源字段映射：[`docs/adapters/codex.md`](./adapters/codex.md)、[`docs/adapters/claude-code.md`](./adapters/claude-code.md)

> **本期实现状态**：只有本契约文档。`HostSource` 的 `RemoteService` 形态**不在本期实现**
> （app 侧本期只有 local 与 ssh 两个实现）。服务端与客户端代码都不在本阶段交付。

## 1. 设计前提：源无关（source-agnostic）

- 归一化 record 的 `source` 是**开放字符串**：`opencode`、`codex`、`claude-code`、`gemini`、
  以及任何未来名字。
- **app 侧对未知 `source` 照常入库并展示，没有白名单、不做校验拒绝。** 这是用户明确要求，
  不是实现疏漏。展示时直接用该字符串作为工具名。
- **一个远程服务可以同时上报多个源。** meta 用 `sources[]` 分列，每源一条元数据。
- 实现者可以是：独立 daemon（复用 `agentlens-core`）、OpenCode TS 插件（监听
  `message.updated` 自行归一化）、或任何读取 codex/claude/gemini 本地日志的第三方程序。
  满足本契约即可。

## 2. 线缆格式：NDJSON

`GET /v1/records` 的响应体是 NDJSON（`application/x-ndjson`）：

- **第 1 行**：meta 对象，恰好一行；
- **第 2 行起**：每行一个归一化 record 对象；
- 行分隔符 `\n`；不允许空行、不允许行内换行（JSON 必须紧凑单行）；
- 零条 record 是合法响应（只有 meta 行）。

这组 wire DTO 与 AgentLens 本机 collector 的 serde 结构**是同一组类型**，字段名逐字一致。

### 2.1 meta 行（字段名为 snake_case）

<!-- wire-dto:meta = protocol_version, machine_id_hash, hostname, collector_version, sources -->
<!-- wire-dto:source_meta = source, data_dir, scan_window, eligible_count, skipped_count -->
<!-- wire-dto:scan_window = since, cutoff -->

| 字段 | 类型 | 必填 | 含义 |
| --- | --- | --- | --- |
| `protocol_version` | u32 | 是 | 本契约版本，v1 恒为 `1` |
| `machine_id_hash` | string | 是 | 远端 machine-id 的哈希；app 的 `host_id` 由此派生。同一主机必须稳定 |
| `hostname` | string | 是 | 显示用主机名，仅供 UI |
| `collector_version` | string | 是 | 实现者自己的版本串（如 `"my-daemon/0.3.1"`） |
| `sources` | array | 是 | 每源一个对象；本次响应只涉及一个源时也必须是单元素数组 |

`sources[]` 元素：

| 字段 | 类型 | 必填 | 含义 |
| --- | --- | --- | --- |
| `source` | string | 是 | 开放源名 |
| `data_dir` | string | 是 | 该源在远端的数据目录（诊断用，可为空串） |
| `scan_window` | object | 是 | 本次扫描的窗口，见下 |
| `eligible_count` | u64 | 是 | 窗口内命中并输出的 record 条数 |
| `skipped_count` | u64 | 是 | 因解析失败/缺字段/不完整被跳过的条数 |

`scan_window`：

| 字段 | 类型 | 必填 | 含义 |
| --- | --- | --- | --- |
| `since` | i64 | 是 | 客户端请求的游标（UTC epoch ms），首次同步为 `0` |
| `cutoff` | i64 | 是 | 本次扫描的确定性上界（UTC epoch ms）。app 只有在成功消费全部 record 后才把游标推进到 `cutoff` |

### 2.2 record 行（字段名为 camelCase）

<!-- wire-dto:record = hostId, source, messageId, sessionId, timeCreatedUtc, timeCompletedUtc, sourceTimeUpdated, origin, originPriority, agentRaw, agentKey, providerId, modelId, variant, tokInput, tokOutput, tokReasoning, tokCacheRead, tokCacheWrite, cost, costSource, isIncomplete, projectDir -->

> **注意大小写不对称**：meta 行是 snake_case，record 行是 camelCase。这不是笔误，
> 是归一化 record 类型上 `#[serde(rename_all = "camelCase")]` 的结果（record 类型还要给
> 前端生成 TS 绑定），meta 类型没有该属性。实现者请逐字照抄两张表。

| 字段 | 类型 | 必填 | 含义 |
| --- | --- | --- | --- |
| `hostId` | string | 是 | 通常等于 `machine_id_hash` 的派生值；app 会以自己的口径覆盖 |
| `source` | string | 是 | **每条 record 强制携带**，必须等于查询参数里的 `source` |
| `messageId` | string | 是 | 源内唯一；`(hostId, source, messageId)` 是去重键 |
| `sessionId` | string | 是 | 源会话标识 |
| `timeCreatedUtc` | i64 | 是 | UTC epoch ms |
| `timeCompletedUtc` | i64 \| null | 是（可为 null） | UTC epoch ms |
| `sourceTimeUpdated` | i64 | 是 | 游标依据；重叠窗口冲突以此比较 |
| `origin` | string | 是 | `"live"` \| `"bak"` \| `"legacy"` |
| `originPriority` | i32 | 是 | `origin` 的固定整数优先级 |
| `agentRaw` | string | 是 | 源显示名/slug，可为空串 |
| `agentKey` | string | 是 | 规范化 agent 键，可为空串 |
| `providerId` | string | 是 | |
| `modelId` | string | 是 | |
| `variant` | string \| null | 是（可为 null） | 如 `"xhigh"` |
| `tokInput` | u64 | 是 | cache-miss 输入 |
| `tokOutput` | u64 | 是 | 输出（已含 reasoning） |
| `tokReasoning` | u64 | 是 | `tokOutput` 的子集，勿相加 |
| `tokCacheRead` | u64 | 是 | |
| `tokCacheWrite` | u64 | 是 | |
| `cost` | f64 \| null | 是（可为 null） | 不可信时必须为 `null`，**禁止写 0 冒充** |
| `costSource` | string | 是 | `"actual"` \| `"unavailable"` \| `"estimated"` |
| `isIncomplete` | bool | 是 | token 全零且无完成时间的记录标记为 `true` |
| `projectDir` | string | 是 | 可为空串 |

**总输入是派生量**：`tokInput + tokCacheRead + tokCacheWrite`。不要在 wire 上放 total 字段，
app 也不会读。

## 3. 端点

### 3.1 `GET /v1/meta`

返回单个 meta 对象（**JSON，不是 NDJSON**）。用于配对后的连通性与能力探测：app 由
`sources[]` 得知这台服务能提供哪些源，然后按源逐个拉取。

此处 `sources[].scan_window` 填当前已知窗口（无扫描发生时 `since` 与 `cutoff` 可同为服务端
当前时间），`eligible_count`/`skipped_count` 可为 `0`。

| 状态码 | 含义 |
| --- | --- |
| 200 | 成功，`application/json` |
| 401 | 缺少或无效 bearer token |
| 500 | 服务端内部错误 |

### 3.2 `GET /v1/records`

| 参数 | 必填 | 类型 | 说明 |
| --- | --- | --- | --- |
| `source` | **是** | string | 只拉这一个源。缺失或空串 → `400` |
| `since` | 否 | i64 | 游标（UTC epoch ms），缺省 `0`（全量）。只返回 `sourceTimeUpdated > since` 的 record |

`source` 之所以必填：app 的游标表 `source_cursor` 主键是 `(host_id, source)`，
每个源有**独立**游标。若允许省略 `source` 用单个 `since` 一次拉全部源，不同源的游标会互相
污染——推进最快的源会把慢源的未读区间"吃掉"，造成永久漏账。必填 `source` 是从协议层面
杜绝这个 bug，而不是靠客户端自律。

| 状态码 | 含义 |
| --- | --- |
| 200 | 成功，`application/x-ndjson`（至少一行 meta） |
| 400 | `source` 缺失/空，或 `since` 非整数 |
| 401 | 缺少或无效 bearer token |
| 404 | `source` 语法合法但该服务不提供此源 |
| 500 | 服务端内部错误 |

响应中 `sources[]` **必须只有一个元素**，且其 `source` 等于查询参数。

分页：v1 不分页。服务端用 `scan_window.cutoff` 控制单次响应规模——想切小就把 `cutoff` 提前，
app 下次带着新游标再来。

## 4. 配对与传输安全

### 4.1 配对握手（有序步骤）

1. **服务端启动时**生成一次性配对码，打印到服务端终端 / 日志。
   码的有效期 **10 分钟**，**单次使用**，用掉即失效，过期即失效。
2. 用户在 app "添加远程服务"里填入服务地址与该配对码。
3. app 调 `POST /v1/pair`：

   ```json
   {"pairing_code": "ABCD-1234-EFGH"}
   ```

4. 服务端校验码（未过期、未用过）后返回长期 bearer token：

   ```json
   {"token": "opaque-long-lived-token", "machine_id_hash": "8f14e45fceea167a", "hostname": "linux-box-01"}
   ```

   校验失败返回 `401`，响应体不得泄露正确码的任何部分。
5. app 把 token 存入**操作系统钥匙串**（Windows Credential Manager / Linux libsecret），
   **绝不明文落盘**、不写配置文件、不写日志。
6. 后续**所有**请求（含 `/v1/meta`）携带 `Authorization: Bearer <token>`。
7. **重新配对即吊销旧 token**：服务端一旦签发新 token，此前签发的全部 token 立即失效。
   用户换机、疑似泄露时的补救手段就是重启服务端拿新码重配。

配对码本身**不能**当访问凭证使用，只能换 token。

### 4.2 传输安全（硬性要求）

- **明文 HTTP 仅允许 loopback**（`127.0.0.1` / `::1`）。
- **任何非 loopback 地址必须走 HTTPS，或经 SSH 隧道把远端端口转发到本地 loopback。**
- app 对非 loopback 的明文 `http://` 地址**拒绝配对**，不提供"我知道风险"开关。
- 配对码与 token 都是明文可读的凭证，明文 HTTP 下会被网络中间人直接抓走，所以这条没有例外。

推荐的最省事部署：服务端只监听 `127.0.0.1`，用户用
`ssh -L 7788:127.0.0.1:7788 user@remote-box` 打隧道，app 连 `http://127.0.0.1:7788`。

## 5. 完整请求 / 响应示例

请求：

```http
GET /v1/records?source=claude-code&since=1785400000000 HTTP/1.1
Host: 127.0.0.1:7788
Authorization: Bearer opaque-long-lived-token
Accept: application/x-ndjson
```

响应（`200`，`application/x-ndjson`，为便于阅读此处对每行做了缩进展示，**真实响应每行必须是紧凑单行 JSON**）：

第 1 行（meta）：

```json
{"protocol_version":1,"machine_id_hash":"8f14e45fceea167a","hostname":"linux-box-01","collector_version":"my-daemon/0.3.1","sources":[{"source":"claude-code","data_dir":"/home/dev/.claude/projects","scan_window":{"since":1785400000000,"cutoff":1785468900000},"eligible_count":1,"skipped_count":0}]}
```

第 2 行（record）：

```json
{"hostId":"8f14e45fceea167a","source":"claude-code","messageId":"msg_01ABC#req_01XYZ","sessionId":"ses_demo_0001","timeCreatedUtc":1785468844419,"timeCompletedUtc":null,"sourceTimeUpdated":1785468844419,"origin":"live","originPriority":3,"agentRaw":"","agentKey":"","providerId":"anthropic","modelId":"claude-sonnet-4-5","variant":null,"tokInput":1200,"tokOutput":340,"tokReasoning":0,"tokCacheRead":8000,"tokCacheWrite":512,"cost":null,"costSource":"unavailable","isIncomplete":false,"projectDir":"/home/dev/work/demo"}
```

app 消费完后把 `(host, "claude-code")` 的游标推进到 `1785468900000`（即 `scan_window.cutoff`），
下次请求 `?source=claude-code&since=1785468900000`。

`GET /v1/meta` 的响应示例（单个 JSON 对象，非 NDJSON）：

```json
{"protocol_version":1,"machine_id_hash":"8f14e45fceea167a","hostname":"linux-box-01","collector_version":"my-daemon/0.3.1","sources":[{"source":"claude-code","data_dir":"/home/dev/.claude/projects","scan_window":{"since":0,"cutoff":1785468900000},"eligible_count":0,"skipped_count":0},{"source":"gemini","data_dir":"/home/dev/.gemini/logs","scan_window":{"since":0,"cutoff":1785468900000},"eligible_count":0,"skipped_count":0}]}
```

注意第二个源 `gemini`：app 此期没有任何 Gemini 解析代码，但依然会把它列出来、拉取、入库、
展示。这就是"未知 source 不白名单"的实际表现。

## 6. 实现者检查表

- [ ] meta 字段名 snake_case，record 字段名 camelCase，逐字一致
- [ ] 每条 record 都带 `source`，且等于查询参数
- [ ] NDJSON 每行紧凑单行，第一行是 meta
- [ ] `/v1/records` 缺 `source` 返回 `400`
- [ ] 不可信成本写 `null`，不写 `0`
- [ ] `cutoff` 单调不倒退
- [ ] 配对码 10 分钟单次使用；重新配对吊销旧 token
- [ ] 所有端点校验 `Authorization: Bearer`
- [ ] 非 loopback 一律 HTTPS 或 SSH 隧道
