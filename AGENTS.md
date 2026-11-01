# Repository instructions for AI contributors

Read [CONTRIBUTING.zh-CN.md](CONTRIBUTING.zh-CN.md) (or the
[English version](CONTRIBUTING.md)) before editing or submitting work. That guide
is the single source for collaboration, verification and review rules; this file
is only an entry point and does not define a second policy.

## Current architecture

- `crates/xenterm/` — Rust service core: SSH/SFTP, terminal engine, Telnet,
  serial, sessions/config, CLI, MCP, resource monitoring.
- `apps/xenterm/` — Flutter desktop UI.
- `apps/xenterm/native/` — `xenterm-bridge`, the FFI bridge crate between the
  Flutter UI and the Rust core.
- `docs/`, `packaging/`, `scripts/` — design notes, packaging recipes and
  release helpers.

The old GPUI UI no longer exists; do not reintroduce GPUI code, paths or
documentation. The maintainers rebuilt the repository history in October 2026
because both the UI and the Rust core were rewritten. Contributors do not need
to migrate or replay old commits: start from `main` and submit normal changes.
Never rewrite shared history as part of an ordinary change.

## Before you write

- Verify the checkout, remotes, current branch, target repository and PR
  base/head. An issue's source repository does not by itself grant permission to
  modify it. Resolve conflicting targets first; leave unrelated worktrees alone.
- Run the checks that match the change (from the repository root):
  - `cargo check --locked`
  - `cargo test --locked`
  - `cd apps/xenterm && flutter pub get && flutter analyze && flutter test`
  - service-only core: `cargo test --locked --no-default-features --features headless`
- Chinese-first communication; English is welcome. Commit messages use Chinese
  conventional commits, e.g. `fix(ssh): 修正跳板链断开后未回收会话`.
- Keep the diff focused, report verification honestly (passed / failed / not run
  / blocked by environment), and never expose credentials or real session data.
- PRs: fork → branch from `main` → Draft → convert to Ready for review yourself
  when the work is done; maintainers merge with squash or rebase. Follow the
  contribution guide for the full flow.

If another tool needs a rules entry point, link here instead of copying policy
text. Keep both contribution guide languages aligned when policy changes.
