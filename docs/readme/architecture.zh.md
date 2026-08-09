# 架构

[← README](../../README.md) · [English](../architecture.md)

## 组成

| 组成 | 路径 | 职责 |
| --- | --- | --- |
| 核心 crate | `crates/agentlens-core` | 归档、解析、聚合、SSH 传输 |
| 远端采集器 | `crates/agentlens-collector` | headless 静态 musl 二进制，随包分发、按需推送到远端 |
| 口令助手 | `crates/agentlens-askpass` | `SSH_ASKPASS` 对端，随包分发 |
| 桌面壳 | `src-tauri/` | Tauri 2 宿主、IPC 命令层、托盘 |
| 前端 | `frontend/` | React 18.3.1、Vite 8、Tailwind v4 |

## 归档库

SQLite。入库时做记录去重并维护按源的水位线，因此重扫未变化的源代价很低，重复导入
有重叠的时间窗也不会双计。归档库被当作权威历史、永不裁剪，见
[data-storage.zh.md](data-storage.zh.md)。

## SSH 传输

远端命令是**恒定**的，请求载荷作为单个位置参数传入。这样远端调用里不存在 shell 插值，
契约也可被检视：每次调用用的是同一条命令字符串，只有参数在变。

采集器二进制每次刷新时传输、在远端校验 sha256、就地执行、退出时清理。
远端侧只做只读扫描。

## IPC 层

Tauri 命令是前端与核心之间唯一的边界。TypeScript 契约由 `ts-rs` 从 Rust 类型生成，
因此前端消费的 Rust 结构体一旦变更，会表现为 TypeScript 类型错误，而不是运行时静默不匹配。
载荷是单个 camelCase 对象，每个 wrapper 的键集合都在单测里被断言。

## 时区与日历分桶

日历分桶在 Rust 侧。前端不引入 `date-fns` / `dayjs` / `moment`：周边界与时区偏移
若有两套实现就是缺陷发生器，所以只保留 Rust 这一套，前端只负责渲染给它的结果。

## 适配器

`OpenCode`、[`Claude Code`](../adapters/claude-code.md)、
[`Codex`](../adapters/codex.md) 与 [`Hermes`](../adapters/hermes.md) 都已实现，本地与远端
两条路径齐备，同一主机可以同时启用多个源。`SUPPORTED_SOURCES` 现为这四元。Claude Code、
Codex 与 Hermes 都已对真实数据实测逐桶对账通过，但仓库内没有真实 fixture，自动化测试仍
全部基于合成数据；`hosts.enabled_sources` 默认值是 `'opencode'`，另外三个源需要显式启用。
Codex 有意不实现 `.jsonl.zst` 解压，遇到即整文件跳过并计数。

## 记录粒度

前三个源是消息级的，Hermes 是会话级的。Hermes 的 `messages.token_count` 全为 NULL，
五桶真值只存在于 `sessions` 表，消息级无法重建会话级 —— 这是数据源特性，不是缺陷。
归档层因此有 `usage_record.granularity` 列，取 `'message'` 或 `'session'`，
Hermes 每个会话归一化成一条会话级记录，`message_id` 与 `session_id` 同值，
靠既有 `UNIQUE(host_id, source, message_id)` 的 upsert 覆盖累计值。

聚合口径随之分成两组：`message_count` 只数消息级记录，`session_record_count` 只数
会话级记录，而 token 五桶、成本与 `active_session_count`（`count(DISTINCT session_id)`）
跨粒度求和。所以 Hermes 的量计入 token 与成本，但不计入消息数。

`granularity` 直接声明在 `migration_v1` 的基线 schema（`DEFAULT 'message'`），
`LATEST_SCHEMA_VERSION` 仍为 3：项目未投产，有意不写 v4 迁移。

## schema 指纹与重建

基线 schema 是就地修改而不是新增迁移，因此旧版本建出的归档库可能带着当前代码无法查询的
表列。打开归档库时会把完整的表列指纹与 `EXPECTED_SCHEMA_COLUMNS` 对比。不匹配时既不只读
打开也不静默修补：先用 `VACUUM INTO` 在同目录写出 `archive.db.backup-<时间戳>.db`，
再按当前基线重建，**不需要手工删除任何文件**。若重建后指纹仍不匹配，那是代码缺陷 ——
迁移与 `EXPECTED_SCHEMA_COLUMNS` 脱节 —— 会直接报错，而不是给出错误的查询结果。

## 定价解析

四个源里有三个拿不到金额，成本只能在查询期按定价目录估算。匹配刻意比字符串相等宽松，
分两步：

- **跨 provider 回退。** 同一模型经不同网关接入时，价格条目往往只挂在归属方名下，
  因此 `(provider, model)` 未命中时允许回退到其他 provider 下同一模型的条目，
  优先级为 `anthropic` / `google` / `openai` → `amazon-bedrock` → 其余。
- **运行档位后缀剥离。** 剥离 8 个后缀：`xhigh`、`high`、`medium`、`low`、`minimal`、
  `max`、`thinking`、`fast`。这些已确认与基础模型共享同一价格条目。

`mini` 与 `nano` **有意不剥离**：它们是独立模型、独立定价（GPT-5.4 实测分别为
2.5/15、0.75/4.5、0.2/1.25 美元每 Mtok），误剥离会造成 3 倍以上偏差。`preview` /
`latest` 是发布通道或滚动别名，`free` 是计费层级，同样不能假定等价。

实测在 251737 条归档记录上，这两步把可定价比例从 0.1% 提到 99.4%。手工覆盖价不参与
上述任何回退：用户填的价格严格按 `(provider, model)` 精确匹配，不会外溢到别的 provider。
价格编辑器支持从已有目录条目填充，那只是界面上的便利，不改变匹配规则。归档层的模型键含
`variant`，但价格条目刻意忽略它 —— 价格随模型变化，不随推理档位变化。

## 聚合性能

趋势查询是单条 SQL，而不是按桶逐次查询。在 251737 条记录的归档库上，实测墙钟时间从
6123ms 降到 514ms，约 12 倍。`migration_v1` 为此带了四个复合索引，分别建在
`(source, time_created_utc)`、`(agent_key, time_created_utc)`、
`(provider_id, time_created_utc)` 与 `(model_id, time_created_utc)` 上。
代价是库体积增长 22.7MB（+41.55%），这是为延迟接受的交换。

## 凭据

主机口令与密钥 passphrase 写入操作系统钥匙串（Linux Secret Service / Windows
凭据管理器），从不写入配置文件，也不会经 IPC 回传给界面。

## 窗口装饰

标题栏在 Windows 与 Linux 上由 React 自绘，在 macOS 上保留系统原生样式。这个分叉是
**构建期配置合并**，不是运行时调用：Tauri 在编译 macOS 目标时会把
`tauri.macos.conf.json` 合并到 `tauri.conf.json` 之上，因此 `src-tauri/src/**`
里没有任何 `set_decorations`，本次改动一行 Rust 都没动。

| 配置文件 | 生效平台 | 窗口设置 |
| --- | --- | --- |
| `src-tauri/tauri.conf.json` | Windows、Linux | `decorations: false`、`shadow: true` |
| `src-tauri/tauri.macos.conf.json` | 仅 macOS | `decorations: true`、`titleBarStyle: "Overlay"`、`hiddenTitle: true`、`trafficLightPosition: { x: 20, y: 18 }` |

`titleBarStyle` 是 macOS 专属设置，所以「一个 `decorations` 值通吃三平台」不成立：
`Overlay` 让网页内容延伸到原生红绿灯之下、同时保留红绿灯本身，而 Windows 与 Linux
没有对应能力。

合并遵循 RFC 7396，**数组是整体替换而非逐元素合并**。`app.windows` 是数组，因此 macOS
覆盖文件必须重述全部几何字段，否则 macOS 构建会静默退回 Tauri 默认窗口尺寸。
`windowConfig.test.ts` 断言两个文件声明的几何完全一致，把这种静默漂移变成失败测试。

React 侧代码在 `frontend/src/app/titlebar/`：

| 文件 | 职责 |
| --- | --- |
| `TitleBar.tsx` | 自绘的标题栏本体：拖拽区、最小化 / 最大化 / 关闭按钮 |
| `useWindowChrome.ts` | 订阅窗口状态（是否最大化、是否聚焦）并提供给标题栏 |
| `windowControls.ts` | 按钮用到的 Tauri 窗口 API 薄封装 |
| `platform.ts` | `detectPlatform(userAgent)`，决定标题栏是否渲染 |

平台判定读 user agent，而不是引入 `@tauri-apps/plugin-os`：那个插件要同时加 npm 包、
Cargo crate、builder 调用与 capability 条目，只为回答 user agent 已经回答了的问题；
而且它在 Vitest 与 Playwright 环境里不会被注入，`platform()` 会直接抛错。

### 已接受的降级：Windows 11 Snap Layouts

在 Windows 11 上，**悬停最大化按钮不再弹出 Windows 11 的布局选择面板**。相对原生装饰
窗口，这是一次真实的功能回退；它是明确接受的降级，不是待修缺陷。

原因在上游，且没有干净解法。布局面板只能通过响应 `NC_HITTEST` 消息打开，而 WebView2
不会为 webview 内部的点击发送该消息
（[tauri#4531](https://github.com/tauri-apps/tauri/issues/4531)）。它阻塞在 WebView2
支持 Window Controls Overlay 之前。

Windows 上仍然保留的：

- Aero Snap —— 拖到屏幕边缘吸附，以及 `Win` + 方向键。
- 缩放边框与 hit-testing，由 Tauri 覆盖在 webview 之上的透明子窗口实现。
- 窗口投影与 Windows 11 圆角。两者都依赖 `shadow: true`，所以要显式声明：
  `shadow: false` 会连圆角一起丢掉。

`tauri-plugin-decorum` 宣称能恢复 Snap Layouts，已被否决。它的实现是用 `enigo`
合成 `Win` + `Z` 按键，而不是真正响应 `NC_HITTEST`，所以在存在缩放边框覆盖子窗口时
飞出面板位置会错位；且该插件自 2024-09 起未再发版。它另一项被考虑的能力（红绿灯位置）
已由 Tauri 官方设置 `trafficLightPosition` 覆盖。

### 图标

`src-tauri/icons/` 下 16 个桌面正典图标文件已由 Tauri 占位换成 AgentLens 品牌图标，
`bundle.icon` 引用其中 5 个，未改动。

路线是手写 SVG → `cairosvg` 光栅化 → 量化验收 → `tauri icon`，全程不涉及 AI 生图。
两端输入均已入仓，因此整套图标可复现：候选源为
`assets/brand/candidate-{a..d}-*.svg`，验收脚本为 `assets/brand/icon_audit.py`。
唯一例外是 `icon.icns` 非字节确定 —— 同一母图多次生成的 sha256 不同，但长度恒为
195,992 字节。
