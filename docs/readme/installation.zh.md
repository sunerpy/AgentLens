# 安装

[← README](../../README.md) · [English](../installation.md)

## 一行式安装

脚本会识别操作系统与 CPU 架构，挑出该组合下**真实存在**的那一个发布产物，
**用发布页的 `sha256sums-<os>.txt` 校验 SHA-256**，再把安装包交给你。脚本不会
自己提权：它只打印安装命令，只有设置 `AGENTLENS_INSTALL=1` 时才代为执行。

Linux 与 macOS：

```sh
curl -fsSL https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.sh | bash
```

Windows（PowerShell）：

```powershell
irm https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.ps1 | iex
```

| 环境变量 | 作用 |
| --- | --- |
| `AGENTLENS_VERSION` | 安装指定版本（如 `0.1.0`），而不是最新 release |
| `AGENTLENS_REPO` | 下载来源的 GitHub `owner/repo` |
| `AGENTLENS_DOWNLOAD_DIR` | 安装包落盘位置 |
| `AGENTLENS_INSTALL=1` | 校验通过后执行平台安装器（用 sudo 前会先声明） |
| `AGENTLENS_DRY_RUN=1` | 只打印解析结果，下载前停止 |

各平台的已发布产物：Linux x86_64 是 `.deb`，Windows x64 是 NSIS `-setup.exe`，
macOS aarch64 是 `.dmg`。**没有** arm64 的 `.deb`，没有 32 位或 arm64 的 Windows
包，也没有 Intel 版 `.dmg`；遇到这些组合脚本会带说明失败并指向
[从源码构建](#从源码构建)，绝不下载错误的文件。

不想把脚本管道给 shell 的话，下面每种方式都可独立使用：从 release 页面下载，
再按对应平台小节操作。

> 上面两个 URL 指向真实仓库 sunerpy/AgentLens，但该仓库尚未创建，所以现在都会 404。
> 脚本只对本地源做过验证，从未拉取过真实 release。被此阻塞的完整清单见
> [repo-metadata.zh.md](repo-metadata.zh.md)。

## Linux（deb）

从 release 页面下载 `AgentLens_<版本>_amd64.deb` 与 `sha256sums-linux.txt`，先校验
再安装。把 `<版本>` 换成你实际下载的版本号。

```sh
sha256sum -c sha256sums-linux.txt    # 先校验完整性，再安装
sudo apt install ./AgentLens_<版本>_amd64.deb
```

安装后可执行文件位于 `/usr/bin/`：

| 路径 | 用途 |
| --- | --- |
| `/usr/bin/agentlens-tauri` | 桌面应用主程序（桌面菜单项名为 AgentLens） |
| `/usr/bin/agentlens-askpass` | SSH 口令助手，由应用在需要时经 `SSH_ASKPASS` 调用 |
| `/usr/bin/agentlens-collector-x86_64-unknown-linux-musl` | x86_64 远端采集器 |
| `/usr/bin/agentlens-collector-aarch64-unknown-linux-musl` | aarch64 远端采集器 |

## Windows（NSIS）

运行 release 页面的 NSIS 安装包（`*-setup.exe`）。安装目录内除主程序外还包含
`agentlens-askpass.exe`、上表两个 musl 采集器，以及 `collectors.sha256` 校验清单。
Windows 端管理的远端仍是 Linux 主机，因此采集器仍是 Linux 静态二进制。

## macOS（dmg）

打开 release 页面的 `.dmg`，把 AgentLens 拖入「应用程序」。macOS 包在 AWS CodeBuild
上针对 `aarch64` 构建（构建 `82b4d172` 产出 5,862,574 字节的
`AgentLens_0.1.0_aarch64.dmg`）。

## 从源码构建

```sh
make dist        # 产出 artifacts/dist/ 下的 deb、双架构 collector 与 sha256sums.txt
make dist-all    # 同上，但缺少 aarch64 collector 时直接失败（发布用）
```

需要 `rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl`、
`musl-gcc`，以及一个 aarch64 musl 的 C 交叉编译器（`aarch64-linux-musl-gcc`，
或安装 `zig` 由 Makefile 自动包装成 `zig cc -target aarch64-linux-musl`）。

缺后者时 `make dist` 会打印醒目告警、只产出 x86_64 采集器，并在 `sha256sums.txt`
首部注明 aarch64 缺席，不会伪造产物。

## 下一步

- [添加远端主机](remote-hosts.zh.md)
- [数据存放与设置](data-storage.zh.md)
