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
  时区引擎只有一份，不存在第二套实现与它对不上。
- **类型化 IPC。** TypeScript 契约由 `ts-rs` 从 Rust 类型生成，边界不会悄悄漂移。

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
4. 在主机卡上点刷新即可采集。

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
| Rust workspace | 全绿（`cargo test --workspace`） | `make test` |
| Vitest 单测 | 15 个 spec 共 268 条 | `make test-unit` |
| Playwright 组件级 | 58 个 spec，mock IPC | `make test-e2e` |
| WebdriverIO | 8 个 spec，真 Tauri WebView 对 155k 行归档库 | `make test-e2e-real` |
| 行覆盖率 | workspace 实测 91.63%（15894/17346），下限由 `make coverage-gate` 强制 | `make coverage-gate` |

GitHub Actions 的三平台矩阵（ubuntu / windows / macos）在 `main` 上全绿。更早一轮
H4b 之后的代码也在 `us-east-2` 的 AWS CodeBuild 上三平台构建通过：Linux
`d2edbcdd`（5,709,438 字节 `AgentLens_0.1.0_amd64.deb`，182 passed / 0 failed /
18 ignored）、Windows `39f89617`（4,142,828 字节
`AgentLens_0.1.0_x64-setup.exe`，170 passed / 0 failed / 8 ignored）、macOS
`82b4d172`（5,862,574 字节 `AgentLens_0.1.0_aarch64.dmg`，180 passed / 0 failed /
10 ignored）。三个数量本就不该一致：`#[cfg(unix)]` 门控的测试在 Windows 上不参与编译，
`#[cfg(target_os = "linux")]` 门控的测试在 macOS 上不参与编译，在对应平台上是
「不存在」而非「被忽略」。构建绿灯只说明缺陷没有复现，不代表产品可用，所以 Windows 上做了
真机验收：EC2 Windows Server 上安装包被真实安装、应用被真实启动，GUI 验收
（run `h7-20260805T123646Z`）25 条机器可判定断言全过 —— 客户区精确 1180x780、无原生标题栏、
最小尺寸 900x600、真实 SendInput 拖拽零漂移、关闭按钮走 `prevent_close + hide` 而不退出应用。
`install.ps1` 另有一轮端到端验证 38/38 通过（run `installps1-20260805T111723Z`），并借此修掉了
一个真实缺陷：`Start-Process -PassThru -Wait` 会等整个进程树，而 NSIS 完成页默认勾选
「运行 AgentLens」，导致脚本永不返回；改用 `ProcessStartInfo` + `WaitForExit()` 后正常退出。
**Linux 与 macOS 的安装包仍未在真机上启动过。**

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
- 适配器契约：[Codex](docs/adapters/codex.md) 与
  [Claude Code](docs/adapters/claude-code.md)

英文版文档在 [docs/](docs/) 下同名文件。

## 现状与限制

- 版本号唯一事实源是根 `Cargo.toml` 的 `[workspace.package].version`：各 crate 与
  `src-tauri` 均继承它，`tauri.conf.json` 不再重复声明版本，`make dist-version`
  可回显解析结果。已发布的版本以页首 badge 与 release 页面为准。
- **只有 OpenCode 适配器已实现。** Codex 与 Claude Code 文档描述的是**预留契约**，
  不是可用的采集能力。
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
