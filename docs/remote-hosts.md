# Adding remote hosts

[← README](../README.md) · [简体中文](readme/remote-hosts.zh.md)

The local machine self-registers the first time you open Host Management, so it
needs no configuration. Add remote hosts in this order.

1. Open Host Management, then Add SSH Host.
2. Fill in the **display name** and the **ssh alias or hostname**. Leave username
   and key path empty to inherit `~/.ssh/config`. Leave the remote data directory
   empty for auto-discovery through `XDG_DATA_HOME`, falling back to
   `~/.local/share/opencode`.
3. Press **Test Connection**. This opens a real SSH connection and echoes back the
   remote architecture, data directory, free space and the machine-id source. On
   failure it returns actionable guidance.
4. Copy the remote machine-id hash reported by Test Connection (64 hex
   characters) into its field. This is what stops one machine being added twice
   and double-counting its usage.
5. If a password or key passphrase is needed, enter it in the credentials
   section. **Passwords go only into the OS keychain** (Linux Secret Service,
   Windows Credential Manager). They never land in a config file and are never
   returned to the UI over IPC.
6. Press Add Host. SSH hosts default to **manual refresh**; the local machine
   defaults to automatic with a 5-minute minimum interval. Press refresh on the
   host card to collect.

## What a refresh actually does

The app reads the remote `uname -m`, picks the matching collector architecture,
`scp`s it to `~/.cache/agentlens/run.XXXXXX` on the remote, verifies its sha256,
executes it in place, and cleans up on exit. The remote side runs a read-only
scan and never modifies the remote tool's data.

## Related

- [Remote Source API v1](remote-source-api.md) for wiring a remote service as a
  `RemoteService` instead of an SSH host
- [Architecture](architecture.md)
