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
make coverage-gate # 覆盖率并强制 90% 下限
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
| Rust workspace | 414 passed / 0 failed / 21 ignored | `make test` |
| Vitest 单测 | 26 个 spec 共 497 条 | `make test-unit` |
| Playwright 组件级 | 12 个 spec 文件共 126 条，mock IPC | `make test-e2e` |
| WebdriverIO | 8 个 spec，真 Tauri WebView 对 155k 行归档库 | `make test-e2e-real` |

行覆盖率强制下限 90%，在 Makefile 里写作 `COVERAGE_MIN := 90`。用 `:=` 是刻意的：
它压过同名环境变量，硬地板不会被静默降低，而命令行 `make coverage-gate COVERAGE_MIN=...`
仍保持最高优先级。`make coverage-gate` 在 HEAD 本地 Linux 上报 92.57%（53101/57363 行）。
实测百分比按运行环境而异 —— llvm-cov 的行基数随环境变化，所以长期成立的保证是下限本身
而不是这个数字。余量之所以一直不厚，是因为 Rust 侧覆盖率最低的是 Tauri 运行时接线
（`state.rs`、`tray.rs`），它们需要真 `AppHandle` 与事件循环，低是结构性的，不是遗漏。

门禁读的就是 Codecov 消费的那份 `artifacts/coverage/lcov.info` 字节，
因此本地数字与面板数字不会漂移。

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

## 真机验收

构建绿灯只说明缺陷没有复现。三个平台里只有 Windows 另外做过真机验收：EC2 Windows Server
上安装包被真实安装、应用被真实启动，GUI 验收（run `h7-20260805T123646Z`）25 条机器可判定
断言全过 —— 客户区精确 1180x780、无原生标题栏、最小尺寸 900x600、真实 SendInput 拖拽零漂移、
关闭按钮走 `prevent_close + hide` 而不退出应用。

`install.ps1` 另有一轮端到端验证 38/38 通过（run `installps1-20260805T111723Z`），
并借此修掉了一个真实缺陷：`Start-Process -PassThru -Wait` 会等整个进程树，而 NSIS
完成页默认勾选「运行 AgentLens」，导致脚本永不返回；改用 `ProcessStartInfo` +
`WaitForExit()` 后正常退出。

**Linux 与 macOS 的安装包仍未在真机上启动过。**

## 持续集成

`.github/workflows/ci.yml` 已在 GitHub Actions 上运行，`main` 分支全绿。除格式化、
clippy 与各测试层级外，有两道门禁值得在推送前知道：

- **ts-rs 生成物零漂移。** 该门禁先清空 `frontend/src/generated/`，用
  `cargo test -p agentlens-tauri --features ts-export bindings_export` 重新导出全部 DTO，
  再用限定路径的 `git status` 判定是否有 diff。手改 bindings 会在这里失败。
- **覆盖率。** `make coverage-gate` 只在 Linux 上跑，也只有 Linux 上传 Codecov ——
  三平台都传会让同一份代码被重复计数。

`.github/workflows/pr-title.yml` 强制 PR 标题符合 Conventional Commits。这不是洁癖：
仓库用 squash 合并，release-please 只能看到 squash 提交的主题，一个标题为 `chore:` 的 PR
会把分支上的 `feat:` / `fix:` 提交整体吞掉，版本号从此静默不再递增。

release-please 在 `main` 上维护着一个常驻的 release PR。**尚未发布任何 release**，
因此版本徽章显示为「no status」，安装脚本的下载路径也从未拉取过真实产物。

## 版本号

唯一事实源是根 `Cargo.toml` 的 `[workspace.package].version`。各 crate 与 `src-tauri`
均继承它，`tauri.conf.json` 不再自行声明版本，`make dist-version` 可回显解析结果。
