# AgentLens

[![CI](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml/badge.svg)](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/sunerpy/AgentLens/branch/main/graph/badge.svg)](https://codecov.io/gh/sunerpy/AgentLens)
![version](https://img.shields.io/github/v/release/sunerpy/AgentLens?sort=semver)
![platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey)
![license](https://img.shields.io/badge/license-MIT-green)

**简体中文** · [English](docs/readme/README.en.md)

AgentLens 是一个桌面用量看板：把本机与多台远端主机上 OpenCode、Claude Code、Codex 与
Hermes 的用量记录采集进一个本地 SQLite 归档库，再按时区、Agent、模型与项目维度看趋势、
查单条明细。默认只采集 OpenCode，其余三个源在主机卡片上逐个勾选后启用。

## 目录

- [界面](#界面)
- [亮点](#亮点)
- [安装](#安装)
- [快速开始](#快速开始)
- [架构](#架构)
- [测试与质量](#测试与质量)
- [开发](#开发)
- [文档](#文档)
- [许可](#许可)

## 界面

总览页：左侧三态侧栏，右侧是区间与粒度、四类 Token 分桶、成本卡与用量趋势图。成本卡默认
只给一个数：本机价目表乘可计费 Token 算出的「本地估算」，旁边标出它覆盖了多少条记录，
以及每百万可计费 Token 的单价，单价是这张卡里唯一可以横向比较的数。带上游自带金额的记录
折叠在一个入口后面，目录里查不到价格的记录另列，两者都不并进估算值。趋势图的斜纹区是
无数据覆盖的断裂桶，底色区是部分覆盖，二者都不会被当成 0。

![总览页](assets/screenshots/overview.png)

同一张趋势图切到「按模型」分组，并换成深海蓝主题。分组维度有不分组 / 模型 / agent / 工具
四种，主题共六套，都在标题栏就地切换。侧栏可展开、收起成 64px 图标栏，或整条隐藏。

![按模型分组的总览页，深海蓝主题](assets/screenshots/overview-by-model-dark.png)

用量分析页：来源 → agent → 模型 三级展开，区间与时区与总览共享同一状态。缺价的行挂
「成本缺失」标记而不是填 0，占比按本级 token 合计算。

![用量分析页的三级展开](assets/screenshots/usage-drilldown.png)

主机页：本机与 SSH 远端并列，采集源在各自的主机卡片上逐个勾选。默认只有 OpenCode，
Claude Code、Codex 与 Hermes 需要显式启用。

![主机管理页](assets/screenshots/hosts.png)

## 亮点

- **归档库是权威历史，永不裁剪。** 源库轮转、备份被删或远端数据目录被清空，都不会导致
  已归档的记录消失。
- **远端采集只读。** 静态链接的 musl 采集器被推送到远端、校验 sha256 后就地执行、
  退出时清理，从不写入远端工具的数据。
- **凭据只进操作系统钥匙串**（Linux Secret Service / Windows 凭据管理器）。
  口令不落任何配置文件，也不会经 IPC 回传给界面。
- **日历分桶只有一份实现，在 Rust 侧。** 前端不引入 `date-fns` / `dayjs` / `moment`，
  所有原始 epoch 都按报表时区分桶，后端格式化好的标签不会被前端二次转换。
- **类型化 IPC。** TypeScript 契约由 `ts-rs` 从 Rust 类型生成，边界不会悄悄漂移。
- **定价能跨 provider 回退。** 同一模型经不同网关接入时，价格条目往往只挂在归属方名下，
  匹配因此允许跨 provider 回退。实测 251737 条记录的可定价比例从 0.1% 升到 99.4%。
  手工覆盖价仍按 `(provider, model)` 精确匹配，不会外溢。
  规则见 [docs/measurement.md](docs/measurement.md)。

## 安装

预编译包：`.deb`（Linux x86_64）、NSIS 安装包（Windows x64）、`.dmg`（macOS
aarch64）。一行式安装脚本会识别平台、**用发布清单校验 SHA-256**，并且从不自行提权。

```sh
curl -fsSL https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.sh | bash
```

```powershell
irm https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.ps1 | iex
```

不想把脚本管道给 shell，就从 release 页面下载后手工校验：

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
| Rust workspace | 426 条 | `make test` |
| Vitest 单测 | 560 条 | `make test-unit` |
| Playwright 组件级 | 151 条，mock IPC | `make test-e2e` |
| WebdriverIO | 8 个 spec，真 Tauri WebView 对 155k 行归档库 | `make test-e2e-real` |
| 行覆盖率 | 实测 92.72%，下限 90% 由 `make coverage-gate` 强制 | `make coverage-gate` |

GitHub Actions 的三平台矩阵（ubuntu / windows / macos）在 `main` 上全绿，三平台也都在
AWS CodeBuild 上出过真实安装包。但**构建绿灯只说明缺陷没有复现，不代表产品可用**，
所以 Windows 上另做了真机验收：EC2 Windows Server 上装了包、起了应用，25 条机器可判定的
GUI 断言全过。

覆盖率地板的实现方式、每个平台的构建 ID、产物字节数，以及真机验收的逐条断言：
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
- [给 AI 编码助手](docs/readme/ai-assistants.zh.md)
- [Remote Source API v1](docs/remote-source-api.md)
- [四个源的计量口径（横向对照）](docs/measurement.md)
- 适配器契约：[Codex](docs/adapters/codex.md)、
  [Claude Code](docs/adapters/claude-code.md) 与 [Hermes](docs/adapters/hermes.md)

英文版文档在 [docs/](docs/) 下同名文件。`docs/measurement.md` 与 `docs/adapters/`
只有中文版，因为它们是逐字段的口径契约，翻译副本一旦漂移比没有更糟。

## 许可

[MIT](LICENSE) © 2026 sunerpy
