<p align="center">
  <a href="README.md">English</a> · <b>简体中文</b>
</p>

<h1 align="center">XenTerm</h1>

<p align="center">
  <em>面向 Windows / macOS / Linux 的跨平台 SSH / SFTP / Telnet / 串口终端<br>Rust 核心 + Flutter 桌面界面</em>
</p>

<p align="center">
  <a href="https://github.com/ixbaicn/XenTerm/stargazers"><img src="https://img.shields.io/github/stars/ixbaicn/XenTerm?style=flat-square&label=Stars&color=2ea043" alt="Stars"></a>
  <a href="https://github.com/ixbaicn/XenTerm/network/members"><img src="https://img.shields.io/github/forks/ixbaicn/XenTerm?style=flat-square&label=Forks" alt="Forks"></a>
  <a href="https://github.com/ixbaicn/XenTerm/releases"><img src="https://img.shields.io/github/v/release/ixbaicn/XenTerm?style=flat-square&label=Release&color=00b8d4" alt="Release"></a>
  <a href="https://github.com/ixbaicn/XenTerm/releases"><img src="https://img.shields.io/github/downloads/ixbaicn/XenTerm/total?style=flat-square&label=Downloads&color=blueviolet" alt="Downloads"></a>
  <a href="#许可证"><img src="https://img.shields.io/github/license/ixbaicn/XenTerm?style=flat-square&label=License&color=blue" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey?style=flat-square" alt="Platforms: Windows, macOS, Linux">
  <a href="https://github.com/ixbaicn/XenTerm/actions/workflows/ci.yml"><img src="https://github.com/ixbaicn/XenTerm/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
</p>

<p align="center">
  <img src="assets/xt.jpg" alt="XenTerm —— SSH 终端 · SFTP · 系统监控" width="860">
</p>

---

## 简介

XenTerm 是一个面向服务器管理者的桌面终端客户端：SSH、Telnet、串口会话、完整的
VT 终端、SFTP 文件管理与实时资源监控集中在同一个窗口里。

应用分为两层：

- **`crates/xenterm`** —— Rust 服务核心，负责协议（SSH、SFTP、Telnet、串口、终端模拟、
  本地 PTY）、会话存储、资源采样、CLI 与 MCP 服务。
- **`apps/xenterm`** —— Flutter 桌面界面，通过进程内 FFI 桥 **`xenterm-bridge`**
  与核心通信。

核心不依赖任何 UI 工具包，因此桌面应用、命令行和 AI 客户端的连接列表、SSH 实现完全
同源——只需要维护一份服务器清单。

> **独立实现。** XenTerm 是独立项目，界面参考现代深色终端的设计语言并完全自主实现。
> 本项目不包含来自其他终端产品的应用代码、字体或美术资产，与任何其他终端产品均无隶属、
> 认可或派生关系。随包分发的第三方组件（emoji 图形与字体）署名见
> [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

| | |
| --- | --- |
| **会话类型** | SSH、本地 Shell、Telnet、串口 |
| **跳板** | 可排序的 SSH 跳板链，每一跳使用独立凭据 |
| **终端能力** | 完整 VT/ANSI 模拟、DEC 线框字符、按会话设置编码；鼠标追踪与彩色 emoji 由核心实现 |
| **文件** | SFTP 文件面板、上传下载与传输进度条；ZMODEM 由核心实现 |
| **资源监控** | 本机与远端 CPU、内存、交换、磁盘、网络、GPU 实时读数 |
| **自动化** | 面向脚本与 CI 的 `xenterm cli`，面向 AI 客户端的 MCP 服务 |
| **界面** | Flutter + shadcn 风格组件，中性灰单色主题，支持深色 / 浅色 |

## 目录

- [简介](#简介)
- [截图](#截图)
- [安装](#安装)
- [快速开始](#快速开始)
- [功能](#功能)
- [CLI 与 MCP](#cli-与-mcp)
- [配置与数据](#配置与数据)
- [安全说明](#安全说明)
- [技术栈](#技术栈)
- [项目结构](#项目结构)
- [从源码构建](#从源码构建)
- [开发](#开发)
- [发版](#发版)
- [关于提交记录清理的说明](#关于提交记录清理的说明)
- [社区](#社区)
- [参与贡献](#参与贡献)
- [致谢](#致谢)
- [许可证](#许可证)

## 截图

<p align="center">
  <img src="docs/screenshots/01-hosts.png" alt="XenTerm 主页：左侧导航栏与已保存的主机" width="860"><br>
  <em>主页 —— 已保存的主机与分组，左侧是导航栏</em>
</p>

<p align="center">
  <img src="docs/screenshots/02-terminal.png" alt="终端会话，右侧是工具面板与实时系统读数" width="860"><br>
  <em>终端会话——右侧面板提供搜索、快捷命令与实时系统读数</em>
</p>

<p align="center">
  <img src="docs/screenshots/03-settings.png" alt="XenTerm 设置页面" width="860"><br>
  <em>设置 —— 终端、SFTP、日志与快捷键偏好</em>
</p>

## 安装

每次打 `v*` 标签，GitHub Actions 都会构建桌面应用并发布到
[Releases](https://github.com/ixbaicn/XenTerm/releases) 页面。安装包内同时包含
Flutter 界面和完整的 Rust 核心，无需另装任何组件。

### Windows

- **MSI 安装包**（推荐）—— `xenterm-<版本>-windows-x86_64.msi`。按机器安装到
  `Program Files`，创建开始菜单与桌面快捷方式、在 *应用和功能* 中注册卸载入口，
  支持原地升级和静默安装（`msiexec /i <msi> /qn`）。由于程序目录只读，配置保存在 `%APPDATA%`。
- **便携 zip** —— `xenterm-<版本>-windows-x86_64-desktop.zip`。解压到任意位置后运行
  `xenterm.exe`；如果目录可写，配置会保存在可执行文件旁的 `config/` 目录中，适合放在
  U 盘里使用。

> **Windows 代码签名：** XenTerm 已申请 [SignPath Foundation](https://signpath.org)
> 开源项目。在申请获批并启用签名流水线之前，Windows 发布产物暂未签名，SmartScreen
> 可能对 MSI 或 zip 给出未知发布者提示。获批后，符合条件（covered）的 Windows 发布
> 将使用 [SignPath.io](https://signpath.io) 提供的免费代码签名，证书由 SignPath
> Foundation 颁发。

### macOS

下载得到的是包含 `XenTerm.app` 的 `.zip`：

```bash
# aarch64 = Apple 芯片，x86_64 = Intel
unzip xenterm-*-macos-*-desktop.zip

# 可选：移动到「应用程序」（留在原地也能运行）
mv XenTerm.app /Applications/

# 去掉隔离属性，否则 macOS 会提示「XenTerm 已损坏，无法打开」
xattr -dr com.apple.quarantine /Applications/XenTerm.app

open /Applications/XenTerm.app
```

如果没有移动它，把上面两条路径换成 `.app` 实际所在的位置即可。需要 macOS 11 Big Sur
或更高版本。

### Linux

tar 包是自包含的运行目录，系统只需要 GTK 3：

```bash
tar -xzf xenterm-*-linux-x86_64-desktop.tar.gz
cd xenterm-*-linux-x86_64-desktop
./xenterm
```

也可以用随包附带的安装器做托管的用户级安装（wrapper 放到 `~/.local/bin`，为新 shell
管理 PATH，并注册开始菜单与桌面快捷方式）：

```bash
./install-linux.sh --user              # 不调用 sudo
./install-linux.sh --user --uninstall  # 只删除未被修改的安装文件
```

系统级安装为 `sudo ./install-linux.sh --prefix /opt/xenterm`；它不会猜测已登录桌面
用户，因此 PATH 与桌面集成需要显式搭配 `--user`。布局、选项与卸载保证见
[docs/release.md](docs/release.md)。

| 发行版 | 命令 |
| --- | --- |
| Arch（AUR） | `yay -S xenterm-bin`（安装同一个包到 `/opt/xenterm`） |

包内附带 `install-linux.sh`、`xenterm.desktop` 与图标，供用户和发行版注册应用使用。

### 平台支持

| 平台 | 架构 | 安装包 |
| --- | --- | --- |
| Windows 10 / 11 | x86_64 | MSI、便携 zip |
| macOS 11+ | Apple 芯片（aarch64）、Intel（x86_64） | 含 `XenTerm.app` 的 `.zip` |
| Linux | x86_64 | `.tar.gz`、AUR `xenterm-bin` |

iOS 与 Android 已在 Flutter 工程中搭好骨架，但不属于本次桌面发布范围。

## 快速开始

首次启动会创建一个空的会话库。点击 **New host** 添加第一台服务器；主页工具栏的
**Import hosts** 可读取 `~/.ssh/config`，**设置 → Profile data** 可导入 XenTerm 之前的
便携导出文件。CLI 还可以额外导入 FinalShell 连接导出文件。

首次连接某台服务器时会显示它的 SHA-256 主机密钥指纹并要求确认。XenTerm 使用自己的
`known_hosts` 文件记住已信任的密钥；之后密钥发生变化会给出明确警告。

## 功能

> **UI 现状说明。** Rust 核心功能已完整；Flutter 桌面 UI 正在逐步对齐。标注
> **（核心）** 的条目目前可通过核心、CLI 与 MCP 使用，尚未在桌面 UI 中暴露。

### 会话与连接

- **四种会话类型** —— SSH、本地 Shell、Telnet、串口，共用同一个窗口。
- **本地 Shell** —— 主页工具栏的 **Terminal** 动作会在本机打开一个临时终端（系统默认
  Shell），不写入会话库。
- **有序多级跳板** —— 通过已保存的 SSH 跳板主机路由会话。跳板链按“最外层优先”解析，
  每一跳使用各自的凭据；旧的单跳配置继续可用，包括嵌套跳板。循环、重复、缺失的跳板，
  以及超过 16 级的链路会在联网前报错。
- **SSH 认证方式** —— 密码、私钥、带密码短语的私钥，以及用于 2FA / OTP 提示的键盘交互。
- **PuTTY `.ppk` 私钥** —— PPK v2/v3 在内存中解密并校验，无需先用 `puttygen` 转换。
- **串口** —— 可配置波特率、数据位 / 停止位、校验位与流控；对要求输入用户名密码的串口
  控制台，可选自动应答登录提示。
- **Telnet** —— 带 RFC 854/855 选项协商状态机（SGA + NAWS 窗口尺寸同步）。
- **触发器** —— expect/send 规则，用于交互式登录，每条规则可带独立的应答内容，随会话保存。
- **主机分组** —— 新建、重命名、删除分组，并在分组之间移动会话。
- **临时会话** —— 本地 Shell 或串口控制台可以临时打开，不写入会话库。

### 终端

- **完整 VT/ANSI 模拟** —— `htop`、`btop`、`vim`、`tmux` 等全屏程序正常渲染，
  回滚缓冲 10 万行。
- **鼠标追踪（核心）** —— SGR 与 X10 鼠标事件会转发给主动请求的 TUI 程序，因此
  `htop`、`mc` 能响应点击；桌面 UI 暂未转发指针事件。
- **DEC 线框字符** —— 支持 VT100 特殊图形字符集，即使处于 UTF-8 模式，
  `dialog`、`mc` 的边框也能正确绘制。
- **彩色 emoji（核心）** —— 肤色、旗帜以及 ZWJ 组合序列由核心按 Twemoji 图像上色；
  桌面 UI 目前使用系统文本渲染。
- **字符编码** —— 终端按会话解码字符集（默认 UTF-8，也支持 GBK 等），采用流式解码，
  因此一个多字节字符被拆到两个数据包也不会乱码。
- **终端内查找** —— 在当前可见缓冲中搜索并高亮匹配项，显示匹配数量。
- **字体与光标** —— 任意已安装的等宽字体，外加可导入的字体；字号、行距、粗体与内边距
  均可配置。
- **会话日志** —— 可选记录会话转录，并在日志页面查看或删除。

### 文件

- **SFTP 面板** —— 可选择跟随终端的工作目录，带懒加载目录树。
- **上传与下载** —— 单个文件或整个目录树，传输进度显示在底部传输条中。
- **目录管理** —— 新建、删除远端目录与文件。
- **ZMODEM（核心）** —— 远端执行 `sz` 可下载到「下载」目录、执行 `rz` 可经现有 PTY
  上传；传输由核心完成，桌面 UI 暂未提供相应对话框。
- **下载目录** —— 在设置中固定，或每次询问。

### 资源监控

- **实时资源面板** —— 终端右侧显示 CPU、内存、交换、各文件系统用量、网络吞吐与 GPU，
  状态用文字明确标出（`waiting`、`stale`、`unavailable`、`unsupported`），不会隐藏失败。
- **读数来自会话所在主机** —— SSH 会话通过一条辅助 SSH 通道读取服务器指标，每两秒刷新
  一次，不需要二次登录；本地 Shell 则采样本机指标。

### 界面与主题

- **一个窗口、一套工作区** —— 左侧导航栏包含主页、密钥库、隧道、快捷命令、已知主机与
  日志，另有设置页；标题栏标签页承载每个打开的会话。
- **主页** —— 以卡片列出已保存主机，支持搜索、分组、悬停操作，以及可编辑所有连接字段的
  主机详情抽屉。
- **终端工作区** —— 左侧是终端（或 SFTP 视图），右侧工具面板提供搜索、复制、清屏、
  字号、快捷命令、会话状态与实时系统读数；导航栏可收起，把整个宽度让给终端。
- **单色主题** —— 黑底 + 12 阶中性灰 + 白色强调，另有浅色主题；图标与控件来自同一套
  组件，所有页面看起来属于同一个产品。
- **快捷键** —— 见 设置 → 快捷键。

## CLI 与 MCP

CLI 与 MCP 服务位于 Rust 核心中，和桌面应用共用同一份会话库，因此只需要维护一份服务器
清单。CLI 适合脚本、CI 和明确的手动命令；MCP 让 AI 客户端用自然语言完成服务器巡检、
日志分析和文件传输。

在仓库根目录构建核心二进制（桌面包里核心是桥接库，不包含独立可执行文件）：

```bash
# 桌面端自动化，可访问操作系统钥匙串
cargo build --locked --release -p xenterm

# 无界面 / 服务器部署：不依赖系统钥匙串后端
cargo build --locked --release -p xenterm --no-default-features --features headless
```

> 使用前请先在桌面应用中成功连接一次目标会话，以完成主机密钥确认。密码、私钥等凭据
> 不会出现在 CLI / MCP 的输出中——也不要把明文密码写进提示词或 MCP 配置文件。

### CLI

```bash
xenterm cli help
```

```bash
# 列出已保存的会话，第一列是后续命令要用的 session-id
xenterm cli sessions
xenterm cli sessions --json

# 查看单个会话的非敏感信息
xenterm cli session <session-id>

# 执行非交互式命令，远端命令放在 -- 之后
xenterm cli exec <session-id> -- free -h
xenterm cli exec <session-id> --timeout 60 --json -- journalctl -n 100 --no-pager

# 浏览、读取和传输远端文件
xenterm cli files <session-id> /var/log
xenterm cli read <session-id> /var/log/example.log
xenterm cli upload <session-id> ./local.txt /tmp
xenterm cli download <session-id> /tmp/result.txt ./downloads

# 将已保存会话导出/导入为便携私有文件
xenterm cli export 新导出.json --include-credentials
xenterm cli import finalshell导出.json --dry-run
xenterm cli sync-native 快照.json --dry-run

# 读取或设置无人值守访问模式（restricted | full）
xenterm cli permissions
xenterm cli permissions restricted
```

下载要求本地目标目录已经存在，且不会覆盖同名文件。

### MCP

MCP 服务通过本机 stdio 暴露同一批会话。MCP 默认关闭，且每项能力——使用已保存凭据、
执行任意 SSH 命令、传输文件——都有独立开关，按客户端实际需要逐项开启；无人值守访问模式
可用 `xenterm cli permissions` 读取或修改，凭据永远不会返回给客户端。

在 MCP 客户端里注册 stdio 服务：

```json
{
  "mcpServers": {
    "xenterm": {
      "command": "/absolute/path/to/xenterm",
      "args": ["mcp", "serve"]
    }
  }
}
```

Windows 下 `command` 可以填 `C:\\path\\to\\xenterm.exe`。重启或刷新客户端后，应当能看到
名为 `xenterm` 的服务，提供九个工具：`list_sessions`、`get_session`、`run_command`、
`list_remote_files`、`read_remote_text_file`、`upload_file`、`download_file`、
`get_access_policy`、`import_sessions`。不同 AI 客户端的 MCP 配置文件位置不同，请以对应
客户端的文档为准。

例如做一次只读的 OOM 诊断：客户端先列会话，再执行 `free -h`、检查内核 OOM 记录，最后
浏览堆转储目录——全部在已授予的权限范围内。若同一主机有多条会话，请在提示词里写明会话
名称。

远程（HTTP）MCP 是可选的进阶模式：它校验来自外部身份提供方的 OAuth 访问令牌，并部署在
你自己的 HTTPS 反向代理之后；构建本身不会创建任何公网监听、账号或部署。部署模板、同域名
多路径与验证步骤见 [远程 MCP 部署与验证](docs/REMOTE_MCP.md)。

## 配置与数据

XenTerm 是 **便携优先** 的：如果可执行文件旁边有可写的 `config/` 目录（U 盘、解压出来的
tar 包），所有数据都放在那里。否则回退到系统的用户配置目录：

| 平台 | 位置 |
| --- | --- |
| Windows | `%APPDATA%\xenterm\XenTerm\config` |
| Linux | `~/.config/xenterm` |
| macOS | `~/Library/Application Support/dev.xenterm.XenTerm` |

所有用户数据都在同一个目录里：

- `sessions.db` —— 会话库（SQLite，WAL 模式）
- `secret.key` —— 系统钥匙串不可用时使用的每份安装独立密钥
- `known_hosts` —— XenTerm 自己信任的主机密钥（自有格式，不是 OpenSSH 的 `known_hosts`）
- `log/` —— 诊断日志，位于配置目录旁边

旧的 `sessions.json` 会被一次性导入，并保留为 `sessions.json.migrated`；已存在的文件
永远不会被覆盖。服务与自动化场景可用 `--data-dir <路径>` 或 `XENTERM_DATA_DIR` 在打开
会话库前固定一个隔离的独立配置；多个写者会检测过期快照而不是覆盖更新的保存。详见
[配置一致性](docs/profile-consistency.md) 与 [配置导入](docs/config-import.md)。

## 安全说明

- **密码静态存储** —— 只要系统支持，会话密码就存进操作系统钥匙串（Windows 凭据管理器、
  macOS 钥匙串、Linux Secret Service）。不可用时改用 **ChaCha20-Poly1305** 加密，
  密钥是每份安装独立的 `secret.key`，因此 `sessions.db` 里不会出现明文密码。
- **导出的连接列表是可移植的，但不是机密的。** 导出格式使用编进二进制的固定密钥，
  以便文件在任何机器上都能打开——这是混淆，不应把它当作加密保护。请妥善保管导出文件。
- **主机密钥** 由 XenTerm 自己记录在 `known_hosts` 文件里（格式为
  `host:port <密钥类型> <base64>`），这与 OpenSSH 的 `known_hosts` 不是同一个文件。
  未知密钥会先要求确认，密钥发生变化会产生明确警告。
- **凭据在释放时清零**，明文密码不会残留在已释放的内存里。
- **MCP 默认关闭并逐项授权** —— MCP 服务与每项能力默认都不开启，凭据不会返回给客户端，
  命令与文件访问都需要显式授权。

## 技术栈

| 层次 | 选型 |
| --- | --- |
| 界面 | Flutter + [shadcn_flutter](https://pub.dev/packages/shadcn_flutter) 组件 |
| 桥接 | 手写 C ABI + [`dart:ffi`](https://dart.dev/interop/c-interop)（`xenterm-bridge`） |
| 异步运行时 | [`tokio`](https://tokio.rs) |
| SSH / SFTP | [`russh`](https://crates.io/crates/russh) + [`russh-sftp`](https://crates.io/crates/russh-sftp)，纯 Rust，不依赖 libssh |
| 终端模拟 | [`rio-vt`](https://crates.io/crates/rio-vt) 原生网格、滚动历史与缩放重排 |
| 本地 PTY | [`portable-pty`](https://crates.io/crates/portable-pty) |
| 串口 | [`serialport`](https://crates.io/crates/serialport) |
| 系统指标 | [`sysinfo`](https://crates.io/crates/sysinfo) |
| 存储 | [`rusqlite`](https://crates.io/crates/rusqlite)（内嵌 SQLite）、[`keyring`](https://crates.io/crates/keyring)、[`chacha20poly1305`](https://crates.io/crates/chacha20poly1305) |
| MCP | [`rmcp`](https://crates.io/crates/rmcp) + [`axum`](https://crates.io/crates/axum) |
| 序列化 / 日志 | `serde`、`serde_json`、`tracing` |

## 项目结构

```
XenTerm/
├── crates/
│   └── xenterm/                 # Rust 服务核心
│       ├── src/
│       │   ├── app/             # 会话运行时与 PTY 泵
│       │   ├── automation/      # 无人值守访问策略
│       │   ├── cli/             # `xenterm cli`
│       │   ├── config/          # sessions.db、凭据、导入导出、配置锁
│       │   ├── core/            # 与界面工具包无关、各入口共用的状态
│       │   ├── mcp/             # MCP 服务（stdio 与可选的认证 HTTP）
│       │   ├── resource/        # 本机采样与远端指标解析
│       │   ├── session/         # SSH / 本地 / Telnet / 串口的连接生命周期
│       │   ├── ssh/  sftp/  tunnel/
│       │   ├── terminal/        # VT 引擎、字符集、渲染模型、ZMODEM
│       │   ├── webdav/          # 手动同步会话
│       │   └── frontend.rs      # 供桥接层使用的会话门面
│       ├── tests/               # Rust 与 Python 端到端测试
│       └── build.rs             # 版本注入与 Windows 图标嵌入
├── apps/
│   └── xenterm/                 # Flutter 桌面界面
│       ├── lib/                 # 页面、外壳、终端视图、主题与组件
│       ├── native/              # xenterm-bridge：核心之上的 C ABI
│       ├── tool/                # 构建、放置、打包与安装包脚本
│       └── windows/ linux/ macos/ ios/ android/   # Flutter 各平台运行器
├── assets/                      # 图标、内置字体与桌面项
├── packaging/                   # AUR 配方、Windows MSI 定义与 remote-MCP 示例
├── scripts/                     # version.sh、release.ps1、make_icons.py、install-wix.ps1
├── docs/                        # 设计说明与运维文档
└── .github/workflows/           # CI、发布与 AUR 发布流水线
```

## 从源码构建

环境要求：

- **Rust** 1.88 或更高版本（工作区在 `Cargo.toml` 中约束各成员）
- **Flutter** 3.47 或更高版本，stable 通道（CI 使用 3.47.7）
- 平台工具链：Windows 需要 Visual Studio C++ 生成工具，macOS 需要 Xcode，
  Linux 需要 GTK 开发包

Debian/Ubuntu 上的 Linux 构建需要：

```bash
sudo apt update
sudo apt install -y --no-install-recommends \
  build-essential pkg-config cmake ninja-build clang \
  libgtk-3-dev liblzma-dev libstdc++-12-dev \
  libudev-dev libdbus-1-dev
```

先构建桥接库，再构建 Flutter 应用：

```bash
cargo build --release --locked -p xenterm-bridge

cd apps/xenterm
flutter pub get
flutter build windows --release     # 或 linux / macos
```

`apps/xenterm/tool/bundle_native.sh` 会把 bridge 拷到构建产物旁
（Windows 是 `build/windows/x64/runner/Release`）。构建后执行一次；开发时
`flutter run` 前同样需要先执行：

```bash
bash apps/xenterm/tool/bundle_native.sh
```

`apps/xenterm/tool/package_desktop.sh <版本>` 生成流水线使用的发布压缩包。

## 开发

贡献前请阅读[社区贡献指南](CONTRIBUTING.zh-CN.md)（[English](CONTRIBUTING.md)），了解
PR 范围、冲突处理、审核就绪与 API 兼容性核验要求。

- **界面只有 Flutter。** 核心必须保持不依赖任何 UI 工具包：它既作为库链接进桌面应用，
  也在服务器上编译为独立 CLI。
- **桥接层只做翻译。** `apps/xenterm/native/` 负责把核心类型映射成 Dart 侧使用的 JSON
  与句柄；行为逻辑属于 `crates/xenterm`。
- **核心按功能分包。** 每个模块把数据结构放在 `struct/`、实现放在 `impls/`，并通过
  `mod.rs` 暴露一个小门面。
- 在仓库根目录常用的检查命令：

  ```bash
  cargo check --locked
  cargo test --locked
  cargo test --locked --no-default-features --features headless
  cd apps/xenterm && flutter pub get && flutter analyze && flutter test
  ```

- `crates/xenterm/tests/` 下的 Python 测试套件覆盖 CLI、MCP、导入导出与 SSH 跳板链的
  端到端行为；运维细节见 `docs/`。

## 发版

不要手动改 Cargo 版本号再打标签。请使用发布脚本，让标签指向的提交本身就已经包含匹配的
版本号：

```powershell
.\scripts\release.ps1 v0.8.0 -Push
```

发布流水线会校验标签与 `crates/xenterm/Cargo.toml`、`Cargo.lock` 中的版本一致，构建
Windows、macOS（Apple 芯片与 Intel）与 Linux 的桌面安装包，并一次性附加到 GitHub
Release。完整流程见 [docs/release.md](docs/release.md)，版本号规范见
[docs/versioning.md](docs/versioning.md)。

## 关于提交记录清理的说明

2026 年 10 月，维护者重建了本仓库的提交历史，并以全新项目的面貌发布 XenTerm。
这是因为界面层（替换为 Flutter 桌面 UI）与 Rust 核心层都经历了大规模重构，原有历史
已无法准确描述当前仓库中的代码。

由此带来的影响是：2026 年 10 月之前的提交链接、blame 信息，以及对旧提交或 PR 的引用
将不再有效。我们对此给引用过这些链接的读者带来的不便表示歉意。重构之前的版本说明仍
保留在 [CHANGELOG](CHANGELOG.md) 中；源代码、许可证与功能不受影响。

## 社区

使用问题、Bug 反馈与功能建议欢迎提交到
[GitHub Issues](https://github.com/ixbaicn/XenTerm/issues)。中文用户也可以加入 QQ 群：

<p align="center">
  <img src="docs/QR/QQ_Group_QR_Code.jpg" alt="QQ 群二维码" width="180"><br>
  <em>扫码加入 QQ 群，交流使用经验、反馈问题或获取最新动态</em>
</p>

## 参与贡献

欢迎参与贡献。请先阅读[社区贡献指南](CONTRIBUTING.zh-CN.md)，其中说明了 PR 流程、
验证命令与审核预期；范围小、聚焦的 PR 最容易审核。社区遵循
[行为准则](CODE_OF_CONDUCT.md)；安全问题请按 [SECURITY.md](SECURITY.md) 的说明私下
报告，不要发在公开 issue 里。

## 致谢

- 界面层：[Flutter](https://flutter.dev) 与 [shadcn_flutter](https://pub.dev/packages/shadcn_flutter)
- 图标：[Lucide](https://lucide.dev) 与 [Simple Icons](https://simpleicons.org)
- Rust 核心：[rio-vt](https://crates.io/crates/rio-vt)、[russh](https://crates.io/crates/russh)、
  [russh-sftp](https://crates.io/crates/russh-sftp)、[portable-pty](https://crates.io/crates/portable-pty)、
  [serialport](https://crates.io/crates/serialport)、[sysinfo](https://crates.io/crates/sysinfo)、
  [tokio](https://tokio.rs)、[rusqlite](https://crates.io/crates/rusqlite) 与 [rmcp](https://crates.io/crates/rmcp)
- 彩色 emoji 图形：[Twemoji](https://github.com/jdecked/twemoji)
- 内置字体 Meatshell Mono、MiSans、HarmonyOS Sans，署名见
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)
- [MeatShell](https://github.com/yituorou/meatshell) 项目：XenTerm 为兼容性保留了它的
  便携导出格式与配置迁移路径

## 许可证

基于 [MIT 许可证](LICENSE) 开源。贡献代码同样按该协议授权。

第三方署名（包括彩色 emoji 使用的 Twemoji 图形与内置字体）见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
