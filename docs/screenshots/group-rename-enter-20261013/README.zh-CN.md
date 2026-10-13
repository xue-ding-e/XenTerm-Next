# 已有分组重命名：Enter 红绿验收

## 环境与版本

- 日期：2026-10-13；Debian 13 / x86_64，Xfce / X11，1364×1024，Mesa Lavapipe，Rust 1.99.0，XenTerm 0.7.3
- 修复前源码：`a622b1d22e7ece364dfabb5f308e78c27ba6f014`
- 修复后源码：本目录所在修复提交；`src` Git tree 为 `f31aa7ba2e67935197de5d5e8d08b99fd4ea3651`
- 修复前二进制 SHA256：`74e6a864c75a9cbb0f0ddd15cf60872cfcd1794b5ea255e28d7bcf5294b4d36e`
- 修复后二进制 SHA256：`52bc0ccbe8952871502dd98e11acd63e85b5922b9984d373a4a47312cd3ea876`
- 独立合成配置，仅 QA 分组及未连接的示例地址 `192.0.2.10`，没有真实凭据

## 复现与根因

1. 打开「服务器 → 分组管理」，建立 `QA Alpha`、`QA Beta`
2. 点击已有 `QA Beta` 行的铅笔，聚焦行内重命名输入框
3. 将名称改成 `QA Renamed`，按 Enter，不点击保存按钮
4. 旧版关闭整个弹窗，分组仍为 `QA Beta`；修复后名称保存为 `QA Renamed`，分组管理弹窗保持打开

问题不只在 macOS：以上已在 Linux 原生界面复现。不要用上方「新分组名称」框代替已有组的行内编辑。

调用链：GPUI 0.7.1 单行 `InputState::enter` 先传播 Enter，再发出 `InputEvent::PressEnter`；原 GroupManager 订阅只重绘，不提交；外层 Dialog 的默认 Confirm 因此关闭弹窗。修复在重命名输入的父节点消费 `input::Enter`，核对实际焦点及 marked text，再复用保存逻辑；未修改通用 Dialog 或 Shell 的确认行为。

## 截图

| 截图 | 证据 |
| --- | --- |
| [rename-before-enter.png](rename-before-enter.png) | 旧版已有组行内输入改名草稿，准备按 Enter |
| [rename-after-enter-bug.png](rename-after-enter-bug.png) | 旧版 Enter 后弹窗消失，名称仍为 QA Beta |
| [rename-fixed-enter-success.png](rename-fixed-enter-success.png) | 修复版 Enter 保存为 QA Renamed，弹窗仍在 |
| [rename-fixed-empty-error.png](rename-fixed-empty-error.png) | 空名称显示行内错误，草稿和弹窗保留 |
| [rename-fixed-duplicate-error.png](rename-fixed-duplicate-error.png) | 重复名称显示行内错误，草稿和弹窗保留 |

截图是实际原生界面，不是示意图。成功后再次改名、行内取消、Escape 和右上角关闭分别验证；Escape/右上角关闭沿用整窗关闭且不保存的旧语义，行内「取消」只取消该行编辑。

## 自动化与边界

本次新增 6 项回归，分组弹窗测试 15/15 通过；Linux 桌面全量 919 项通过，7 个已有 benchmark 忽略；headless 535 项通过。

覆盖真实输入焦点和键盘事件、空名/重复名/保留名、重复 Enter、保存按钮、同名无操作、连续改名、磁盘失败后的缓存/关联会话/折叠状态回滚与重试、创建框焦点不误提交旧行、取消与关闭。组合输入使用 GPUI 的真实 marked-text / IME commit 接口：候选确认 Enter 不保存，提交组合文字后第二次 Enter 才保存。

已有原生开发依赖的独立检出中运行：

```sh
cargo test --locked --jobs 2 ui::shell::group_dialog_tests -- --test-threads=1
cargo test --locked --jobs 2 -- --test-threads=1
cargo test --locked --no-default-features --features headless --jobs 2
```

复现旧版自动化红灯时，在独立旧版检出中仅移入本提交的 `src/ui/impls/group_dialog_tests.rs`，运行 `inline_rename_enter_saves_the_row_without_closing_the_group_manager`；该测试在旧实现失败，在修复实现通过。不要覆盖正在开发的工作副本。

CI 已在现有 Linux、Windows、macOS desktop job 中加入同一组分组回归，不增加 runner。当前提交的 CI 结果应查看工作流；Linux 手动验收与跨平台自动化是不同证据，未声称在原生 Windows/macOS 中文输入法中手动验证。
