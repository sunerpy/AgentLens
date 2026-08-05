# changelog/

变更日志按**主版本**分文件存放，而不是堆在一份无限增长的 `CHANGELOG.md` 里：

| 文件 | 覆盖范围 |
| --- | --- |
| `CHANGELOG-v0.md` | `0.x` 全部发布 |

下一个主版本落地时新增 `CHANGELOG-v1.md`，并把
`release-please-config.json` 里的 `changelog-path` 指向它。旧文件原样保留，
不迁移、不重写。

## 谁写这些文件

- **release-please 持有 `CHANGELOG-v*.md`。** 每次它开/更新发布 PR 时追加条目。
  人不要手改这些文件：下一次生成会覆盖，PR 里会出现无意义的冲突 diff。
  `.oxfmtignore` 已排除 `changelog/`，避免 oxfmt 重排它持有的字节。
- **git-cliff 不写仓库文件。** 它只在 `.github/workflows/release.yml` 的
  `publish` 阶段用 `git cliff --latest --strip all` 生成 **GitHub Release 正文**。
  两个工具的分组顺序在 `cliff.toml` 的 `commit_parsers` 与
  `release-please-config.json` 的 `changelog-sections` 里保持一致。

## 版本号的单一事实源

`[workspace.package].version`（根 `Cargo.toml`）是唯一事实源，`make dist-version`
回显它。release-please 通过**显式 `extra-files`** 写它，以及唯一的另一份字面量
`frontend/package.json`。

四个成员 crate（`agentlens-core` / `agentlens-collector` / `agentlens-askpass` /
`agentlens-tauri`）都写 `version.workspace = true`，没有字面量可改；
`src-tauri/tauri.conf.json` 刻意不声明 `version`，Tauri 回落到
`src-tauri/Cargo.toml` 的包版本，即 workspace 版本。所以它们全都**不需要**接线。

`release-type` 选 `simple` 而不是 `rust`，这是被源码证实的必要选择：
release-please 的 `CargoToml` updater 在清单里找不到 `[package]` 段时会
**直接抛错**（`is not a package manifest (might be a cargo workspace)`），
而本仓库根 `Cargo.toml` 正是只有 `[workspace]` 的虚拟清单。`simple` 策略对它默认
的 `version.txt` 用的是 `createIfMissing: false`，仓库里没有这个文件就不会被凭空
创建，于是不会多出第二份版本字面量。

## 已知缺口：Cargo.lock

`Cargo.lock` 里有四条 `agentlens-*` 的 `version = "0.1.0"`，release-please
**不接线**它 —— 没有任何 updater 能在不调用 cargo 的前提下安全重写 lockfile。
当前所有构建都不带 `--locked`，首次 cargo 调用会自行刷新，因此不会失败。
**若将来给 workspace 构建加上 `--locked`，必须同时在发布流程里补一步
`cargo update -w`**，否则 tag 上的 lockfile 会与 `Cargo.toml` 不一致而硬失败。
