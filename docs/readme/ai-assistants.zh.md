# 给 AI 编码助手

在本仓库里改代码前，按顺序读这三份，能省掉大部分返工：

1. [四个源的计量口径](../measurement.md) —— 四个源「一条记录代表什么」的差异。
   动任何聚合口径之前必读，否则很容易把消息级与会话级的计数相加。
2. [架构](architecture.zh.md) —— 归档表、定价解析、IPC 边界与窗口装饰的取舍。
3. [开发与构建](development.zh.md) —— 全部 `make` 目标与验证路径。

## 三条硬约束

- **时间处理只在 Rust 侧。** 不要给 `frontend/` 引入 `date-fns` / `dayjs` / `moment`；
  第二套时区实现就是缺陷发生器。
- **TypeScript 契约是生成物。** 不要手改 `frontend/src/generated/`；改完 Rust 侧 DTO 后跑
  `cargo test -p agentlens-tauri --features ts-export bindings_export` 重新导出。
  CI 有一道零漂移门禁，生成物与 Rust 类型不一致就红。
- **交付前跑 `make lint` 与 `make test`。** 覆盖率地板 90%，`make coverage-gate` 会拦。

## 容易踩的几处

- 默认只采集 OpenCode。`hosts.enabled_sources` 的列默认值是 `'opencode'`，
  「装好了却没数据」通常是主机卡片上那三个源没勾，而不是解析出错。
- Hermes 是会话级源。它的用量落在 `session_record_count`，不进 `message_count`，
  把这两个数相加是错的。
- 仓库内没有真实 fixture。自动化测试全部基于合成数据，文档里的实测数字无法在 CI 里复现。
- Linux 与 macOS 的安装包仍未在真机上启动过，只有 Windows 做过真机验收，
  见[开发与构建](development.zh.md)。
