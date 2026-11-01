<p align="center">
  <a href="CONTRIBUTING.md">English</a> · <b>简体中文</b>
</p>

# XenTerm 贡献指南

感谢你帮助改进 XenTerm。本指南说明问题反馈、Pull Request、验证要求和评审标准，是协作规则的
唯一出处；[AGENTS.md](AGENTS.md) 只是 AI 助手的入口。

## 项目结构

XenTerm 由 Rust 核心与 Flutter 桌面界面组成：

| 路径 | 内容 |
| --- | --- |
| `crates/xenterm/` | Rust 服务核心：SSH/SFTP、终端引擎、Telnet、串口、会话与配置、CLI、MCP、资源监控 |
| `apps/xenterm/` | Flutter 桌面应用（界面） |
| `apps/xenterm/native/` | `xenterm-bridge`，界面与核心之间的 FFI 桥接 crate |
| `packaging/`、`scripts/`、`docs/` | 打包配方、构建与发布脚本、设计文档 |

由于界面与 Rust 核心都经过重写，维护者已在 2026 年 10 月重建过一次仓库历史。贡献者无需迁移或
重放旧提交：从当前 `main` 开始正常提交即可。除维护者统一协调外，不要改写共享历史。

参与本项目即表示你愿意遵守[行为准则](CODE_OF_CONDUCT.md)。

## 交流语言

- **优先使用中文。** Issue、PR 的标题、说明和讨论以中文为主，方便社区多数成员参与。
- 欢迎使用英文；维护者会尽量以中文回复。
- 代码标识符、命令、原始报错和必要的英文引文保留原文。
- 社区渠道见 README；影响代码的决定应记录在 Issue 或 PR 中。

## 提问、问题反馈与安全

- 使用问题和开放式想法请到 GitHub Discussions，不要占用 Issue 跟踪器。
- 提交前先搜索现有 Issue 和 PR；相似问题优先补充已有讨论并附上链接。
- 问题反馈请写明 XenTerm 版本、操作系统、安装方式（MSI、便携包、包管理器或源码构建）、
  完整的复现步骤、预期结果和实际结果。
- 发布日志、截图和示例前，请先移除密码、私钥、令牌和真实会话数据。
- **安全漏洞不要公开提 Issue。** 请按 [SECURITY.md](SECURITY.md) 使用 GitHub 私密漏洞报告。

## Pull Request 流程

1. Fork 仓库，从当前 `main` 开分支，分支名与改动对应（`fix/...`、`feat/...`、`docs/...`）。
2. 保持范围聚焦。同一轮相似修复合并到一个 PR，不要拆成大量细碎 PR；也不要为了减少 PR 数量
   把无关改动强行拼在一起。需要拆分时说明边界和依赖，便于独立评审。
3. 保持 PR 可合并。请求评审前 rebase 到目标分支，解决冲突并重新检查受影响的代码和测试；
   目标分支继续变化时及时处理新冲突。不要绕过分支保护或覆盖他人的提交。
4. PR 实现或验证期间保持 **Draft**。完成后检查 diff、填写模板，并自行把
   **Draft 转为 Ready for review**。只留言“已完成”不会触发评审；旧提交上的绿色检查也不等于
   当前提交已验证。若迟迟没有反馈，先说明 PR 状态和已做的操作，不要猜测机器人名称或反复切换状态。
5. 维护者评审通过后，单个主题优先用 **Squash and merge** 合并；多个提交各自完整时可用
   **Rebase and merge**。来自 fork 本身不是添加 merge commit 的理由。合并后从最新 `main`
   开始下一项任务，不继续复用旧分支历史。
6. 相互依赖或改动同一区域的 PR 请互相链接并说明顺序；改写共享分支前先协调。

## 提交信息规范

使用中文 [Conventional Commits](https://www.conventionalcommits.org/)，格式为
`<type>(<scope>): <摘要>`。

- 类型：`feat`、`fix`、`docs`、`refactor`、`perf`、`test`、`build`、`ci`、`chore`、`revert`。
- scope 对应模块或区域：`ssh`、`sftp`、`terminal`、`telnet`、`serial`、`config`、`cli`、
  `mcp`、`bridge`、`ui`、`deps`、`ci`。
- 摘要用中文，简洁、祈使语气、不加句号。例如：`fix(ssh): 修正跳板链断开后未回收会话`。
- 一个提交只做一件事；摘要不够时在正文说明原因、取舍和影响。评审前 squash 掉 fixup 提交。
- 不要在 PR 中修改版本号或打 tag；发版由维护者通过发布脚本完成。

## 验证要求

按改动范围运行对应检查，并如实记录实际运行的内容：

```sh
cargo check --locked
cargo test --locked
cd apps/xenterm && flutter pub get && flutter analyze && flutter test
```

- Cargo 命令务必保留 `--locked`。`Cargo.lock` 是有意跟踪的，用来保证发布构建可复现；
  若因清单与锁文件不一致而失败，应修正有意的依赖改动，而不是去掉 `--locked`
  把重新解析后的成功当作验证通过。
- 只改服务核心：`cargo test --locked --no-default-features --features headless`；
  headless 测试通过不能证明 Flutter 界面或各平台 API 正常。
- 只改 Flutter：`flutter analyze` 和 `flutter test` 即可；跨 bridge 的改动需要两侧都验证。
- 涉及 Windows 打包或安装/升级行为：构建 MSI 或便携包，在 Windows 上实机验证安装、升级和卸载。
  本机无法覆盖的平台请明确说明，不要声称“全平台通过”。
- 写明使用的命令、操作系统和提交 SHA，并区分“已通过”“失败”“未运行”“环境阻塞”。
  手工验证应写出步骤、预期和实际行为；界面改动附截图或简短录屏。
- 新行为应补充相应测试；bug 修复最好给出同一测试在修复前失败、修复后通过的红绿证据。
  过滤后运行了零个测试不算覆盖。
- 纯文档改动：核对链接、路径、示例和表述与当前代码一致即可，无需声称运行过代码检查。
- AI 生成的测试输出、没有实际运行过的命令、旧提交的截图都不能作为证据。解决冲突或继续改代码后，
  重新运行受影响的检查并更新 PR 说明。

## 依赖与 API

- 以目标分支当前的 `Cargo.toml`、`Cargo.lock`、`apps/xenterm/pubspec.yaml` 以及锁定版本的
  实际源码为依据，不要照搬旧分支、旧教程或模型记忆中的接口。
- `Cargo.lock` 和 `apps/xenterm/pubspec.lock` 都是有意跟踪的，不要提议忽略它们。
- 普通修复应保持锁文件稳定。依赖升级应单独提 PR（由 Dependabot 发起），并说明原因和影响；
  不要夹带无关升级。
- 新增 crate 或 package 需要简短说明：维护状态、许可证兼容性、构建与 CI 成本；先确认
  MSRV 和 Flutter SDK 约束。
- 协议、配置格式和 FFI 接口的改动属于 API 变更：说明兼容性影响，同步更新调用方与文档，
  并在 PR 中标注破坏性变更。

## 代码风格

- Rust：遵循 rustfmt 默认风格，4 空格缩进，UTF-8、LF、文件末尾换行。评审前运行
  `cargo fmt --all`，并尽量清掉 `cargo clippy` 警告；压制 lint 时用注释说明原因。
- Dart/Flutter：2 空格缩进，使用 `dart format`，遵循
  `apps/xenterm/analysis_options.yaml` 中的 lint，`flutter analyze` 必须干净。
- YAML/JSON/Markdown：2 空格缩进，UTF-8、LF。中英文文档成对维护，界面文案在支持双语的区域
  保持双语。
- 保留解释“为什么”的有用注释；删除调试输出、注释掉的代码和无关的格式化改动。

## 评审与合并标准

PR 满足以下条件即视为可以评审：

- [ ] 说明写清了问题、改动和验证结果
- [ ] diff 聚焦，没有混入无关的格式化或功能改动
- [ ] CI 在当前 head 提交上通过（不是在旧提交上）
- [ ] 没有合并冲突，目标分支已同步
- [ ] 依赖、API、配置格式和文档已按需更新
- [ ] 破坏性变更已明确标注
- [ ] 日志、截图和 diff 中没有密钥或真实凭据
- [ ] 提交者已自行完成 Draft → Ready for review

Ready 表示可以开始评审，不代表已经批准。请持续回应评审意见并保持说明最新；是否合并、何时合并
由维护者决定。保持响应、让改动易于评审，比快速开大量 PR 更重要。

## 许可证

XenTerm 基于 [MIT 许可证](LICENSE) 发布。贡献代码即表示你同意以相同条款授权。第三方代码和素材
必须使用兼容许可证，并在 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 中署名。
