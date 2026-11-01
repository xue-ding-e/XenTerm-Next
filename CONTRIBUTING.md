<p align="center">
  <b>English</b> · <a href="CONTRIBUTING.zh-CN.md">简体中文</a>
</p>

# Contributing to XenTerm

Thanks for helping improve XenTerm. This guide covers problem reports, pull
requests, verification and review standards; it is the source of truth for
collaboration rules, while [AGENTS.md](AGENTS.md) is only an AI entry point.

## Project layout

XenTerm is a Rust core with a Flutter desktop UI:

| Path | Contents |
| --- | --- |
| `crates/xenterm/` | Rust service core: SSH/SFTP, terminal engine, Telnet, serial, sessions/config, CLI, MCP, resource monitoring |
| `apps/xenterm/` | Flutter desktop application (UI) |
| `apps/xenterm/native/` | `xenterm-bridge`, the FFI bridge between UI and core |
| `packaging/`, `scripts/`, `docs/` | Packaging recipes, build/release helpers, design notes |

The UI and the Rust core were both rewritten, so the maintainers rebuilt the
repository history once in October 2026. Contributors do not need to migrate or
replay old commits: start from current `main` and submit normal changes. Do not
rewrite shared history unless you are a maintainer coordinating it.

Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## Communication language

- **Chinese first.** Issue and PR titles, descriptions and discussion are mainly
  Chinese so most community members can take part.
- English is welcome; maintainers reply in Chinese where they can.
- Keep identifiers, commands, raw error output and necessary quotations as-is.
- Community channels are listed in the README; decisions that affect code belong
  in an issue or PR.

## Questions, bug reports and security

- Use GitHub Discussions for usage questions and open-ended ideas, not the issue
  tracker.
- Search existing issues and PRs first; extend an existing discussion and link
  it when similar.
- A bug report includes the XenTerm version, OS, install method (MSI, portable
  archive, package manager or source build), reproduction steps, expected and
  actual result.
- Remove passwords, private keys, tokens and real session data from logs,
  screenshots and examples before posting.
- **Security issues must not be public issues.** Follow [SECURITY.md](SECURITY.md)
  and use GitHub private vulnerability reporting.

## Pull requests

1. Fork and branch from current `main` (`fix/...`, `feat/...`, `docs/...`).
2. Keep the scope focused; batch repeated fixes instead of many tiny PRs and do
   not force unrelated changes together. When splitting, state boundaries and
   dependencies so each part can be reviewed.
3. Keep the PR mergeable: rebase before review, resolve conflicts and re-check
   affected code and tests; handle new conflicts promptly. Do not bypass branch
   protection or overwrite others' commits.
4. Keep the PR as a **Draft** until implementation and verification are done,
   then check the diff, fill in the template and convert **Draft → Ready for
   review** yourself. A "done" comment triggers nothing, and older green checks
   do not prove the current commit. If no feedback appears, report the PR state
   and what you tried instead of guessing bot names or toggling drafts.
5. Maintainers merge reviewed topics with **Squash and merge**, or **Rebase and
   merge** when several complete commits should be kept. A fork is not a reason
   for a merge commit. After merging, start the next task from updated `main`.
6. Link related PRs and state their order; coordinate before rewriting a shared
   branch.

## Commit messages

Use Chinese [Conventional Commits](https://www.conventionalcommits.org/) as
`<type>(<scope>): <summary>`.

- Types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`,
  `chore`, `revert`.
- Scopes follow the area: `ssh`, `sftp`, `terminal`, `telnet`, `serial`,
  `config`, `cli`, `mcp`, `bridge`, `ui`, `deps`, `ci`.
- Summary in Chinese, concise and imperative, no trailing period, e.g.
  `fix(ssh): 修正跳板链断开后未回收会话`.
- One topic per commit; explain cause, trade-offs and impact in the body when
  needed. Squash fixup commits before review.
- Do not bump the version or create tags in a PR; maintainers do that through the
  release helper.

## Verification

Run the checks that match your change and record what actually ran:

```sh
cargo check --locked
cargo test --locked
cd apps/xenterm && flutter pub get && flutter analyze && flutter test
```

- Keep `--locked` for Cargo: `Cargo.lock` is tracked so releases are
  reproducible. If it fails because manifest and lockfile disagree, fix the
  dependency change instead of dropping the flag and calling a re-resolved build
  "verified".
- Service-only core: `cargo test --locked --no-default-features --features headless`;
  passing headless does not prove the Flutter UI or platform APIs work.
- Flutter-only changes need `flutter analyze` and `flutter test`; bridge changes
  need both halves.
- Windows packaging or install/upgrade changes: build the MSI or portable archive
  and verify install, upgrade and uninstall on Windows. State platform gaps
  instead of claiming "passed on all platforms".
- State the exact commands, OS and commit SHA, and distinguish "passed",
  "failed", "not run" and "blocked by environment". Manual checks list steps,
  expected and actual behavior; UI changes include screenshots or a recording.
- New behavior needs tests; bug fixes should show the same test failing before
  and passing after. A filtered run that executes zero tests is not coverage.
- Docs-only changes: check links, paths, examples and wording against current
  code; no code-check claim is needed.
- AI-generated output, unrun commands or screenshots from old commits are not
  evidence. Re-run affected checks after conflict resolution or further edits.

## Dependencies and APIs

- Work against the current `Cargo.toml`, `Cargo.lock`,
  `apps/xenterm/pubspec.yaml` and the locked source, not old branches, tutorials
  or model memory.
- `Cargo.lock` and `apps/xenterm/pubspec.lock` are tracked intentionally; do not
  propose ignoring them.
- Ordinary fixes keep locks stable. Dependency upgrades belong in a separate PR
  (Dependabot opens them) with reason and impact; do not smuggle unrelated
  upgrades into a feature or fix.
- New crates/packages need a justification: maintenance, license compatibility
  and build/CI cost; check MSRV and the Flutter SDK constraint first.
- Protocol, config format and FFI changes are API changes: describe compatibility
  impact, update all callers and docs, and mark breaking changes.

## Code style

- Rust: rustfmt defaults, 4-space indent, UTF-8, LF, final newline; run
  `cargo fmt --all` and keep `cargo clippy` warnings down. Do not silence a lint
  without a comment explaining why.
- Dart/Flutter: 2-space indent, `dart format`, the lints in
  `apps/xenterm/analysis_options.yaml`, and a clean `flutter analyze`.
- YAML/JSON/Markdown: 2-space indent, UTF-8, LF; keep the Chinese/English docs
  aligned and UI strings bilingual where supported.
- Preserve comments that explain why; remove debug prints, commented-out code and
  unrelated formatting churn.

## Review and merge criteria

A PR is ready for review when:

- [ ] Description states the problem, change and verification result
- [ ] Diff is focused; unrelated formatting or features are not mixed in
- [ ] CI passes on the current head commit (not an older one)
- [ ] No merge conflicts and the target branch is synced
- [ ] Dependencies, APIs, config and docs are updated as needed
- [ ] Breaking changes are called out explicitly
- [ ] Logs, screenshots and diff contain no secrets or real credentials
- [ ] Draft → Ready for review was done by the submitter

Ready means ready for review, not approved. Keep answering comments and keep the
description current; whether and when to merge is the maintainers' decision.
Being responsive and keeping changes reviewable matters more than opening many
PRs quickly.

## License

XenTerm is [MIT licensed](LICENSE). By contributing you agree to license your
work under the same terms. Third-party code and assets need a compatible license
and credit in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
