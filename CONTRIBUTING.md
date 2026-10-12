<p align="center">
  <b>English</b> · <a href="CONTRIBUTING.zh-CN.md">简体中文</a>
</p>

# XenTerm Community Contribution Guide

Welcome bug reports, feature suggestions and code contributions. This guide
consolidates the collaboration requirements already stated by the maintainers and
offers pre-submission verification suggestions; the newly added communication
conventions and check recommendations are submitted with this contribution-guide
proposal for maintainer review and do not represent a community-wide resolution that
is already in force. If a maintainer sets new requirements on a specific PR, those
latest requirements prevail.

This guide supplements the existing [Chinese README](README.zh.md), the
[app test instructions](tests/app/README.md) and the per-feature documents; it
neither replaces nor removes them. AI-assisted contributors should start with the
repository-root [AGENTS.md](AGENTS.md); it links back here rather than defining a
second set of collaboration rules.

## Communication language

- **Chinese first.** Issue and PR titles, descriptions and follow-up discussion are
  primarily in Chinese, so that most community members can take part.
- Non-native Chinese speakers are welcome to write in English; maintainers will reply
  in Chinese where possible and add clarifications as needed.
- Keep code identifiers, commands, raw error output and necessary English quotations
  in their original form.

## Maintainer-established collaboration requirements

### Stability and interaction quality first

The current maintenance focus is fixing problems, improving UI and interaction
feedback, and driving toward the first stable release; new features should start
with a scope and design discussion, and a flood of feature proposals must not crowd
out stability fixes. Fix proposals still need to explain their trade-offs and pass
review — "the problem is real" does not mean any implementation is automatically
approved for merge.

### Keep PRs mergeable

PRs with merge conflicts cannot be merged. Before requesting review, sync the target
branch, resolve conflicts, and re-check the affected code and tests. While the
target branch keeps moving during review, please handle new conflicts promptly.

Correct conflict resolution, a clear diff and a mergeable result remain essential.
New history recommendations are described separately in the
[proposed linear-history and migration policy](#proposed-linear-history-and-migration-policy);
their status is distinct from the maintainer-established requirements.

### Batch frequent, similar changes

Please consolidate frequently repeated, similar changes into one PR instead of
splitting the same round of fixes into many tiny PRs. Related bug fixes can be
submitted together, itemizing the problem, the change and the verification result
for each.

Organize by shared problem, module or a verifiable theme: follow-up fixes for the
same feature should go into the existing PR first; unrelated new features,
refactors and fixes should not be force-combined just to reduce the PR count. When
a split is needed, state the boundaries and dependencies so each part can be
reviewed independently.

### Convert Draft to Ready when done

The maintainers have switched PR review bots; the old bot's workflow is no longer
the operational basis. When the work is done, the submitter must convert the PR
from **Draft** to **Ready for review** themselves to trigger the current review
bot.

Keep the PR as a Draft while it is still being implemented, while conflicts are
being handled or while verification is being added; once ready, complete the
conversion proactively instead of leaving a "done" comment and waiting for a
maintainer to do it. After converting, check the actual review feedback on the PR
and respond item by item. A failed trigger, stale comments or stale check results
from the old bot do not prove the current bot has finished reviewing, let alone
that this change passed; verify against the commits the current bot's feedback
actually refers to. If the expected feedback does not appear, report the PR's
current state and the steps you already took before asking the maintainers; do not
guess bot names, trigger phrases or permissions, and do not repeatedly toggle the
state trying to force a trigger.

### Verify current dependencies and APIs

AI-assisted development still requires checking API versions: an interface that
worked on an old version does not necessarily apply to the current one. Submitters
must review generated code and are responsible for the final result.

Base your work on the target branch's current `Cargo.toml`, `Cargo.lock`, the
actually enabled features, and the corresponding version's source code or
documentation; do not copy interfaces straight from old branches, old tutorials or
model memory.

## Proposed linear-history and migration policy

**Status: a policy proposal awaiting upstream maintainer acceptance.** The existing
mainline contains merge commits, and the existing guide did not mandate rebase or
merge. Do not present the new policy below as a previously established rule. Forks
may adopt it within their own collaboration scope; upstream adoption, its effective
date and exceptions require an explicit maintainer decision. Keep this section and
its Chinese translation aligned. AI entry points and the PR template reference
these sections rather than maintain another history policy.

### Keep routine development history linear

- Use short-lived branches for ordinary fixes and development on the same mainline.
  Rebase onto the current target before review and organize coherent, verifiable
  commits; avoid repeatedly merging main back into a topic branch.
- Prefer **Squash and merge** for one reviewed topic. Use **Rebase and merge** when
  several individually complete logical commits deserve preservation. Neither adds
  an ordinary PR merge node to the mainline. There is no requirement to squash the
  work branch into one commit after every PR update.
- Group related fixes, listing each change, without combining unrelated features
  merely to simplify the graph. After squash merging, start the next task from the
  updated mainline rather than reusing the old topic history.
- A merge commit is an exception for a substantial, independently evolving,
  long-lived feature whose integration ancestry is important. Explain the reason
  and obtain maintainer agreement first. Coming from a fork alone is not a reason.
- Coordinate before rewriting a shared or published work branch. Keep a verified
  recovery backup, reread the full remote SHA and use an explicit ref and expected
  old SHA with `--force-with-lease` when replacement is needed. Never use bare
  `--force`; stop if the remote changed.
- Rerun affected checks after rebase, squash or conflict resolution. Green checks
  on an old SHA are not validation of the rewritten SHA.

### Review and execute history migration separately

- Ordinary PR squash/rebase merging appends contributions to the target history;
  it does not replace the target branch's existing ancestors. Full-history cleanup
  requires a coordinated maintenance window and explicit ref cutover. Feature PR
  approval does not implicitly authorize rewriting shared history.
- Create and verify a restorable Git bundle, and back up all affected refs, Git
  objects, release tags and assets (release assets require a separate backup; they
  are not in the bundle). Keep old → new mappings and genuine contributor
  attribution. When linearizing merges,
  preserve the actual integrated snapshot, including conflict resolutions, rather
  than blindly replaying side commits. Verify every mainline snapshot's tree.
- Do not incorporate unapproved feature PRs into main as part of a historical
  rewrite. Document PR dependencies, branch restacking, archival, tag migration
  and release rebuilding separately.
- Inspect automation side effects first: in the current
  [release.yml](.github/workflows/release.yml), an upstream `main` push triggers
  rolling beta publication, deleting and recreating the beta release and tag.
  Coordinate a release freeze, asset backup, rebuild verification and resumption;
  do not rely solely on
  skip-CI commit messages.
- Check permissions, branch rules and exact expected ref SHAs before cutover.
  Replacement updates require explicit `--force-with-lease=<ref>:<expected-old-sha>`;
  a lease without the expected SHA or bare force is not a substitute. Never bypass
  protection or overwrite concurrent work. Read back target and unaffected refs,
  and retain CI, release rebuild and acceptance evidence for the new SHAs. Rollback
  also requires exact expected SHAs so it cannot erase work added after cutover.
  If access is insufficient, explain the action, target and required permission
  and obtain authorization before expanding access.
- GitHub-managed historical `refs/pull/*` are not ordinary writable branches. Old
  PR pages and commit links are not guaranteed to disappear. Collaborators should
  obtain the new development branches without pushing the archived old graph back.

## Communication conventions

- Search existing issues and PRs before submitting; highly similar issues should
  extend the existing discussion and link to it
- Bug reports state the XenTerm version or commit, the operating system, the
  install or build method, reproduction steps, expected results and actual results
- PRs state the problem solved, the solution and why, the scope of impact, related
  issues and verification results; when combining multiple related fixes, list them
  item by item. For bug fixes, state the identified root cause and the key call
  chain; clearly mark undetermined parts as speculation
- Remove passwords, private keys, access tokens, real session credentials and other
  sensitive information from logs, screenshots and examples first

## Recommended implementation and verification practices

These are suggestions to improve reviewability and reduce regressions; they must
not be presented as an all-encompassing test gate additionally mandated by the
maintainers. Choose checks according to the scope of the change and honestly
record what was not done.

### Dependencies, lockfiles and features

1. Check the target branch's latest dependency configuration before developing;
   align first, then judge whether it is an incorrect call or a genuinely needed
   upgrade.
2. Use API documentation or source code matching the locked versions; pay attention
   to method signatures, trait imports, conditional compilation, event lifetimes
   and platform differences. Do not casually downgrade a dependency just to keep an
   old interface.
3. Ordinary feature fixes should keep the lockfile stable. When a dependency change
   is truly needed, explain the reason, impact and the related `Cargo.toml` /
   `Cargo.lock` changes in the PR; do not smuggle in unrelated upgrades.
4. Prefer verifying with `--locked`. If it fails because the manifest and lockfile
   disagree, fix the intentional dependency change; do not simply drop `--locked`
   and treat a re-resolved success as the original lockfile having passed.
5. Distinguish desktop from headless builds. The current feature definitions are in
   [Cargo.toml](Cargo.toml): the default build contains the desktop features;
   headless paths use `--no-default-features --features headless`. Headless tests
   passing does not prove the desktop UI or platform APIs are correct; do not
   accidentally pull optional GUI dependencies into headless paths.

### Verify what you actually changed

- Rust or desktop code: run `cargo check --locked` and execute tests covering the
  change; if your machine lacks system development libraries, say so — do not write
  an environment blocker up as a passed check
- Related unit and integration tests: use `cargo test --locked` or an appropriate
  test filter; give the complete command in the description and confirm the
  intended tests actually ran — a filtered run that executed zero tests is not
  valid coverage
- New tests under `tests/app/`: follow the existing module wiring and confirm the
  test is on a compiled and executed path; see that directory's README. Creating a
  test file alone does not necessarily execute it
- Headless, CLI or MCP paths: run
  `cargo test --locked --no-default-features --features headless` and the related
  scenario checks according to the affected scope; for deployment, authentication
  or protocol work refer to the [Remote MCP document](docs/REMOTE_MCP.md)
- GUI interactions: supplement real operation steps and results, with screenshots
  or recordings when necessary; for focus, mouse, keyboard or dialog issues check
  real event behavior, covering recovery after repeated operations, cancellation
  and occlusion; for style changes observe light/dark themes plus necessary zoom
  and window sizes, without rerunning the full suite of unrelated features.
  Compilation success alone does not prove a fix
- Platform-specific code: verify on the affected platforms and target toolchains;
  when your machine cannot cover them, list the platform gaps explicitly and judge
  from actual CI results — never claim "passed on all platforms"
- Documentation-only changes: check that links, paths, examples and wording match
  the current code; no need to fill unused code checks in as passed

Existing development conventions continue to apply, including the GPUI interface,
paired Chinese and English translations, and how UI icons and text are used — see
the [README development section](README.zh.md#开发). Do not misread "the terminal
content supports emoji" as permission to replace application UI icons with emoji.

### Provide verifiable evidence

Record in the PR the commit, system, toolchain, features, full commands and results
used for verification. For manual verification, write out the steps, the expected
and the actual behavior; attach run links for CI results. For bugs suited to
automated regression, provide red-green evidence — the same test failing on the old
implementation and passing after the fix — plus negative cases showing inputs that
must be rejected are still rejected. Reproduction commands should pin the exact
commit and state build dependencies, configuration and launch steps, so a reviewer
can reproduce with one click or a few steps. Distinguish "passed", "failed", "not
run" and "blocked by environment", and state known limitations.

AI-generated test output, commands that were never actually run, screenshots of old
commits, or green checks from other branches are not evidence that this change was
verified. After resolving conflicts or continuing to change code, re-run the
affected checks and update the description.

## Review readiness and post-submission self-check

The following checklist helps submitters confirm their preparation; it adds no
mandatory platform or testing requirements that have not been confirmed by the
maintainers.

- [ ] Changes are complete; related fixes are grouped by topic; no unrelated
      features smuggled in
- [ ] The target branch's latest state was checked, merge conflicts resolved, and
      the resolution reviewed
- [ ] Current dependencies, lockfile, features and APIs verified; necessary
      dependency changes explained
- [ ] The PR states the problem, the solution and rationale, the scope of impact
      and related discussion
- [ ] Verification was completed according to the scope of the change with real
      results; uncovered items and environment blockers are clearly listed
- [ ] New tests are actually on an executed path; existing documentation, comments
      and feature conventions are preserved or updated with a reason
- [ ] Sensitive content has been removed from logs and screenshots
- [ ] The submitter performed **Draft → Ready for review** themselves and checked
      the current review bot's feedback

Ready means ready for review, not approved for merge. Keep responding to review
comments and updating the change description and verification results; whether to
merge is ultimately the maintainer's decision.
