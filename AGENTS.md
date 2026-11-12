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

This layout describes the `flutter-next` product, not the whole independent
XenTerm Next repository. The owner confirmed dual GPUI/Flutter maintenance on
2026-11-02: `main` retains GPUI and `flutter-next` maintains Flutter. Do not remove
GPUI or replay upstream's history replacement into either maintained branch.
Flutter must cover GPUI features and Next enhancements and pass installation,
upgrade, uninstall, configuration compatibility and actual-use acceptance before
replacing GPUI. Partial tests or successful builds are not replacement acceptance.
See the independent-repository scope in the contribution guide and [#17](https://github.com/xue-ding-e/XenTerm-Next/issues/17).

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
- PRs: select the correct product base (`flutter-next` here; GPUI uses `main`) →
  topic branch → Draft → convert to Ready for review yourself
  when the work is done; maintainers merge with squash or rebase. Follow the
  contribution guide for the full flow.

If another tool needs a rules entry point, link here instead of copying policy
text. Keep both contribution guide languages aligned when policy changes.
