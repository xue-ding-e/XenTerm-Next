# Changelog

All notable changes to XenTerm are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the packaging version
follows [Semantic Versioning](https://semver.org/). The application version string
itself is assembled by `scripts/version.sh` as described in
[docs/versioning.md](docs/versioning.md).

Each release lists changes under **Added**, **Changed**, **Removed** and **Fixed**;
every entry gives the Chinese summary first and the English summary after a `/`.
Entries before 0.8 describe the desktop application as it existed before the Flutter
rewrite and are kept for reference only.

本文件记录 XenTerm 的所有重要变更。格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
打包版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。应用版本字符串由
`scripts/version.sh` 统一拼接，规范见 [docs/versioning.md](docs/versioning.md)。
每个版本按 **新增 / Added**、**改进 / Changed**、**移除 / Removed**、**修复 / Fixed**
分类；每条变更先给中文摘要，再以 `/` 分隔给出英文摘要。0.8 之前的条目描述 Flutter
重写前的桌面应用，仅作历史存档。

## [Unreleased]

### Added / 新增
- **全新 Flutter 桌面界面** / A new Flutter desktop UI replaces the previous interface; hosts, terminal, SFTP, keychain, tunnels, snippets, known hosts, logs and settings all run on the new shell.
- **单色主题** / A neutral monochrome theme: a 12-step grey scale on black with a white accent, plus a light theme.
- **统一组件** / One shared component set for inputs, selects, buttons and dialogs across every screen.
- **Windows MSI 安装包** / A Windows MSI installer alongside the portable zip; it creates Start-menu and desktop shortcuts with the real XenTerm icon.
- **界面语言选择** / A persisted interface-language choice (English / 简体中文) in Settings, covering the navigation rail and the Settings screen.

### Changed / 改进
- **仓库重组** / The repository is organised around `crates/xenterm` (Rust core) and `apps/xenterm` (Flutter UI), with the FFI bridge in `apps/xenterm/native`.
- **文档与工作流整理** / Documentation and workflows refreshed: new README, a condensed CHANGELOG, updated third-party notices, and release automation for the new layout.

### Removed / 移除
- **旧界面资源** / Removed the previous desktop UI sources, screenshots and component licences.

### Fixed / 修复
- **串口连接可删除** / Saved serial connections now offer the same "Remove host" action as SSH, Telnet and local sessions, from a shared drawer menu.
- **Linux 启动找不到桥接库** / The Flutter bundle now loads `libxenterm_bridge.so` from the executable's directory instead of relying on the working directory.

## [0.7.3] - 2026-09-07

### Changed / 改进
- **保留 Intel Mac 安装包** / Keep Intel Mac packages.

### Fixed / 修复
- **修复内置文本查看器和编辑器的行号错位** / Fix line-number alignment in the built-in text viewer and editor.
- **修复串口会话误显示 SSH 端口（#406）** / Fix serial sessions showing an SSH port (#406).
- **修复会话输入框提示文字重叠（#415）** / Fix overlapping placeholders in session fields (#415).
- **修复命令输入框全选与多行滚动（#416，输入框部分）** / Fix command-input select-all and multiline scrolling (input-field portion of #416).

## [0.7.2] - 2026-09-04

### Added / 新增
- **支持终端鼠标追踪与 VT100 线框字符（#399、#400）** / Support terminal mouse tracking and VT100 line drawing (#399, #400).
- **支持在多个标签栏之间拖动标签页（#408）** / Support dragging tabs across tab bars (#408).

### Changed / 改进
- **优化终端内存管理与跨平台分配器（#410）** / Improve terminal memory management and cross-platform allocation (#410).

### Fixed / 修复
- **修复旧版 macOS 启动闪退** / Fix startup crashes on older macOS.
- **修复多项 SFTP 文件操作问题（#401、#402、#403）** / Fix several SFTP file-operation issues (#401, #402, #403).
- **修复 Wayland 启动后出现多个任务栏窗口（#404）** / Fix duplicate taskbar windows after startup on Wayland (#404).
- **修复右键重命名标签页时闪退（#411）** / Fix crashes when renaming tabs from the context menu (#411).
- **修复通过 JumpServer/Koko 连接后 Shell 自动断开（#359）** / Fix shells disconnecting through JumpServer/Koko (#359).

## [0.7.1] - 2026-08-29

### Changed / 改进
- **完善终端内 ZMODEM `rz` 上传（#308）** / Improve in-terminal ZMODEM `rz` uploads (#308).
- **优化文件传输工具栏与拖拽上传（#397）** / Polish the file-transfer toolbar and drag-and-drop uploads (#397).

## [0.7.0] - 2026-08-27

### Added / 新增
- **终端内 ZMODEM 支持 `rz` 上传（#308）** / Support in-terminal ZMODEM uploads with `rz` (#308).

### Fixed / 修复
- **修复 SSH 私钥选择器与多行粘贴键盘操作（#392、#393）** / Fix SSH key selection and multiline-paste keyboard handling (#392, #393).

## [0.6.16] - 2026-08-24

### Changed / 改进
- **维护性更新与内部修复** / Maintenance and internal fixes.

## [0.6.15] - 2026-08-24

### Added / 新增
- **新增 MCP 与 CLI 自动化入口** / Add MCP and CLI automation entry points.

### Fixed / 修复
- **修复 Fedora 等非 Debian Linux 桌面按下 Ctrl 即触发终端快捷键的问题（#369）** / Fix bare Ctrl triggering terminal shortcuts on Fedora and other non-Debian Linux desktops (#369).

## [0.6.12] - 2026-08-16

### Added / 新增
- **支持双击标签复制连接（#340）** / Support duplicating connections by double-clicking tabs (#340).
- **命令栏新增历史命令自动提示（#349）** / Add command-history autocomplete to the command bar (#349).
- **快捷命令支持手动排序（#310）** / Support manual quick-command ordering (#310).
- **支持配置多个 WSL 启动项和默认目录（#336）** / Support multiple WSL entries and startup directories (#336).
- **SSH config 导入支持 `Include`（#341）** / Support `Include` while importing SSH config (#341).
- **SSH 终端支持多字符集（#338）** / Support multiple character encodings in SSH terminals (#338).
- **支持在设置中关闭多行粘贴确认（#346）** / Allow disabling multiline-paste confirmation in Settings (#346).

### Changed / 改进
- **降低键盘输入后的字符回显延迟** / Reduce character-echo latency after keyboard input.

## [0.6.10] - 2026-08-05

### Changed / 改进
- **回复终端状态与设备属性查询（#328）** / Respond to terminal status and device-attribute queries (#328).

### Fixed / 修复
- **修复关闭“欢迎页设为侧栏”时闪退（#323）** / Fix crashes when disabling “Welcome page as sidebar” (#323).
- **修复快速连接中重复的 system 分组和空白右键菜单（#316、#324）** / Fix duplicate system groups and empty context menus in Quick Connect (#316, #324).
- **修复内置编辑器打开大文件时崩溃，并调整历史命令排序（#331）** / Prevent large-file editor crashes and reorder command history (#331).
- **修复 zsh 中 Home 和 End 按键无效（#329）** / Fix Home and End keys in zsh (#329).

## [0.6.9] - 2026-07-31

### Changed / 改进
- **快速连接分组默认收起并记住展开状态** / Default Quick Connect groups to collapsed and remember their state.
- **提升终端鼠标拖选文字的响应速度** / Improve terminal text-selection responsiveness.

### Fixed / 修复
- **修复 OpenWrt SSH 登录时泄露并卡在 shell 集成初始化命令的问题（#314、#317）** / Prevent shell-integration setup from leaking or hanging OpenWrt SSH sessions (#314, #317).
- **修复终端粘贴、命令框、光标、选区与回滚历史问题（#319）** / Fix terminal paste, command bar, cursor, selection, and scrollback behavior (#319).
- **修复 macOS 下 `Ctrl+X` 在 nano 中错误打开搜索的问题（#312）** / Fix `Ctrl+X` opening search instead of exiting nano on macOS (#312).
- **改善超大终端输出的渐进显示与响应性（#311）** / Improve progressive rendering and responsiveness under very large terminal output (#311).

## [0.6.8] - 2026-07-26

### Added / 新增
- **支持使用快捷键循环切换标签页（#294）** / Add keyboard shortcuts for cycling tabs (#294).

### Changed / 改进
- **Windows MSI 会沿用已有安装目录并创建桌面快捷方式（#293）** / Preserve the existing Windows MSI install location and add a desktop shortcut (#293).
- **优化大容量终端回滚历史（#290）** / Optimize large terminal scrollback histories (#290).

### Fixed / 修复
- **修复新建 Telnet 会话的默认端口（#303）** / Use the standard Telnet port for new sessions (#303).
- **修复 Windows 使用 `Ctrl+Space` 切换输入法后 Ctrl 状态残留（#309）** / Fix the Ctrl modifier remaining active after switching the Windows IME with `Ctrl+Space` (#309).
- **修复 SSH 内部初始化命令污染远端历史（#289）** / Prevent SSH shell setup from polluting remote history (#289).
- **修复 SSH 会话中 Bash 历史命令重绘错位（#289）** / Fix misaligned Bash history repainting in SSH sessions (#289).

## [0.6.7] - 2026-07-25

### Changed / 改进
- **维护性更新与内部修复** / Maintenance and internal fixes.

## [0.6.6] - 2026-07-23

### Added / 新增
- **快捷指令支持主窗口四向停靠（#215）** / Dock quick commands on any main-window edge (#215).

### Fixed / 修复
- **改进侧边停靠的 SFTP 面板窄宽布局** / Improve the narrow layout of side-docked SFTP panels.
- **修复 KDE Wayland 下窗口恢复后的点击偏移（#286）** / Fix click offsets after restoring a window on KDE Wayland (#286).
- **简化会话端口转发规则编辑（#277）** / Simplify session port-forward rule editing (#277).
- **修复 Windows 输入法切换后输入框失效（#236）** / Fix text fields becoming unusable after switching IMEs on Windows (#236).

## [0.6.5] - 2026-07-17

### Added / 新增
- **支持自定义终端输入光标（#275）** / Add customizable terminal insertion cursors (#275).

### Fixed / 修复
- **修复 SFTP 左右停靠时工具栏越界（#285）** / Fix SFTP toolbar overflow when docked left or right (#285).
- **缩短 SSH 会话终端首屏等待时间** / Reduce SSH terminal time-to-first-frame.
- **修复跨平台多行文本粘贴格式错位（#284）** / Fix cross-platform multi-line paste formatting (#284).
- **优化当前标签的高亮样式（#283）** / Improve active-tab highlighting (#283).
- **修复“测试连接”未验证 SSH 凭据的问题（#276）** / Fix connection tests that did not validate SSH credentials (#276).

## [0.6.4] - 2026-07-17

### Added / 新增
- **原生支持 PuTTY / MobaXterm 的 PPK 私钥（#281）** / Add native PuTTY / MobaXterm PPK private-key support (#281).

### Fixed / 修复
- **修复主窗口无法记住上次调整大小的问题（#278）** / Fix the main window forgetting its last adjusted size (#278).
- **修复 Windows 运行期间无法通过安装程序更新的问题（#267）** / Fix updates getting stuck while XenTerm is running on Windows (#267).
- **修复 Debian 桌面端 nano 中 `Ctrl+X` 错误进入搜索的问题（#274）** / Fix `Ctrl+X` opening search instead of exiting nano on Debian desktops (#274).
- **修复超长多行粘贴无法确认的问题（#271）** / Fix unconfirmable long multi-line pastes (#271).

## [0.6.3] - 2026-07-16

### Added / 新增
- **终端支持彩色 emoji（#269）** / Render color emoji in the terminal (#269).
- **进程监视支持居中打开、复制 PID 与安全结束进程** / Center the process monitor, copy PIDs, and safely terminate remote processes.
- **纯文本日志级别自动高亮** / Automatically highlight plain-text log levels.
- **新增输出高亮设置与 DevOps 规则集** / Add output-highlighting settings and a DevOps preset.
- **支持自定义输出高亮规则** / Support custom output-highlighting rules.

### Fixed / 修复
- **不再采集或显示本地系统详情（#268）** / Stop collecting and exposing local system details (#268).
- **修复 Windows 11 双屏不同分辨率/缩放下的窗口定位与最大化恢复（#254、#272）** / Fix window placement and maximized recovery on Windows 11 mixed-resolution/DPI displays (#254, #272).
- **修复进程列表停止刷新以及 root 进程无法结束** / Fix frozen process lists and root-process termination.

## [0.6.2]

### Added / 新增
- **新增终端字体加粗开关 (#262)** / Add a bold terminal text option (#262).
- **新增详细系统信息窗口** / Add a detailed system-information window.
- **新增本地终端入口** / Add a built-in local terminal entry.
- **终端支持 Ctrl 多选与 Shift 范围扩展 (#262)** / Add Ctrl multi-selection and Shift range extension (#262).
- **新增多行粘贴安全确认提示 (#262)** / Add a multi-line paste safety prompt (#262).
- **默认收起内置 system 终端分组** / Collapse the built-in system terminal group by default.

### Fixed / 修复
- **修复 SFTP 文件列表下方出现大块空白的问题 (#259)** / Fix a large blank area under the SFTP file list (#259).
- **修复 oh-my-zsh 首屏显示 shell integration 注入命令的问题 (#257)** / Fix shell-integration setup echo leaking on oh-my-zsh servers (#257).

## [0.6.1] - 2026-07-11

### Added / 新增
- **新增运行时 SSH 隧道面板 (#206)** / Add a runtime SSH tunnel panel (#206).

### Fixed / 修复
- **移除点击标签页时复制标签名称的旧行为** / Remove the old copy-title-on-tab-click behavior.

## [0.6.0] - 2026-07-10

### Fixed / 修复
- **修复拖动资源侧栏宽度时突然变宽的问题 (#244)** / Fix sudden resource-sidebar width jumps while resizing (#244).
- **修复 Windows 10 无边框窗口点击坐标整体错位的问题 (#193)** / Fix whole-window click offset on Windows 10 frameless windows (#193).
- **改善 Windows 高 DPI 缩放下字体发糊的问题 (#224)** / Improve blurry text on Windows high-DPI scaling (#224).
- **修复 SFTP 面板拖动后拖拽上传命中区域错误的问题 (#253)** / Fix drag-and-drop upload hit testing after moving the SFTP panel (#253).
- **支持显式选择键盘交互认证 (#249)** / Support explicit keyboard-interactive authentication (#249).
- **修复 macOS 触控板无法滚动终端的问题 (#252)** / Fix terminal scrolling with the macOS trackpad (#252).

## [0.5.71] - 2026-07-10

### Added / 新增
- **SFTP 文件列表支持排序 (#248)** / Support sorting the SFTP file list (#248).

### Changed / 改进
- **WebDAV 上传前自动创建缺失目录** / Create missing WebDAV folders before upload.

### Fixed / 修复
- **修复顶部工具栏与 SFTP 操作栏间距问题 (#245)** / Fix top toolbar and SFTP action bar spacing (#245).
- **修复复制终端软换行文本时多出换行的问题 (#241)** / Preserve logical lines when copying soft-wrapped terminal text (#241).
- **修复部分旧服务器 SFTP 认证失败的问题 (#186)** / Fix SFTP authentication on some legacy servers (#186).

## [0.5.7] - 2026-07-08

### Changed / 改进
- **优化标签页标题显示与复制 (#228)** / Improve tab title display and copy (#228).
- **欢迎页作为侧栏时隐藏空会话提示** / Hide the empty-session prompt when the welcome page is docked as a sidebar.

### Fixed / 修复
- **修复程序目录配置被卸载删除后连接丢失的问题** / Recover connections when the program-directory config was removed during uninstall/update.
- **修复串口历史输出搜索不到的问题 (#233)** / Fix searching serial scrollback history (#233).
- **修复 Windows 无边框窗口恢复后超出屏幕的问题 (#234)** / Clamp restored frameless windows to the current monitor on Windows (#234).

## [0.5.6] - 2026-07-06

### Fixed / 修复
- **修复分屏终端在右侧资源面板展开时被错误压缩到 10 列** / Avoid transient 10-column terminal resize in split panes.

## [0.5.5] - 2026-07-05

### Added / 新增
- **支持分屏合并 (#216)** / Split-pane merging (#216).
- **支持拖到标签栏合并分屏** / Drag-to-tab-strip pane merge.

### Changed / 改进
- **优化当前标签页识别度 (#200)** / Improve active-tab visibility (#200).
- **标签页关闭按钮固定在右侧** / Pin tab close buttons to the right edge.
- **关闭确认弹窗按钮接入主题色** / Theme the close-confirmation buttons.

## [0.5.4] - 2026-07-04

### Added / 新增
- **SSH 跳板机（堡垒机）支持 (#211)** / SSH jump host (bastion) support (#211).

### Fixed / 修复
- **修复多会话大量输出导致界面卡死 (#209)** / Fix UI freeze on heavy output across multiple sessions (#209).
- **修复欢迎页会话面板首次建会话时高度跳变 (#214)** / Stop the welcome session panel height jump on the first session (#214).

## [0.5.3] - 2026-07-04

### Added / 新增
- **欢迎页侧栏支持四向停靠与持久化** / Four-edge docking and persistence for the welcome sidebar.
- **资源面板收起状态持久化** / Persistent resource-panel collapse state.
- **同边收起图标栏合并与外侧停靠** / Merged outer collapse strips for same-edge panels.

### Fixed / 修复
- **修复欢迎页为侧栏时 tab 栏仍显示“+”的问题** / Hide the tab “+” button when the welcome page is a sidebar.
- **修复启动时同边双面板同时展开的兜底问题** / Prevent same-edge double-expanded panels on startup.

## [0.5.1] - 2026-07-01

### Added / 新增
- **会话备注与快捷命令「点击发送」选项** / Per-session notes and a “send on click” option for quick commands.

## [0.5.0] - 2026-06-30

### Added / 新增
- **分屏(IDEA 式拖动分屏)** / Split panes (IDEA-style drag-to-split).
- **复制打开(再开一个当前会话)** / Duplicate connection.
- **欢迎页可设为侧栏 + IDEA 式收起抽屉** / Welcome page as a sidebar + IDEA-style collapse drawers.
- **每个会话独立记住自己的 SFTP 状态** / Per-session SFTP state.

### Fixed / 修复
- **设置面板字体发虚** / Blurry settings-panel text.

## [0.4.20] - 2026-06-28

### Added / 新增
- **终端内容随窗口缩放重排 (reflow) (#169)** / Terminal content reflows on window resize (#169).
- **alt-screen 里把鼠标滚轮转发给程序 (#170)** / Forward the mouse wheel to alt-screen programs (#170).
- **文本批量导入 SSH 连接 (#150)** / Batch-import SSH connections from text (#150).

## [0.4.19] - 2026-06-28

### Added / 新增
- **macOS 沉浸式标题栏 (#162)** / Immersive title bar on macOS (#162).

### Fixed / 修复
- **修正 macOS 快捷键映射,0.4.18 写反了 (#158)** / Corrected the macOS shortcut mapping that 0.4.18 had backwards (#158).

## [0.4.18] - 2026-06-26

### Added / 新增
- **Windows 11 圆角窗口 + 投影 (#162 / #166)** / Native rounded corners + drop shadow on Windows 11 (#162 / #166).
- **macOS 上 app 快捷键改用 Cmd(⌘),释放 Ctrl 给终端 (#158)** / macOS app shortcuts now use Cmd (⌘), freeing Ctrl for the terminal (#158).

### Fixed / 修复
- **SFTP 闲置一段时间后失效 (#160)** / SFTP stopped working after sitting idle (#160).

## [0.4.17] - 2026-06-24

### Added / 新增
- **MFA / 验证码登录(JumpServer 等强制开启 MFA 的堡垒机)(#86)** / MFA / verification-code login on bastions that force MFA (JumpServer etc.) (#86).
- **纯键盘命令历史检索(#140)** / Keyboard-only command-history search (#140).
- **会话选项:禁用 shell 集成(Windows / pwsh 服务端)(#140)** / Session option: disable shell integration (Windows / pwsh server) (#140).

## [0.4.16] - 2026-06-23

### Added / 新增
- **便携模式:配置改存到程序同目录的 `config/`(#141)** / Portable mode: config moves to a `config/` folder next to the app (#141).
- **终端内查找:Ctrl+F 唤出查找栏** / Find in terminal: Ctrl+F opens the find bar.
- **面板可拖动吸附停靠(资源面板 + SFTP)** / Drag-to-dock panels (resource panel + SFTP).

## [0.4.13] - 2026-06-21

### Added / 新增
- **侧栏可拖动调宽** / Drag-resize the sidebar.
- **进程监视独立窗口** / Detachable process-monitor window.
- **快捷命令支持分组、可收起 (#55)** / Group quick commands, collapsible (#55).

## [0.4.12] - 2026-06-20

### Fixed / 修复
- **修复 macOS 26 界面文字不可见的问题 (#108)** / Fix invisible interface text on macOS 26 (#108).

## [0.4.11] - 2026-06-20

### Added / 新增
- **上传支持取消并清理远端半成品 (#100)** / Cancel an in-progress upload, with remote cleanup (#100).

## [0.4.10] - 2026-06-19

### Added / 新增
- **SFTP 文件多选 + 打包下载 (#100)** / SFTP multi-select with one-archive download (#100).
- **可取消进行中的传输 (#100)** / Cancel an in-progress transfer (#100).
- **端口转发规则可命名 (#100)** / Name port-forward rules (#100).

## [0.4.9] - 2026-06-19

### Added / 新增
- **命令历史下拉支持搜索 (#101)** / Searchable command-history dropdown (#101).
- **命令输入框支持 Readline 快捷键 + 快捷键参考 (#103)** / Readline keys in the command box + shortcuts reference (#103).
- **标签溢出时显示滚动箭头 (#122)** / Scroll arrows when tabs overflow (#122).

## [0.4.8] - 2026-06-18

### Added / 新增
- **沉浸式无边框标题栏** / Immersive frameless title bar.

### Fixed / 修复
- **htop/btop 的线框和盲文字符不再显示为方块** / htop/btop box-drawing and braille no longer render as tofu.
- **连接后不再出现注入的设置命令** / The injected setup line no longer leaks to the terminal on connect.

## [0.4.7] - 2026-06-16

### Added / 新增
- **主机密钥校验 + 首次连接确认弹窗** / Host-key verification with a first-connect confirmation dialog.
- **类 Xshell 的快速连接登录** / Quick-connect login, Xshell-style.
- **终端里直接敲的命令现在也进命令历史** / Commands typed in the terminal now join the command history.

## [0.4.6] - 2026-06-14

### Fixed / 修复
- **会话同步上传现在对「拖拽」也生效** / Session-sync upload now works for drag-and-drop too.

## [0.4.5] - 2026-06-14

### Fixed / 修复
- **会话同步上传改为各会话用自己的当前目录** / Session-sync upload now targets each session's own current directory.

## [0.4.4] - 2026-06-14

### Added / 新增
- **会话同步 / 广播输入** / Session sync / broadcast input.
- **右上角图标悬停提示** / Tooltips on the top-bar icons.

### Fixed / 修复
- **浅色模式对话框** / Light-mode dialogs.

## [0.4.3] - 2026-06-14

### Fixed / 修复
- **终端里的中文(宽字符)对齐到网格** / Wide CJK glyphs are grid-aligned in the terminal.

## [0.4.1] - 2026-06-14

### Added / 新增
- **命令历史的运行 / 复制 / 删除 (#96)** / Run / copy / delete actions on command history (#96).
- **侧栏默认收起设置 (#78)** / Default-collapse settings for the sidebars (#78).

## [0.4.0] - 2026-06-14

### Added / 新增
- **SSH 端口转发 / 隧道 (#56)** / SSH port forwarding / tunnels (#56).
- **快捷命令、命令输入框与历史 (#55)** / Quick commands, command box & history (#55).
- **远端进程监控 (#23)** / Remote process monitor (#23).

## [0.3.8] - 2026-06-12

### Added / 新增
- **有活动会话时关闭前先确认 (#88)** / Confirm before closing when there are active sessions (#88).
- **「总是询问保存去何处」下载选项 (#87)** / "Always ask where to save" download option (#87).
- **下载开始时自动弹出传输面板** / The transfers popup opens automatically when a download starts.

## [0.3.7] - 2026-06-12

### Added / 新增
- **SFTP 文件列表任意处可右键 (#84)** / Right-click anywhere in the SFTP file list (#84).
- **可视化权限对话框 (#84)** / Visual permissions dialog (#84).
- **外部程序查看 / 编辑 (#81)** / Open / Edit externally (#81).

## [0.3.5] - 2026-06-12

### Changed / 改进
- **维护性更新与内部修复** / Maintenance and internal fixes.

## [0.3.3] - 2026-06-11

### Added / 新增
- **应用内新版本通知 (#48)** / In-app new-version notification (#48).
- **可编辑的 SFTP 路径栏,支持复制和粘贴跳转 (#54)** / Editable SFTP path bar with copy & paste-to-jump (#54).

### Fixed / 修复
- **SFTP 面板手动导航后不再卡"加载中" (#59)** / SFTP panel no longer gets stuck "loading" after manual navigation (#59).

## [0.3.2] - 2026-06-11

### Added / 新增
- **Windows MSI 安装包(尽力而为)** / MSI installer for Windows (best-effort).
- **显式会话文件夹 (#41)** / Explicit session folders (#41).

### Fixed / 修复
- **宽(CJK)字符不再导致光标错位 (#60)** / Wide (CJK) characters no longer misalign the cursor (#60).

## [0.3.1] - 2026-06-10

### Security
- **将 sessions.json 限制为仅属主可读(Unix)(#34)** / Restrict sessions.json to owner-only on Unix (#34).

## [0.3.0] - 2026-06-10

### Added / 新增
- **「界面」设置 —— 终端字体与字号** / Interface settings — terminal font & size.
- **会话文件夹 / 分组 (#41)** / Session folders / groups (#41).
- **SFTP 文件夹递归传输 (#50)** / Recursive SFTP folder transfer (#50).

## [0.2.3] - 2026-06-05

### Added / 新增
- **SSH / SFTP 代理支持 (#7)** / Proxy support for SSH / SFTP (#7).
- **从 `~/.ssh/config` 导入主机 (#1)** / Import hosts from `~/.ssh/config` (#1).

### Fixed / 修复
- **修复英文界面下全角 `＋` 显示为豆腐块** / Fix a tofu box for the full-width `＋` in the English interface.

## [0.2.2] - 2026-06-05

### Added / 新增
- **国际化：中 / 英双语，运行时切换 (#9)** / Internationalization: Chinese / English with runtime switching (#9).
- **会话弹窗的私钥文件选择器** / Private-key file picker in the session dialog.
- **Linux 桌面集成** / Linux desktop integration.
