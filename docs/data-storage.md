# Data storage and settings

[← README](../README.md) · [简体中文](readme/data-storage.zh.md)

| Content | Linux | Windows |
| --- | --- | --- |
| Archive database | `~/.local/share/agentlens/archive.db` | `%APPDATA%\agentlens\archive.db` |
| Price overrides | `~/.local/share/agentlens/prices.json` | `%APPDATA%\agentlens\prices.json` |
| Host password / passphrase | OS keychain (Secret Service) | Windows Credential Manager |

- The authoritative path is the one shown under Archive Location in Settings. On
  startup the desktop shell writes it to the `archive.path` key in
  `app_settings`; the UI displays it read-only.
- **The archive is authoritative history and is never pruned.** Source-database
  rotation, a deleted backup or a wiped remote data directory cannot remove an
  already-archived record. Before a migration the app writes a sibling backup
  named `archive.db.backup-<timestamp>.db`.
- Every setting (timezone, first day of week, refresh interval, price table)
  lives only in the `app_settings` table of the archive and in `prices.json`.
  There is no second config file.
- Uninstalling does not delete anything in the table above. To wipe fully, remove
  those directories and the keychain entries by hand.

## Related

- [Installation](installation.md)
- [Architecture](architecture.md)
