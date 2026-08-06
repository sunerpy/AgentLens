# 添加远端主机

[← README](../../README.md) · [English](../remote-hosts.md)

本机会在首次打开「主机管理」时自动注册，无需配置。远端主机按下面顺序添加。

1. 打开「主机管理」→「添加 SSH 主机」。
2. 填写**显示名称**与**ssh 别名 / 主机名**。用户名、密钥路径留空即沿用 `~/.ssh/config`；
   远端数据目录留空即按 `XDG_DATA_HOME` → `~/.local/share/opencode` 自动发现。
3. 点「测试连接」。这一步会真实建立 SSH 连接，回显远端架构、数据目录、可用空间与
   machine-id 来源；失败时给出中文处理建议。
4. 把「测试连接」读出的远端机器标识哈希（64 位十六进制）填入对应字段。它用于保证
   同一台机器不会被重复添加导致用量双计。
5. 如需口令或密钥 passphrase，在凭据区填写。**口令只写入操作系统钥匙串**
   （Linux Secret Service / Windows 凭据管理器），不落任何配置文件，
   也不会经 IPC 回传给界面。
6. 点「添加主机」。SSH 主机默认是**手动刷新**（本机默认自动，间隔下限 5 分钟），
   在主机卡上点刷新即可采集。

## 一次刷新实际做了什么

应用按远端 `uname -m` 选择对应架构的采集器，`scp` 到远端
`~/.cache/agentlens/run.XXXXXX`，校验 sha256 后就地执行，读完即随退出清理。
远端只运行只读扫描，不会修改远端的工具数据。

## 相关

- [Remote Source API v1](../remote-source-api.md)：以 `RemoteService` 形态接入远程服务，
  而不是走 SSH 主机
- [架构](architecture.zh.md)
