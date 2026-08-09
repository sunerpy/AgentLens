# Adding remote hosts

[← README](readme/README.en.md) · [简体中文](readme/remote-hosts.zh.md)

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
4. The machine-id hash (64 hex characters) is **filled in from the probe result
   and turns read-only** — nothing to retype. The probe is the field's only
   writer: the remote computes the SHA-256 itself and the UI only constrains it to
   64 lowercase hex characters. This is what stops one machine being added twice
   and double-counting its usage. Editing `host` or `user` invalidates and clears
   the hash, so the probe has to be re-run.
5. If a password or key passphrase is needed, enter it in the credentials
   section. **Passwords go only into the OS keychain** (Linux Secret Service,
   Windows Credential Manager). They never land in a config file and are never
   returned to the UI over IPC.
6. Press Add Host.
7. **Tick the sources to collect on the host card.** Enabling lives on the host
   card, not in Settings; `hosts.enabled_sources` defaults to `'opencode'`, so
   Claude Code, Codex and Hermes each have to be ticked.
8. Press refresh on the host card to collect. Both the local machine and remotes
   can be switched to automatic, each with its own interval
   (`refresh.localIntervalMs` / `refresh.remoteIntervalMs`) and the same 600-second
   floor — a configured value below the floor is clamped back to it on read and
   never takes effect.

## What a refresh actually does

The app reads the remote `uname -m`, picks the matching collector architecture,
`scp`s it to `~/.cache/agentlens/run.XXXXXX` on the remote, verifies its sha256,
executes it in place, and cleans up on exit. The remote side runs a read-only
scan and never modifies the remote tool's data.

## Related

- [Remote Source API v1](remote-source-api.md) for wiring a remote service as a
  `RemoteService` instead of an SSH host
- [Architecture](architecture.md)
