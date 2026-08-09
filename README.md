# AgentLens

[![CI](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml/badge.svg)](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/sunerpy/AgentLens/branch/main/graph/badge.svg)](https://codecov.io/gh/sunerpy/AgentLens)
![version](https://img.shields.io/github/v/release/sunerpy/AgentLens?sort=semver)
![platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey)
![license](https://img.shields.io/badge/license-MIT-green)

**简体中文** · [English](docs/readme/README.en.md)

AgentLens 是一个桌面用量看板：把本机与多台远端主机上 AI 编码工具的用量记录采集进一个本地
SQLite 归档库，再按时区、Agent、模型与项目维度看趋势、做用量分析、查单条明细。

## 目录

- [亮点](#亮点)
- [安装](#安装)
- [快速开始](#快速开始)
- [架构](#架构)
- [测试与质量](#测试与质量)
- [开发](#开发)
- [文档](#文档)
- [给-ai-编码助手](#给-ai-编码助手)
- [现状与限制](#现状与限制)
- [许可](#许可)

## 亮点

- **归档库是权威历史，永不裁剪。** 源库轮转、备份被删或远端数据目录被清空，都不会导致
  已归档的记录消失。
- **远端采集只读。** 静态链接的 musl 采集器被推送到远端、校验 sha256 后就地执行、
  退出时清理，从不写入远端工具的数据。
- **凭据只进操作系统钥匙串**（Linux Secret Service / Windows 凭据管理器）。
  口令不落任何配置文件，也不会经 IPC 回传给界面。
- **日历分桶在 Rust 侧实现。** 前端刻意不引入 `date-fns` / `dayjs` / `moment`，
  时区引擎只有一份，不存在第二套实现与它对不上。所有原始 epoch 都按报表时区分桶，
  后端已格式化好的标签不会被前端二次转换。
- **类型化 IPC。** TypeScript 契约由 `ts-rs` 从 Rust 类型生成，边界不会悄悄漂移。
- **定价能跨 provider 回退。** 同一模型经不同网关接入时价格条目往往只挂在归属方名下，
  匹配因此允许跨 provider 回退，并剥离运行档位后缀（`max` / `thinking` / `fast` 等 8 个）。
  实测 251737 条记录的可定价比例从 0.1% 升到 99.4%。手工覆盖价则严格按
  `(provider, model)` 精确匹配，不会跨 provider 外溢。

## 安装

预编译包：`.deb`（Linux x86_64）、NSIS 安装包（Windows x64）、`.dmg`（macOS
aarch64）。一行式安装脚本会识别平台、**用发布清单校验 SHA-256**，并且从不自行提权。

```sh
curl -fsSL https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.sh | bash
```

```powershell
irm https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.ps1 | iex
```

不愿把脚本管道给 shell 的话，从 release 页面下载并手工校验：

```sh
sha256sum -c sha256sums-linux.txt
sudo apt install ./AgentLens_*_amd64.deb
```

完整步骤、环境变量、安装后的文件布局与从源码构建：
[docs/installation.md](docs/installation.md)。

## 快速开始

1. 安装后启动 **AgentLens**。
2. 打开「主机管理」。本机会在首次打开时自动注册，无需配置。
3. 添加 SSH 主机，点「测试连接」。机器标识哈希会由探测结果自动填入并转为只读，保存即可。
4. **在主机卡片上勾选要采集的源。** 启用入口在主机卡片上，不在设置页；默认只有 OpenCode，
   Claude Code、Codex 与 Hermes 需要逐个勾选。
5. 在主机卡上点刷新即可采集。本机与远端都可以改成自动刷新，远端用独立的间隔，
   两者的下限都是 600 秒。

逐步说明，包含机器标识去重规则与采集器的传输方式：
[docs/remote-hosts.md](docs/remote-hosts.md)。

归档库、价格覆盖表与密钥在各平台的存放位置：
[docs/data-storage.md](docs/data-storage.md)。

## 架构

| 组成 | 路径 | 职责 |
| --- | --- | --- |
| 核心 crate | `crates/agentlens-core` | 归档、解析、聚合、SSH 传输 |
| 远端采集器 | `crates/agentlens-collector` | 静态 musl 单文件，按需推送到远端 |
| 口令助手 | `crates/agentlens-askpass` | `SSH_ASKPASS` 对端，随包分发 |
| 桌面壳 | `src-tauri/` | Tauri 2 宿主、IPC 命令、托盘 |
| 前端 | `frontend/` | React 18.3.1 + Vite 8 + Tailwind v4 |

归档库是带去重与按源水位线的 SQLite；SSH 传输使用恒定的远端命令，载荷作为一个位置参数
传入。详见 [docs/architecture.md](docs/architecture.md)。

## 测试与质量

| 层级 | 数量 | 命令 |
| --- | --- | --- |
| Rust workspace | 414 passed / 0 failed / 21 ignored | `make test` |
| Vitest 单测 | 26 个文件共 497 条 | `make test-unit` |
| Playwright 组件级 | 12 个 spec 文件共 126 条，mock IPC | `make test-e2e` |
| WebdriverIO | 8 个 spec，真 Tauri WebView 对 155k 行归档库 | `make test-e2e-real` |
| 行覆盖率 | workspace 实测 92.57%（53101/57363），下限 90% 由 `make coverage-gate` 强制 | `make coverage-gate` |

覆盖率的硬地板是 `COVERAGE_MIN := 90`，写在 Makefile 里且用 `:=` 压过同名环境变量，
避免被静默降低。实测 92.57% 相对上一轮的 92.61% 基本持平，分母从 43674 行涨到 57363 行 ——
本轮新增的定价回退、粒度聚合与主题层都带了测试，而不是靠缩小分母保住比例。

GitHub Actions 的三平台矩阵（ubuntu / windows / macos）在 `main` 上全绿，三平台也都在
AWS CodeBuild 上出过真实安装包。但**构建绿灯只说明缺陷没有复现，不代表产品可用**，
所以 Windows 上另做了真机验收：EC2 Windows Server 上安装包被真实安装、应用被真实启动，
25 条机器可判定的 GUI 断言全过，`install.ps1` 另有一轮端到端 38/38。
**Linux 与 macOS 的安装包仍未在真机上启动过。**

每个平台的构建 ID、产物字节数、分平台测试数量差异的原因，以及真机验收的逐条断言：
[docs/readme/development.zh.md](docs/readme/development.zh.md)。

## 开发

```sh
make help          # 列出全部目标
make dev           # Tauri 开发模式
make fmt           # 格式化 Rust + 前端
make lint          # cargo fmt/clippy + 前端 lint/typecheck + 文案门禁
make test          # cargo test --workspace
make test-unit     # vitest
make test-e2e      # Playwright 组件级 QA（mock IPC）
make test-e2e-real # WebdriverIO 对真 WebView
make coverage-gate # 覆盖率并强制下限
make dist          # 产出 artifacts/dist/
```

更多内容，包含 `dist` 系列目标与 AWS CodeBuild 路径：
[docs/development.md](docs/development.md)。

## 文档

- [安装](docs/readme/installation.zh.md)
- [仓库元数据](docs/readme/repo-metadata.zh.md)
- [添加远端主机](docs/readme/remote-hosts.zh.md)
- [数据存放与设置](docs/readme/data-storage.zh.md)
- [架构](docs/readme/architecture.zh.md)
- [开发与构建](docs/readme/development.zh.md)
- [Remote Source API v1](docs/remote-source-api.md)
- [四个源的计量口径（横向对照）](docs/measurement.md)
- 适配器契约：[Codex](docs/adapters/codex.md)、
  [Claude Code](docs/adapters/claude-code.md) 与 [Hermes](docs/adapters/hermes.md)

英文版文档在 [docs/](docs/) 下同名文件。`docs/measurement.md` 与 `docs/adapters/`
只有中文版，因为它们是逐字段的口径契约，翻译副本一旦漂移比没有更糟。

## 给 AI 编码助手

在本仓库里改代码前，按顺序读这三份，能省掉大部分返工：

1. [docs/measurement.md](docs/measurement.md) —— 四个源「一条记录代表什么」的差异。
   动任何聚合口径之前必读，否则很容易把消息级与会话级的计数相加。
2. [架构](docs/readme/architecture.zh.md) —— 归档表、定价解析、IPC 边界与窗口装饰的取舍。
3. [开发与构建](docs/readme/development.zh.md) —— 全部 `make` 目标与验证路径。

三条硬约束：

- **时间处理只在 Rust 侧。** 不要给 `frontend/` 引入 `date-fns` / `dayjs` / `moment`；
  第二套时区实现就是缺陷发生器。
- **TypeScript 契约是生成物。** 不要手改 `frontend/src/generated/`；改完 Rust 侧 DTO 后跑
  `cargo test -p agentlens-tauri --features ts-export bindings_export` 重新导出。
  CI 有一道零漂移门禁，生成物与 Rust 类型不一致就红。
- **交付前跑 `make lint` 与 `make test`。** 覆盖率地板 90%，`make coverage-gate` 会拦。

## 现状与限制

- 版本号唯一事实源是根 `Cargo.toml` 的 `[workspace.package].version`：各 crate 与
  `src-tauri` 均继承它，`tauri.conf.json` 不再重复声明版本，`make dist-version`
  可回显解析结果。已发布的版本以页首 badge 与 release 页面为准。
- **已实现的适配器有四个：OpenCode、Claude Code、Codex 与 Hermes。** 四者都走本地与远端
  （采集器 `--source`）两条路径，同一主机可同时采集多个源，`SUPPORTED_SOURCES` 现为四元。
  `hosts.enabled_sources` 的默认值是 `'opencode'`，既有主机升级后只挂 OpenCode，
  其余三个都需要 **显式启用**。
- **三个消息级源 + 一个会话级源，这个分野会影响你看到的「消息数」。** OpenCode、
  Claude Code、Codex 的 token 挂在单条消息上；Hermes 不同 —— 它的 `messages.token_count`
  全是 NULL，五桶真值只存在于 `sessions` 表（实测 158 行消息里 0 行带 token，
  9 行会话合计 2038297）。这是数据源特性，不是缺陷，所以归档层引入了 `granularity`
  列（`'message'` / `'session'`），Hermes 每个会话归一化成一条会话级记录。
  由此聚合口径分成两组：`message_count` 只数消息级、`session_record_count` 只数会话级，
  而 token 五桶、成本与 `active_session_count` 跨粒度求和。**Hermes 的用量计入 token
  与成本，但不计入消息数**，改为计入会话汇总记录数；把这两个数相加是错的。
  `granularity` 与本轮新增的四个复合索引直接写在 `migration_v1` 基线 schema 里
  （`granularity` 带 `DEFAULT 'message'`），`LATEST_SCHEMA_VERSION` 仍为 3 ——
  项目未投产，有意不写 v4 迁移。**早于本轮创建的开发用归档库与当前基线不兼容**，
  但不需要手工删除：打开归档库时会校验表列指纹，不匹配就先 `VACUUM INTO` 出一份
  `archive.db.backup-<时间戳>.db`，再按当前基线重建。
- **6 个模型共 1617 条记录仍估不出成本。** 逐个是 `claude-haiku-4-5` 1510 条、
  `antigravity-gemini-3.1-pro` 75 条、`claude-sonnet-4-5` 20 条、`gpt-5.6` 9 条、
  `big-pickle` 2 条、`auto` 1 条 —— 定价目录里没有对应条目，落在 `unavailable`
  而不是被估成 0。设置页可以手工补价。
- **Claude Code、Codex 与 Hermes 都已对真实数据实测对账，但仓库内没有真实 fixture。**
  Claude Code 对本机 `~/.claude/projects` 的 645 个 jsonl / 5222 行跑过全量采集，
  17 条 `messageId` 全部命中、五桶合计 404254 与独立提取逐桶一致；Codex 对 220 个
  rollout 采出 20252 条事件，五桶数值与独立提取零缺失零多余，定价命中 19952/20252；
  Hermes 采出 9 条会话级记录、四桶合计 2038297、`skipped=0`，与独立 SQL 完全一致。
  这三轮对账都在本机私人数据上完成，数据不入库，所以仓库内的自动化测试仍全部基于合成数据
  （Claude Code 14 个、Codex 5 个、Hermes 6 个 `#[test]`）。
- **Codex 的记录全部标成 `openai`，接入通道信息丢失。** `provider_id` 取的是
  `turn_context.model` 的 namespace，所以实测 20252 条 Codex 记录的 provider 一律是
  `openai`，而其中 17317 条实际经 amazon-bedrock 接入。这是有意的取舍：定价目录按模型归属方
  组织，照 `session_meta.model_provider`（转发通道）取值会让 85% 的记录永久估不出成本。
  代价是两条 —— 通道维度在归档层不可见，且 Bedrock 单价与直连并不相同
  （实测 5.5/27.5 与 5.0/25.0 美元每 Mtok），因此这部分成本估算带系统性偏差。
- **Codex 不支持 `.jsonl.zst`。** 契约要求流式解压，但本机零个 zst 文件且 workspace
  无 zstd 依赖，本轮有意不实现：遇到即整文件跳过并计数。这是范围限制，不是缺陷。
- **Hermes 的本地 Ollama 模型没有价格。** provider 归一化后写 `ollama`，而
  `pricing_catalog.json` 里没有这个 provider，定价匹配的三层都要求 provider 相等，
  所以本地模型一律不估价。实测 6 条 ollama 记录全部未命中、3 条 anthropic 正常命中 ——
  这是正确结果，避免本地模型误套云端价格。
- 无论从哪个平台管理，远端主机都是 Linux 主机，因此随包分发的采集器是 Linux 静态二进制。
- **Windows 11 Snap Layouts 丢失。** 标题栏在 Windows 与 Linux 上由应用自绘，因此悬停
  最大化按钮不再弹出 Windows 11 的布局选择面板。Aero Snap、缩放边框、窗口投影与圆角
  均保留。这是阻塞在上游 WebView2 的已接受降级，不是待修项；详细取舍见
  [docs/readme/architecture.zh.md](docs/readme/architecture.zh.md#已接受的降级windows-11-snap-layouts)。
- **尚未发布 release。** 安装一行式指向的脚本已在仓库中，但 release 页面还没有产物，
  因此下载步骤暂时拿不到包。`install.ps1` 已在真实 Windows 上端到端验证 38/38 通过，
  覆盖真实 NSIS 取消码、校验和拒绝与非 HTTPS 拒绝等场景；`install.sh` 只做过 shellcheck
  静态检查与本地源实测，**两个脚本都从未拉取过真实 GitHub Release**（因为还没有 release）。
  仓库描述与 topics 的记录：[docs/readme/repo-metadata.zh.md](docs/readme/repo-metadata.zh.md)。

## 许可

[MIT](LICENSE) © 2026 sunerpy。同一声明也出现在根 `Cargo.toml` 的
`license = "MIT"`（各 crate 继承）与 `frontend/package.json` 的
`"license": "MIT"`。
