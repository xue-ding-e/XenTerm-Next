# 版本号规范 / Versioning

**简体中文** | [English](#english)

本文是版本号规范 v1 在本仓库的落地说明。规范正文以本文为准。

## 格式（§1）

- 稳定版：`xt-v<M>.<m>.<p>`（如 `xt-v0.7.3`）
- 预发布版：`xt-<channel>-<hash7>-T<unix_seconds>`（如 `xt-beta-1a2b3c4-T1760000000`）
- channel 白名单：`beta | alpha | dev`；`hash7` 是 7 位小写十六进制的提交哈希；`T` 后是 10 位 UTC 秒级时间戳
- 合法正则：`^xt-(v[0-9]+\.[0-9]+\.[0-9]+|(beta|alpha|dev)-[0-9a-f]{7}-T[0-9]{10})$`

## 权威来源（§2）

优先级：`XT_VERSION` 环境变量 > git tag > 分支推导 > dev 兜底。

- `XT_VERSION` 非空时必须通过 §1 校验，否则构建失败，**不静默回退**
- 稳定 tag 与 `XT_VERSION` 不一致时以 `XT_VERSION` 为准并记录警告
- 分支推导：`release/*` → 稳定版（取 Cargo.toml 的版本号，正式以 tag 为准）；`beta*` → beta；`alpha*` → alpha；其余 → dev

## 禁止事项（§3）与注入方式（§5）

- 源码中不硬编码版本号；禁止在运行期通过 git/网络取版本号
- 版本号的**唯一合法拼接点是 [scripts/version.sh](../scripts/version.sh)**；Rust 侧由 [build.rs](../build.rs) 调用它，以编译期环境变量 `XT_VERSION` 注入，运行期通过 `src/version.rs` 的 `version::VERSION` 只读
- Cargo.toml 的 `[package] version` 仅作为打包元数据（deb / MSI / Info.plist 需要 semver），不是应用的显示版本

## version.sh 契约（§4）与可追溯（§6）

- stdout 仅一行版本号，日志走 stderr；退出码 0 成功、非 0 失败
- 解析器默认在仓库根生成 `version.json`（不提交，可用 `XT_VERSION_JSON` 指定输出路径），字段固定如下。它是构建期记录，当前发布工作流不承诺分发完整文件；Next beta/headless 包通过 `BUILD.json` 保存运行时版本、Cargo 包版本、版本方案、时间戳及来源证据，详见[打包契约](headless-server.md#运行时版本与包版本)。

| 字段 | 含义 |
| --- | --- |
| `version` | 完整版本字符串 |
| `channel` | `stable` / `beta` / `alpha` / `dev` |
| `semver` | 稳定版的三段号，预发布为 `null` |
| `hash` | 预发布的 hash7，稳定版为 `null` |
| `timestamp` | 预发布的 Unix 秒，稳定版为 `null` |
| `source` | `xt-version` / `git-tag` / `branch` / `dev-fallback` |
| `branch` / `tag` | 参与推导的分支 / tag，未参与为 `null` |
| `generated_at` | 生成时刻（UTC，ISO 8601） |

## 校验（§7）

- 生成时自检：version.sh 组装出的版本号不合规立即非 0 退出，任何失败都不降级为 dev
- 构建注入自检：`XT_VERSION` 非法时 build.rs 直接令构建失败
- CI 二次校验：[release.yml](../.github/workflows/release.yml) 的 `Verify binary version` 步骤要求产物 `xenterm --version` 输出与 `XT_VERSION` 完全一致

## 更新检测（§8）

判定逻辑在 `src/version.rs::update_decision`，已覆盖单元测试：

- 稳定版：按 semver 大小比较；远端更旧视为异常（不更新、记录）
- beta：hash 相同 → 同一构建不算更新；hash 不同且时间戳更大 → 有更新；hash 不同且时间戳更小/相等 → 不更新、记录异常；客户端需记录完整版本字符串
- alpha / dev：不提供更新检测，客户端应禁用自动更新
- 跨 channel（稳定 vs beta）不比较

<a name="english"></a>

## English

This document describes how versioning spec v1 is implemented in this
repository.

- **Format (§1)** — stable `xt-v<M>.<m>.<p>` (e.g. `xt-v0.7.3`); prerelease
  `xt-<channel>-<hash7>-T<unix_seconds>` with channel ∈ `beta|alpha|dev`.
  Legal regex:
  `^xt-(v[0-9]+\.[0-9]+\.[0-9]+|(beta|alpha|dev)-[0-9a-f]{7}-T[0-9]{10})$`
- **Authority (§2)** — `XT_VERSION` env > git tag > branch derivation
  (`release/*` → stable from Cargo.toml, `beta*` → beta, `alpha*` → alpha,
  anything else → dev) > dev fallback. A non-empty `XT_VERSION` that fails the
  §1 check fails the build; it is never silently downgraded.
- **Single assembly point (§3/§4/§5)** — [scripts/version.sh](../scripts/version.sh)
  is the only place a version string may be assembled. [build.rs](../build.rs)
  calls it and injects the result as the compile-time `XT_VERSION`; runtime
  code reads `src/version.rs::VERSION`. Cargo.toml's `[package] version`
  remains packaging metadata (deb / MSI / Info.plist need plain semver).
- **Traceability (§6)** — the resolver writes gitignored `version.json`
  (schema above) at the repository root unless `XT_VERSION_JSON` overrides
  its path. This is build-time metadata; current release workflows do not
  promise to ship the full file. Next beta/headless packages instead record
  runtime/package versions, scheme, timestamp and provenance in `BUILD.json`;
  see the [packaging contract](headless-server.md#运行时版本与包版本).
- **Checks (§7)** — version.sh self-checks its output; build.rs fails the
  build on any script failure; the release workflow's `Verify binary version`
  step requires `xenterm --version` to match `XT_VERSION` exactly.
- **Update detection (§8)** — `src/version.rs::update_decision`: stable
  compares semver; beta compares hash first, then timestamp (same hash → no
  update; different hash with older/equal timestamp → no update, record the
  anomaly); alpha/dev and cross-channel pairs never auto-update.
