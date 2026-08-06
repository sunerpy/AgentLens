# Repository metadata and the pending-remote checklist

[← README](readme/README.en.md) · [简体中文](readme/repo-metadata.zh.md)

This repository **has no git remote**. `git remote -v` prints nothing. Everything
on this page is therefore *drafted and ready to paste*, not applied: no `gh`
command below has been run, and none can be until someone creates the remote.

Creating the remote is a deliberate human decision (which owner or org, public or
private, which name), so this document stops one step short of it.

## Table of Contents

- [Description](#description)
- [Topics](#topics)
- [Paste-ready gh commands](#paste-ready-gh-commands)
- [Owner placeholder sweep: done](#owner-placeholder-sweep-done)
- [Blocked by the missing remote](#blocked-by-the-missing-remote)

## Description

Proposed one-line repository description (117 characters), mirroring the first
paragraph of the README so the framing does not drift:

```text
Desktop dashboard for AI coding-agent token usage across local and remote SSH hosts, backed by a local SQLite archive
```

## Topics

Nine topics, all of which genuinely apply. No aspirational tags: there is no
`electron`, no `openai`, and no `claude` here, because none of those is in the
build. `opencode` is listed because it is the one implemented adapter.

| Topic | Why it applies |
| --- | --- |
| `tauri` | The desktop shell is Tauri 2 (`src-tauri/`) |
| `rust` | Core, collector and askpass crates are Rust |
| `react` | Frontend is React 18.3.1 + Vite + Tailwind |
| `sqlite` | The archive is SQLite and is the product's core claim |
| `desktop-app` | Ships as `.deb` / NSIS / `.dmg`, not a service |
| `ai-agents` | Subject matter: coding-agent usage records |
| `token-usage` | The measured quantity: tokens and derived cost |
| `opencode` | The only implemented source adapter |
| `ssh` | Remote collection is over SSH with a pushed collector |

## Paste-ready gh commands

**None of these have been run**, but they are now literal: the owner is decided
(`sunerpy`), so the commands below are paste-ready as written. `gh` is installed
and authenticated in this workspace (`gh auth status` reports an active account),
so the blocker is purely the absent repository, not credentials.

```bash
# 1. Confirm the remote exists and gh can see it (fails today: no such repo)
gh repo view sunerpy/AgentLens --json name,description,repositoryTopics

# 2. Description
gh repo edit sunerpy/AgentLens \
  --description "Desktop dashboard for AI coding-agent token usage across local and remote SSH hosts, backed by a local SQLite archive"

# 3. Topics (--add-topic is incremental, so this is safe to re-run)
gh repo edit sunerpy/AgentLens \
  --add-topic tauri \
  --add-topic rust \
  --add-topic react \
  --add-topic sqlite \
  --add-topic desktop-app \
  --add-topic ai-agents \
  --add-topic token-usage \
  --add-topic opencode \
  --add-topic ssh

# 4. Verify
gh repo view sunerpy/AgentLens --json description,repositoryTopics,homepageUrl
```

Homepage is deliberately left unset: there is no docs site, and pointing it at
the repository itself adds nothing.

Two repository **settings** also have to be changed by hand in the GitHub UI, and
neither can be scripted from here:

- **Settings → General → Pull Requests → squash commit message = "Pull request
  title"**. Without it, release-please only sees the squash commit subject; a PR
  titled `chore:` silently swallows the `feat:` / `fix:` commits on its branch and
  version bumps stop with no error. `.github/workflows/pr-title.yml` enforces the
  title format, but the squash policy itself is a UI setting.
- **Settings → Actions → General → Workflow permissions = read and write**. The
  release jobs declare `permissions: contents: write` per job, but a repository
  locked to read-only overrides that.

## Owner placeholder sweep: done

The owner placeholder is gone. `sunerpy` is substituted at every URL, script
default and one-liner, and the two installer guard blocks that existed only
because no repository was reachable have been deleted outright.

Counted, not remembered. This is the audit command; it should now report exactly
one line, from `.aws/buildspec/macos.yml`:

```bash
git ls-files -z -- ':!.omo' | xargs -0 grep -n 'OWN''ER'
```

The split-and-rejoined literal is deliberate, not a typo: the shell concatenates
`'OWN''ER'` back into one word, so the command searches for the real token while
this page does not contain it. That is what keeps the expected result at exactly
one line instead of drifting every time this page is edited -- the
self-counting problem the earlier version of this section had.

That single remaining hit is a **false positive, and must stay**: the buildspec
carries a `# >>>` section-marker comment about repository-file ownership whose
first word begins with the same five uppercase letters as the old placeholder, so
a boundary-less grep still matches it. It was never a placeholder. Use the
word-boundary form (`grep -nP '(?<![A-Za-z])OWN''ER(?![A-Za-z])'`) to exclude it.

Orchestration notes under `.omo/` are not part of the shipped surface, which is
why the pathspec excludes them.

### What was substituted (mechanical)

| File | Sites |
| --- | --- |
| `README.md` | 4 badge URLs, 2 installer one-liners |
| `docs/readme/README.en.md` | Same, English mirror |
| `scripts/install.sh` | `DEFAULT_REPO`, usage one-liner, usage default text |
| `scripts/install.ps1` | `$DefaultRepo`, usage one-liner |
| `docs/installation.md` | 2 installer one-liners |
| `docs/readme/installation.zh.md` | Same, Chinese mirror |
| `docs/repo-metadata.md` | 4 `gh` command lines, now literal |
| `docs/readme/repo-metadata.zh.md` | Same, Chinese mirror |

### What was reworded, not substituted

Sentences that *described* the placeholder. Substituting an owner into them
produces nonsense ("Replace `sunerpy` with the real GitHub owner"), so they were
rewritten to state the surviving limitation instead: the URLs are real, the
**repository** is what does not exist yet. Sites: the HTML honesty comment, the
Install note and the Status bullet in each README; the blockquote in each
installation page; the header comment in each installer.

### What was deleted, not substituted (the trap)

`scripts/install.sh` and `scripts/install.ps1` each carried a guard that refused
to run while the repo default still started with the placeholder, plus a
`TODO(remote)` comment saying to delete the block rather than sweep it. **A
blanket `sed` over the owner token breaks these**: it rewrites the comparison to
the real owner, so the installer refuses the real repository. Verified, not
assumed -- a blanket substitution applied to the pre-sweep script produced:

```text
error: AGENTLENS_REPO is still the placeholder owner "sunerpy"
```

Both blocks were removed; `grep -c 'TODO(remote)'` now reports 0 in each. With
them gone and no environment overrides set, the script derives
`https://github.com/sunerpy/AgentLens/releases/download/v<version>/...`
(`AGENTLENS_DRY_RUN=1` prints the resolved plan). Recorded in
`.omo/evidence/g1g2-license-placeholders.md`.


### Version placeholders: intentional, keep them

`<version>` / `<版本>` document a *filename pattern*
(`AgentLens_<version>_amd64.deb`), resolved at build time from
`[workspace.package].version`. They are not waiting on anything.

| File | Token | Count |
| --- | --- | --- |
| `docs/installation.md` | `<version>` | 2 |
| `docs/readme/installation.zh.md` | `<版本>` | 2 |
| `Makefile` | `<version>` | 1 |
| `scripts/install.ps1` | `<version>` | 1 |

**6 occurrences, all deliberate.** Likewise `<os>` in `sha256sums-<os>.txt`
(3 occurrences) and the `<owner>` / `<real-owner>` metavariables in usage text.

## Blocked by the missing remote

Authored, locally verified, and **never executed**. Nothing here is a defect; it
is all work that cannot be proven without a remote.

| Item | State | What is unproven |
| --- | --- | --- |
| `gh repo edit --description` | drafted above | Never run. No repository to edit |
| `gh repo edit --add-topic` | drafted above | Never run |
| Squash policy = PR title | UI-only | Cannot be scripted; release-please bumps silently stop without it |
| Workflow permissions = read/write | UI-only | Cannot be scripted |
| `.github/workflows/ci.yml` | actionlint clean | Has never run on GitHub Actions. The CI badge renders as "no status" |
| `.github/workflows/release.yml` | actionlint clean | Never run. release-please has never opened a release PR; no tag, no draft Release, no published asset has ever existed |
| `.github/workflows/pr-title.yml` | actionlint clean, logic replayed locally | Never run as a real PR check |
| Codecov upload + badge | `codecov.yml` committed | No upload has ever happened; `CODECOV_TOKEN` is not set anywhere |
| `scripts/install.sh` download path | shellcheck clean, exercised against a local `file://` source | Has never fetched a real GitHub release. The releases API path in particular is untested against live GitHub |
| `scripts/install.ps1` download path | pwsh parse-clean, exercised against a local HTTP source from Linux | Never run on Windows; the NSIS launch + exit-code branch has never executed against a real installer |
| Install one-liners in the README | written | The `raw.githubusercontent.com` URLs 404 until the remote exists |
| Badge URLs (CI, codecov) | written | 404 until the remote exists |
| `cliff.toml` `[remote]` / issue links | deliberately absent | Cannot be filled without an owner; adding it now would produce 404 links in every changelog entry |
| `LICENSE` | **present** (MIT, `Copyright (c) 2026 sunerpy`) | Nothing. Closed: it never depended on the remote. Matches `license = "MIT"` in `Cargo.toml` and `frontend/package.json` |

What *is* proven locally: `make lint`, `cargo test --workspace`,
`actionlint .github/workflows/*.yml`, `shellcheck scripts/install.sh`, the pwsh
parser check, and the installer behaviour probes recorded in
`.omo/evidence/wd-repo-metadata.md` (checksum rejection, architecture matrix,
malformed-input rejection, repeat-run stability).

All three platforms have been built green on AWS CodeBuild, which is why the
artifact names in the installers are facts rather than guesses:
a 5,709,438-byte `AgentLens_0.1.0_amd64.deb`, a 4,142,828-byte
`AgentLens_0.1.0_x64-setup.exe`, and a real 5,862,574-byte
`AgentLens_0.1.0_aarch64.dmg`.
