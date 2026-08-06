# 开发与构建

[← README](../../README.md) · [English](../development.md)

## 日常目标

```sh
make help          # 列出全部目标
make dev           # Tauri 开发模式
make fmt           # 格式化 Rust + 前端
make fmt-check     # 只校验格式，不写回
make lint          # cargo fmt/clippy + 前端 lint/typecheck + 文案门禁
make test          # cargo test --workspace
make test-unit     # vitest
make coverage      # 覆盖率报告输出到 artifacts/coverage/
make coverage-gate # 覆盖率并强制 75% 下限
make hooks         # 安装 pre-commit / pre-push 钩子
```

## 前端脚本

在 `frontend/` 下运行，或经上面的 Makefile 目标调用。

```
dev  build  lint  format  format:check  preview  typecheck
test:unit  test:unit:coverage  test:e2e  test:e2e:real  check:i18n
```

## 测试层级

| 层级 | 数量 | 命令 |
| --- | --- | --- |
| Rust workspace | 182 passed / 0 failed / 18 ignored | `make test` |
| Vitest 单测 | 15 个 spec 共 268 条 | `make test-unit` |
| Playwright 组件级 | 58 个 spec，mock IPC | `make test-e2e` |
| WebdriverIO | 8 个 spec，真 Tauri WebView 对 155k 行归档库 | `make test-e2e-real` |

行覆盖率强制下限 75%，这条下限才是长期成立的保证。实测百分比按运行环境而异：
`make coverage-gate` 在 HEAD 本地 Linux 上报 76.92%（12025/15633 行），在 CodeBuild
Linux runner 上报 79.56%（11252/14143 行），差异来自 llvm-cov 的行基数随环境变化。
本地相对下限的余量是 1.92pp，比上一轮的 1.85pp 略宽，没有回退。余量之所以一直不厚，
是因为 Rust 侧覆盖率最低的是 Tauri 运行时接线（`state.rs`、`tray.rs`），它们需要真
`AppHandle` 与事件循环，低是结构性的，不是遗漏。

## 打包

```sh
make dist            # deb + 双架构 collector + sha256sums.txt
make dist-all        # 同上，缺 aarch64 collector 时直接失败
make dist-version    # 回显解析出的版本号
make dist-verify     # 校验暂存的产物
make dist-clean      # 清理 artifacts/dist/
```

前置条件与 aarch64 缺失时的行为见
[installation.zh.md](installation.zh.md#从源码构建)。

## AWS CodeBuild

```sh
make aws-source-upload
make aws-build-linux
make aws-build-windows
make aws-build-macos
make aws-status
make aws-logs
```

三平台均已在 `us-east-2` 的 CodeBuild 上针对 H4b 之后的代码构建通过：Linux
`d2edbcdd`（5,709,438 字节 `AgentLens_0.1.0_amd64.deb`，182 passed / 0 failed /
18 ignored）、Windows `39f89617`（4,142,828 字节
`AgentLens_0.1.0_x64-setup.exe`，170 passed / 0 failed / 8 ignored）、macOS
`82b4d172`（真实的 5,862,574 字节 `AgentLens_0.1.0_aarch64.dmg`，180 passed /
0 failed / 10 ignored）。Linux 与 Windows 读的是 `13:17:14Z` 的源码 zip，macOS 读的
是更晚的 `14:55:03Z`，中间只有文档提交，因此产品代码一致，但并不是字面上同一个 zip。
三个数量本就不该一致：`#[cfg(unix)]` 门控的测试在 Windows 上不参与编译，
`#[cfg(target_os = "linux")]` 门控的测试在 macOS 上不参与编译，在对应平台上是
「不存在」而非「被忽略」。构建成功只说明缺陷没有复现，不等于有人启动过安装包；
`.omo/evidence/aws-aw5-test-matrix.md` 记录的是更早一轮构建（更早的 `us-west-2`
区域）的分平台对账。buildspec 与相应
说明在 `.aws/` 下，见 [.aws/README.md](../../.aws/README.md)。

## 持续集成

`.github/workflows/ci.yml` 已编写且通过 `actionlint` 检查，但**从未执行过**：本仓库
尚未配置 git remote，GitHub Actions 没有可运行的对象。在创建远端仓库前，README 里的
CI 徽章应被视为「no status」。

## 版本号

唯一事实源是根 `Cargo.toml` 的 `[workspace.package].version`。各 crate 与 `src-tauri`
均继承它，`tauri.conf.json` 不再自行声明版本，`make dist-version` 可回显解析结果。
