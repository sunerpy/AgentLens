# 数据存放与设置

[← README](README.zh.md) · [English](../data-storage.md)

| 内容 | Linux | Windows |
| --- | --- | --- |
| 归档库 | `~/.local/share/agentlens/archive.db` | `%APPDATA%\agentlens\archive.db` |
| 价格覆盖表 | `~/.local/share/agentlens/prices.json` | `%APPDATA%\agentlens\prices.json` |
| 主机口令 / passphrase | 系统钥匙串（Secret Service） | Windows 凭据管理器 |

- 精确路径以「设置」页的「归档库位置」为准：桌面壳启动时会把它写入 `app_settings`
  的 `archive.path` 键，界面只读展示。
- **归档库是权威历史，永不裁剪**：源库轮转、备份被删或远端数据目录被清空，都不会导致
  已归档的记录消失；迁移前会自动生成一次 `archive.db.backup-<时间戳>.db` 兄弟备份。
- 所有设置（时区、周起始日、刷新间隔、价格表）都只存在归档库的 `app_settings` 表与
  `prices.json` 里，没有第二处配置文件。
- 卸载不会删除上表内容；需要彻底清理时手动删除对应目录与钥匙串条目。

## 相关

- [安装](installation.zh.md)
- [架构](architecture.zh.md)
