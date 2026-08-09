# 仓库元数据与未被证明项清单

[← README](../../README.md) · [English](../repo-metadata.md)

远端已存在：`sunerpy/AgentLens`，公开仓库。描述与 topics 都已应用，CI 已在
GitHub Actions 上跑过并在 `main` 全绿。本页记录两件事：这些元数据是怎么定的，
以及**在没有 release 之前仍然无法被证明的那些项**。

## 目录

- [仓库描述](#仓库描述)
- [Topics](#topics)
- [可直接粘贴的 gh 命令](#可直接粘贴的-gh-命令)
- [owner 占位符清扫：已完成](#owner-占位符清扫已完成)
- [仍未被证明的项](#仍未被证明的项)

## 仓库描述

**已应用**。描述用中文，与 README 首句同一说法，避免两处漂移：

```text
把本机与远端 SSH 主机上 AI 编码工具的用量采集进本地 SQLite 归档库的桌面看板
```

描述用中文、topics 用英文，这不是不一致：描述是给人读的第一句话，README 的默认视图
也是中文；而 topics 是 GitHub 的全局检索维度，要求小写连字符英文，中文 topic 检索不到。

## Topics

**已应用，9 个**，每一个都名副其实。不写任何「愿望型」标签：没有 `electron`、没有
`openai`，因为构建里并不存在这些。`opencode` 在列，是因为它是默认启用的数据源
（`enabled_sources` 默认值为 `'opencode'`）。此后落地的 `claude-code`、`codex`、
`hermes` 三个适配器是否也要各加一个 topic，属于仓库元数据决策，刻意留待另行决定。

| Topic | 为什么成立 |
| --- | --- |
| `tauri` | 桌面外壳是 Tauri 2（`src-tauri/`） |
| `rust` | core / collector / askpass 三个 crate 都是 Rust |
| `react` | 前端是 React 18.3.1 + Vite + Tailwind |
| `sqlite` | 归档库是 SQLite，也是本产品的核心主张 |
| `desktop-app` | 交付形态是 `.deb` / NSIS / `.dmg`，不是服务 |
| `ai-agents` | 主题即编码智能体的用量记录 |
| `token-usage` | 被度量的量：token 与推导出的成本 |
| `opencode` | 默认启用的数据源适配器（`enabled_sources` 的默认值） |
| `ssh` | 远端采集走 SSH，推送 collector 执行 |

## 可直接粘贴的 gh 命令

描述与 topics **已经应用**，下面留作复现与核对用。`--add-topic` 是增量的，重复执行安全。

```bash
# 1. 确认远端存在且 gh 能看到
gh repo view sunerpy/AgentLens --json name,description,repositoryTopics

# 2. 描述
gh repo edit sunerpy/AgentLens \
  --description "把本机与远端 SSH 主机上 AI 编码工具的用量采集进本地 SQLite 归档库的桌面看板"

# 3. Topics（--add-topic 是增量的，可重复执行）
gh repo edit sunerpy/AgentLens \
  --add-topic tauri \
  --add-topic rust \
  --add-topic react \
  --add-topic sqlite \
  --add-topic desktop-app \
  --add-topic ai-agents \
  --add-topic token-usage \
  --add-topic opencode \
  --add-topic ssh

# 4. 复核
gh repo view sunerpy/AgentLens --json description,repositoryTopics,homepageUrl
```

homepage 刻意不设：没有文档站点，指回仓库本身没有任何信息量。

还有两处仓库**设置**必须在 GitHub UI 上人工改，都无法从这里脚本化：

- **Settings → General → Pull Requests → squash 提交信息选 "Pull request
  title"**。不设的话，release-please 只看得到 squash 提交的标题；一个标题写成
  `chore:` 的 PR 会把分支里的 `feat:` / `fix:` 全部吞掉，**升版随即静默停止**，
  没有任何报错。`.github/workflows/pr-title.yml` 只能强制标题格式，squash 策略
  本身是 UI 设置。
- **Settings → Actions → General → Workflow permissions 选 read and write**。
  发布相关 job 各自声明了 `permissions: contents: write`，但仓库级锁成只读会
  覆盖 job 级声明。

## owner 占位符清扫：已完成

owner 占位符已经清空。`sunerpy` 已替换到每一处 URL、脚本默认值和一行式命令；两处
「因为拿不到仓库才存在」的安装器守卫块已整块删除。

数出来的，不是记出来的。下面是审计命令，现在它应当只报出一行，来自
`.aws/buildspec/macos.yml`：

```bash
git ls-files -z -- ':!.omo' | xargs -0 grep -n 'OWN''ER'
```

字面量被拆开再拼回是刻意的，不是笔误：shell 会把 `'OWN''ER'` 拼回一个词，于是命令
搜的是真实 token，而本页并不包含它。这正是「期望结果恒为一行」的前提 —— 本节的旧
版本有过自我计数的毛病，每编辑一次数字就变。

那唯一剩下的命中是**误匹配，必须保留**：该 buildspec 里有一条讲仓库文件归属的
`# >>>` 分节标记注释，其首个单词的前五个大写字母与旧占位符相同，所以缺词边界的
grep 仍会命中它。它从来就不是占位符。用词边界形式
（`grep -nP '(?<![A-Za-z])OWN''ER(?![A-Za-z])'`）即可排除。

`.omo/` 下的编排笔记不属于交付面，命令中已用 pathspec 排除。

### 机械替换的位置

| 文件 | 位置 |
| --- | --- |
| `README.md` | 4 处徽章 URL、2 条安装一行式 |
| `docs/readme/README.en.md` | 同上，英文镜像 |
| `scripts/install.sh` | `DEFAULT_REPO`、用法一行式、用法默认值文本 |
| `scripts/install.ps1` | `$DefaultRepo`、用法一行式 |
| `docs/installation.md` | 2 条安装一行式 |
| `docs/readme/installation.zh.md` | 同上，中文镜像 |
| `docs/repo-metadata.md` | 4 行 `gh` 命令，现已字面可用 |
| `docs/readme/repo-metadata.zh.md` | 同上，中文镜像 |

### 改写措辞、而非替换的位置

那些**描述**占位符的句子。把 owner 替进去会变成胡话（「把 `sunerpy` 替换为真实的
GitHub owner」），因此它们被改写为陈述真正尚存的限制：URL 是真的，**仓库**才是还
不存在的那一半。位置：两份 README 的 HTML 诚实声明注释、安装小节说明、现状与限制
条目；两份安装文档的引用块；两个安装器的头部注释。

### 删除、而非替换的位置（陷阱本体）

`scripts/install.sh` 与 `scripts/install.ps1` 各有一处守卫：仓库默认值仍以占位符
开头时拒绝运行，并带 `TODO(remote)` 注释说明应整块删除而不是清扫。**对 owner token
一刀切 `sed` 会把它们改坏**：比较式被改写成真实 owner，安装器于是拒绝真实仓库。
这是实测出来的，不是推断 —— 对清扫前的脚本做一刀切替换后得到：

```text
error: AGENTLENS_REPO is still the placeholder owner "sunerpy"
```

两个块均已删除，`grep -c 'TODO(remote)'` 现各报 0。删除后在无任何环境变量覆盖的
情况下，脚本推导出的地址是
`https://github.com/sunerpy/AgentLens/releases/download/v<版本>/...`
（`AGENTLENS_DRY_RUN=1` 会打印解析后的 plan）。记录见
`.omo/evidence/g1g2-license-placeholders.md`。


### 版本占位符：刻意保留

`<version>` / `<版本>` 描述的是**文件名模式**（`AgentLens_<version>_amd64.deb`），
实际值构建期由 `[workspace.package].version` 解析。它们不在等待任何东西。

| 文件 | 占位符 | 数量 |
| --- | --- | --- |
| `docs/installation.md` | `<version>` | 2 |
| `docs/readme/installation.zh.md` | `<版本>` | 2 |
| `Makefile` | `<version>` | 1 |
| `scripts/install.ps1` | `<version>` | 1 |

**6 处，全部刻意保留。** 同理 `sha256sums-<os>.txt` 里的 `<os>`（3 处），以及
用法文本里的 `<owner>` / `<real-owner>` 元变量。

## 仍未被证明的项

远端已经存在，因此上一版这张表里「没有 remote 就无法证明」的多数条目已经闭环。
剩下的都卡在同一件事上：**尚未发布任何 release**。

| 项 | 状态 | 未被证明的部分 |
| --- | --- | --- |
| `gh repo edit --description` | **已应用** | 无。`gh repo view` 可复核 |
| `gh repo edit --add-topic` | **已应用**（9 个） | 无 |
| squash 策略 = PR 标题 | 仅 UI 可改 | 无法脚本化；不设会让 release-please 静默停止升版 |
| Workflow permissions = 读写 | 仅 UI 可改 | 无法脚本化 |
| `.github/workflows/ci.yml` | **已运行，`main` 全绿** | 无 |
| `.github/workflows/pr-title.yml` | **已作为真实 PR 检查运行** | 无 |
| `.github/workflows/release.yml` | actionlint 干净；release-please 已开出 release PR | 从未合并过 release PR，因此从未存在过 tag、draft Release 或已发布资产 |
| Codecov 上传与徽章 | `codecov.yml` 已提交，CI 里只有 Linux 上传 | 面板与徽章需要 `CODECOV_TOKEN`，未确认已配 |
| `scripts/install.sh` 下载路径 | shellcheck 干净，已对本地 `file://` 源实测 | 从未拉取过真实 GitHub Release；尤其是 releases API 路径未对活的 GitHub 验证 |
| `scripts/install.ps1` 下载路径 | 已在真实 Windows 上端到端 38/38 通过 | 仍未对真实 GitHub Release 执行（还没有 release） |
| README 里的安装一行式 | 已写，URL 可取到脚本 | 脚本能跑，但下载不到产物 |
| 版本徽章 | 已写 | 没有 release，渲染为 "no status" |
| `cliff.toml` 的 `[remote]` / issue 链接 | 刻意留空 | owner 已确定，是否补齐属于未决定的元数据取舍 |
| `LICENSE` | **已存在**（MIT，`Copyright (c) 2026 sunerpy`） | 无。已闭环：它本来就不依赖 remote。与 `Cargo.toml`、`frontend/package.json` 的 `license = "MIT"` 一致 |

**本地已证明**的部分：`make lint`、`cargo test --workspace`、
`actionlint .github/workflows/*.yml`、`shellcheck scripts/install.sh`、
pwsh 解析检查，以及记录在 `.omo/evidence/wd-repo-metadata.md` 的安装器行为探针
（校验和拒绝、架构矩阵、非法输入拒绝、重复运行稳定性）。

三个平台都在 AWS CodeBuild 上构建过绿灯，所以安装脚本里的产物名是事实而非猜测：
5,709,438 字节的 `AgentLens_0.1.0_amd64.deb`、4,142,828 字节的
`AgentLens_0.1.0_x64-setup.exe`，以及一个真实的 5,862,574 字节
`AgentLens_0.1.0_aarch64.dmg`。
