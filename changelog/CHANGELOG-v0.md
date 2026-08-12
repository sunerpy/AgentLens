# Changelog

## [0.0.5](https://github.com/sunerpy/AgentLens/compare/v0.0.4...v0.0.5) (2026-08-12)


### Features

* **updater:** 增加签名自动更新能力 ([#25](https://github.com/sunerpy/AgentLens/issues/25)) ([b72ad36](https://github.com/sunerpy/AgentLens/commit/b72ad36cb234ccf7ed10744bb0b3f9218c867b72))

## [0.0.4](https://github.com/sunerpy/AgentLens/compare/v0.0.3...v0.0.4) (2026-08-12)


### Features

* **windows:** 增加 MSI 安装包并让校验和覆盖双格式 ([#23](https://github.com/sunerpy/AgentLens/issues/23)) ([39f95cf](https://github.com/sunerpy/AgentLens/commit/39f95cfec84cf9f0398a547f82a4320a1d2559db))

## [0.0.3](https://github.com/sunerpy/AgentLens/compare/v0.0.2...v0.0.3) (2026-08-11)


### Bug Fixes

* **tray:** 支持左键单击托盘图标打开主面板 ([#21](https://github.com/sunerpy/AgentLens/issues/21)) ([b9fff30](https://github.com/sunerpy/AgentLens/commit/b9fff303bff75ef25a24417a6f3ce4128e5ad5f8))


### Documentation

* **readme:** 重写语感并同步 Cargo.lock 到 0.0.2 版本号 ([#19](https://github.com/sunerpy/AgentLens/issues/19)) ([c6d5ad9](https://github.com/sunerpy/AgentLens/commit/c6d5ad9a9d3540b74535a78010c76fb4d3b97232))


### Build System

* **release-please:** 把 Cargo.lock 纳入发版版本号同步 ([#22](https://github.com/sunerpy/AgentLens/issues/22)) ([fd028fe](https://github.com/sunerpy/AgentLens/commit/fd028fe1432ffee6188e2424e6f19a022e7a9b53))

## [0.0.2](https://github.com/sunerpy/AgentLens/compare/v0.0.1...v0.0.2) (2026-08-11)


### Bug Fixes

* **overview:** 成本卡术语正名，默认只显示本地估算 ([9a1c3ce](https://github.com/sunerpy/AgentLens/commit/9a1c3ce45c1af608a0c6c50ee9ee2d7da16925ed))


### Documentation

* 换用用户提供的总览截图，同步成本卡描述 ([ddbfe95](https://github.com/sunerpy/AgentLens/commit/ddbfe95eeb9704d0a98a816e2d307d2459b3555d))


### CI

* setup-zig 关闭缓存，它会从 cancelled run 保存残缺产物 ([3b362a8](https://github.com/sunerpy/AgentLens/commit/3b362a8f52ee8c559f9e55124bae18963cb10717))

## 0.0.1 (2026-08-10)


### Features

* AgentLens —— AI 编码工具用量看板 ([a53cbec](https://github.com/sunerpy/AgentLens/commit/a53cbec1148cd983607fa6c556517098a3d85be1))


### Bug Fixes

* **overview:** 进行中的时间桶不再当成覆盖缺口报出 ([0c0ced6](https://github.com/sunerpy/AgentLens/commit/0c0ced6463860016ed2f84ffec7392c05d88315c))


### Performance

* **hosts:** 主机行 memo 化消除刷新卡顿；成本卡改分层呈现 ([e4902f5](https://github.com/sunerpy/AgentLens/commit/e4902f5a2c7716bfb424f07f0342dd0aba4bcf9d))


### Documentation

* README 加界面截图 ([38aac5f](https://github.com/sunerpy/AgentLens/commit/38aac5f5b410c2a1d385c7fc84185921d2743561))
* 首段点名四个采集源，并说明默认只启用 OpenCode ([a38dfe7](https://github.com/sunerpy/AgentLens/commit/a38dfe78f3c5fbea43c1c1ac74bd56a08bcfbaf1))


### Build System

* 加 Windows 交叉编译发版目标，附 sidecar 护栏 ([89de3ef](https://github.com/sunerpy/AgentLens/commit/89de3efe958c0427d9ae86f43f33fb97397b1345))


### CI

* tauri-cli 与 zigbuild 改预编译分发，加 Rust 编译缓存 ([16da4c7](https://github.com/sunerpy/AgentLens/commit/16da4c7d30a502aabc6348addf4e40dfcc3a0cd4))

## Changelog

## Changelog (0.x)

`0.x` 全部发布的变更日志。由 release-please 维护，请勿手改（见
[`changelog/README.md`](README.md)）。

`0.1.0` 是接入发布自动化之前的基线版本，从未打过 `v0.1.0` tag，也没有对应的
GitHub Release，所以这里刻意没有 `0.1.0` 条目 —— 不为没发布过的版本编造记录。
`.release-please-manifest.json` 以 `0.1.0` 作为起点播种，下一次带 `feat` 的
发布 PR 会给出 `0.2.0`，条目从那时起追加在本文件顶部。
