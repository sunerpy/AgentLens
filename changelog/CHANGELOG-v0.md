# Changelog

## [0.0.2](https://github.com/sunerpy/AgentLens/compare/v0.0.1...v0.0.2) (2026-08-10)


### Bug Fixes

* **overview:** 进行中的时间桶不再当成覆盖缺口报出 ([0c0ced6](https://github.com/sunerpy/AgentLens/commit/0c0ced6463860016ed2f84ffec7392c05d88315c))


### Performance

* **hosts:** 主机行 memo 化消除刷新卡顿；成本卡改分层呈现 ([e4902f5](https://github.com/sunerpy/AgentLens/commit/e4902f5a2c7716bfb424f07f0342dd0aba4bcf9d))


### Documentation

* 首段点名四个采集源，并说明默认只启用 OpenCode ([a38dfe7](https://github.com/sunerpy/AgentLens/commit/a38dfe78f3c5fbea43c1c1ac74bd56a08bcfbaf1))


### Build System

* 加 Windows 交叉编译发版目标，附 sidecar 护栏 ([89de3ef](https://github.com/sunerpy/AgentLens/commit/89de3efe958c0427d9ae86f43f33fb97397b1345))

## 0.0.1 (2026-08-09)


### Features

* AgentLens —— AI 编码工具用量看板 ([a53cbec](https://github.com/sunerpy/AgentLens/commit/a53cbec1148cd983607fa6c556517098a3d85be1))

## Changelog (0.x)

`0.x` 全部发布的变更日志。由 release-please 维护，请勿手改（见
[`changelog/README.md`](README.md)）。

`0.1.0` 是接入发布自动化之前的基线版本，从未打过 `v0.1.0` tag，也没有对应的
GitHub Release，所以这里刻意没有 `0.1.0` 条目 —— 不为没发布过的版本编造记录。
`.release-please-manifest.json` 以 `0.1.0` 作为起点播种，下一次带 `feat` 的
发布 PR 会给出 `0.2.0`，条目从那时起追加在本文件顶部。
