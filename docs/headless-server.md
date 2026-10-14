# Headless 服务器构建与发布 / Server packaging

## 一套核心，按 feature 选择前端

GUI、headless、MCP 和 CLI 已复用同一套配置、会话、SSH、SFTP、终端及自动化核心。
MCP/CLI 经 `automation::call` 统一业务分发；GUI 与自动化共用更底层的 `ConfigStore`、SSH 连接/认证和 SFTP worker。
交互 PTY、GUI 对话框、HTTP OAuth/JSON-RPC、CLI 参数与输出仍是各自适配层。服务器打包不维护另一份业务实现。

服务器必须使用完整命令：

```sh
cargo build --locked --release --no-default-features --features headless
```

只加 `--features headless` 仍会保留默认 GUI；仅 `--no-default-features` 不等于启用 HTTP MCP。
默认桌面构建保持不变。可执行文件名、数据目录、配置结构不因打包种类改名。

## 可复用 GitHub Actions 入口

在本仓库 Actions 中手动运行 **XenTerm Next headless server** (`headless-server.yml`)：

1. `ci_run`：填本仓库 `main` 的已成功 **CI** 运行 ID。工作流从该运行读取完整产品 SHA，不接受任意源码 ref。
2. `publish=false`（默认）：构建并验证，只上传 14 天保存期的 Actions artifact。
3. `publish=true`：必须使用 `main` 上的发布工具；验证后创建独立的 `headless-<完整产品SHA>` 预发布，不改滚动桌面 `beta`。

门禁核对 CI 的仓库、head 仓库、main 分支、ci.yml 路径、push/manual 事件、成功终态、完整 SHA、相对当前 main 的祖先关系，并回读 main 排除检查期间的并发改变。
产品可以是已验证的历史 main 提交；`BUILD.json` 分别记录产品 SHA、工具 SHA、源 CI 和本次构建运行，不能用工具提交冒充产品提交。

构建使用 Ubuntu 22.04 x86_64、锁文件和 headless feature，运行实际 release 二进制的 stdio/HTTP 故障恢复回归。
打包门禁核验 ELF64/x86_64、GLIBC <= 2.35、无未知 GLIBC ABI、基础 libc/libm/libgcc_s/loader 依赖、`ldd` 和版本。
产物包含 MIT/第三方许可证、运行时依赖、BUILD.json、压缩包 SHA256SUMS 和 manifest 中的二进制 SHA256。
不宣称 glibc 2.28 兼容，也不要求服务器安装 GUI 库。

默认权限只读，仅显式发布 job 有短期 `contents: write`；没有部署 secret、SSH、服务器连接或持久凭据配置。
上传后核对精确资产名、大小、digest，完整后才把 Draft Release 转为公开预发布。
已有 tag 或草稿/正式 release 都拒绝覆盖；失败保留证据，由维护者核查后处理，不能盲目重跑或删除既有资产。
发布后应匿名下载核对 SHA256，再按部署流程做一致性 profile 备份、配置兼容预检、单台升级和回滚验收。

首次 issue #1 的一次性发布位于短 tag `headless-2c8da5b8486c`；其固定来源脚本 `publish-headless-server.yml` 仅保留历史工具分支触发规则，日常使用上述通用入口。它不在 main push 上执行。

## Shared-core and release summary (English)

Desktop and headless use the same product source. CLI/MCP share the automation dispatcher; GUI and automation share configuration, SSH authentication/connection and SFTP workers. Their transport and interaction adapters remain distinct.

Build with `cargo build --locked --release --no-default-features --features headless`.
Run **XenTerm Next headless server** with a successful same-repository main `CI` run ID. The default produces artifacts only; explicitly selecting publish on main creates a new fixed-source prerelease. Product source and build tooling SHAs are independently recorded.

The Ubuntu 22.04 runner tests the actual release binary, enforces the ABI/runtime dependency gate and packages license/provenance/checksum files. Only the optional publication job receives temporary repository write permission. It cannot overwrite existing tags/releases. No deployment access or persistent credential is configured. Verify public downloads and preserve a consistent profile backup before upgrading servers.
