# Changelog

## [0.2.0](https://github.com/sunerpy/AgentLens/compare/v0.1.0...v0.2.0) (2026-08-07)


### Features

* AgentLens —— AI 编码代理 token 用量的归档与可视化桌面应用 ([ea8a9e8](https://github.com/sunerpy/AgentLens/commit/ea8a9e8cf8294c7a0403c55b05482c7c05a331a3))
* **ipc:** 刷新进度改用 ipc::Channel 流式推送，去掉状态轮询 ([#8](https://github.com/sunerpy/AgentLens/issues/8)) ([3d34857](https://github.com/sunerpy/AgentLens/commit/3d348576c7755df85a16e51993ab3b3c4385693c))
* **quality:** 覆盖率门槛提到 90 并把中文 README 设为默认 ([#3](https://github.com/sunerpy/AgentLens/issues/3)) ([a06be7f](https://github.com/sunerpy/AgentLens/commit/a06be7f46dffe3827c4e800c27540c79e313c5f8))
* **ui:** 六套主题 + 趋势图分组 + 日志查看与 GitHub 反馈入口 ([#10](https://github.com/sunerpy/AgentLens/issues/10)) ([21663c2](https://github.com/sunerpy/AgentLens/commit/21663c26eb3d9519aad4914b075c645beb55b261))


### Bug Fixes

* **hosts:** 修复主机页卡顿与手填机器标识，并把「下钻」改名为「用量分析」 ([#4](https://github.com/sunerpy/AgentLens/issues/4)) ([de7b8dd](https://github.com/sunerpy/AgentLens/commit/de7b8dd68b0e57808cfe1516e79150a7b9afefc0))
* **hosts:** 机器标识字段跨满两栏，窄窗口下 64 位摘要不再被裁 ([#7](https://github.com/sunerpy/AgentLens/issues/7)) ([f64400c](https://github.com/sunerpy/AgentLens/commit/f64400cdaad2b24fc70ee5f1a99cc95202d13f3f))
* **portability:** 修复 Windows 上 SSH 超时测试与 bindings 导出的两处环境依赖 ([#1](https://github.com/sunerpy/AgentLens/issues/1)) ([41e1054](https://github.com/sunerpy/AgentLens/commit/41e1054208f9c3ad8e967236971c91492006654c))
* **shell:** 消除 cmd 闪窗、接入 opener、禁用右键，并把模型成本改为下拉选择 ([#9](https://github.com/sunerpy/AgentLens/issues/9)) ([1002496](https://github.com/sunerpy/AgentLens/commit/1002496349b6182b42bf864606223239dbbf0c91))


### Refactoring

* **qa:** QA 脚本的安装包名改为从单一版本来源派生 ([#6](https://github.com/sunerpy/AgentLens/issues/6)) ([1eec925](https://github.com/sunerpy/AgentLens/commit/1eec9259ecd43d004fdfb67d8aed69868fa37e23))


### Documentation

* **readme:** 版本号不再硬编码，安装命令与 badge 随发布自动跟随 ([#5](https://github.com/sunerpy/AgentLens/issues/5)) ([c2b1e8b](https://github.com/sunerpy/AgentLens/commit/c2b1e8bc1ec9aba06bf58b3b2584a0af695c3ca0))

## Changelog (0.x)

`0.x` 全部发布的变更日志。由 release-please 维护，请勿手改（见
[`changelog/README.md`](README.md)）。

`0.1.0` 是接入发布自动化之前的基线版本，从未打过 `v0.1.0` tag，也没有对应的
GitHub Release，所以这里刻意没有 `0.1.0` 条目 —— 不为没发布过的版本编造记录。
`.release-please-manifest.json` 以 `0.1.0` 作为起点播种，下一次带 `feat` 的
发布 PR 会给出 `0.2.0`，条目从那时起追加在本文件顶部。
