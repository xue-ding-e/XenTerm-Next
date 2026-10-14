# Repository instructions for AI contributors

Read [CONTRIBUTING.zh-CN.md](CONTRIBUTING.zh-CN.md) or its
[English version](CONTRIBUTING.md) before editing or submitting work. That guide is
the source for community collaboration and verification practices; this file is
an entry point, not a separate policy.

- Before changes, commits or publication, verify the checkout, remotes, current
  branch, authorized repository and PR base/head. An issue's source repository does
  not by itself identify the repository to fix or grant permission to modify it.
  Resolve conflicting targets before writing; keep unrelated worktrees untouched.
- Follow the guide's Chinese-first communication, topic grouping, dependency/API
  checks, truthful verification and submitter-owned Draft → Ready workflow. Read
  relevant scoped instructions and preserve useful comments and existing work.
- For branch updates and integration, read the
  [linear-history proposal](CONTRIBUTING.md#proposed-linear-history-and-migration-policy)
  ([中文](CONTRIBUTING.zh-CN.md#线性历史与迁移政策提案)). Respect its adoption status:
  a proposal is not evidence of upstream acceptance. Repository-local adoption
  does not authorize rewriting shared history, changing access or bypassing rules.
- Historical migration is separate maintenance work with explicit scope and
  authorization. Use the guide's backup, mapping, tree-equivalence, release-freeze,
  exact-lease and new-commit verification gates; do not apply it as a side effect
  of an ordinary PR. If required access is missing, explain the action, target and
  permission and ask before expanding access.
- If another tool needs a rules entry point, link it here or to the guide instead
  of copying policy text. Keep both guide languages aligned when policy changes.

## Independent repository branch entry point

For `xue-ding-e/XenTerm-Next`, first read the [branch policy](CONTRIBUTING.md#independent-repository-branch-policy-xue-ding-exenterm-next) ([中文](CONTRIBUTING.zh-CN.md#独立仓库的分支约定xue-ding-exenterm-next)). Keep custom development on linear `main`; `upstream-main` is an unchanged official mirror. A fetch never authorizes integrating or overwriting custom work.
