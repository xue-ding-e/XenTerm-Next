# Flutter 前端与 Rust 核心的 FFI 桥

本文描述桌面界面（`apps/xenterm`）与 Rust 服务核心（`crates/xenterm`）之间的桥接
方式，以及新增桥接接口时的约定。

## 决策：进程内 FFI

iOS 不允许应用派生子进程，因此“Rust 侧车进程 + JSON-RPC”无法覆盖全部目标平台；唯一
全平台可行的形态是**进程内 FFI**。当前实现是手写的 C ABI + `dart:ffi` 直连：接口少，
每一处都能逐条审计，不引入代码生成和第二条异步栈。等 API 面稳定后，可以再评估
[flutter_rust_bridge](https://pub.dev/packages/flutter_rust_bridge) 生成类型绑定。

## 组成

```
apps/xenterm/
  lib/bridge/native_bridge.dart   # Dart 侧：加载动态库、类型投影、工作 isolate
  native/src/lib.rs               # xenterm-bridge：唯一对外暴露的 C ABI
  tool/build_native.ps1|.sh       # 构建 bridge 并放到 Flutter 产物旁
  tool/bundle_native.sh           # 发布/运行前放置 bridge 的通用步骤
crates/xenterm/
  src/frontend.rs                 # 核心公开的会话门面（启动、轮询、输入、关闭）
  src/config/                     # 会话库、凭据、导入导出
```

桥接 crate 只做“翻译”：把核心的模型（`Session`、`TermBuffer`、`ResourceSample`、
`Transfer`…）映射为 JSON 或句柄，把 Dart 的输入还原成核心调用。核心 crate 保持零前端
依赖；`arch_guards` 测试继续约束这一点。

## 数据通道约定

- **字符串**：所有结构以 UTF-8 JSON 的 C 字符串跨边界；内存由调用方持有，Dart 侧读取后
  必须调用 `xenterm_string_free` 释放。入参同样由 Dart 分配并以 NUL 结尾。
- **返回值**：写操作返回 `{"ok":true,"id":"…"}` 或 `{"ok":false,"error":"…"}`；Dart 侧
  在 `ok` 为假时抛 `NativeBridgeError`，由页面显示错误与重试。
- **凭据**：密码、私钥内容、代理凭据不跨桥；会话投影只包含 `has_password` 之类的布尔
  标记，密钥只投影路径、算法、指纹与用途。
- **句柄**：活动会话用 `u64` 句柄标识，由 bridge 维护的映射表分配，Dart 不解释其含义。

## 终端会话流程

1. `xenterm_session_start(id, cols, rows)` 或 `xenterm_session_start_adhoc(json)`
   （本地 Shell、串口临时会话，不写会话库）启动连接，返回句柄。
2. Dart 侧每 50 ms 读取一次 `xenterm_session_version(handle)`；版本号变化时才调用
   `xenterm_session_snapshot(handle)` 取紧凑快照（`rows[].runs[{t,c,s}]`、光标、模式位、
   状态、提示），避免把整屏数据每帧搬运一次。
3. 键盘输入按 xterm 约定编码后经 `xenterm_session_input` 送入 PTY；缩放同时调整
   rio-vt 屏幕和传输端（`xenterm_session_resize`）。
4. 主机密钥、账号密码、MFA 等连接期提示通过快照回传，界面作答后调用
   `xenterm_session_answer_prompt` 唤醒被暂停的认证流程。
5. `xenterm_session_close(handle)` 结束会话并释放缓冲。

SFTP 是同一模式的第二条通道：`xenterm_sftp_start/version/snapshot/list/refresh/
download/upload/mkdir/delete/toggle_tree/close/answer_prompt`，目录树与传输进度都在
快照里，界面只负责展示与发起操作。

## 线程与阻塞

- `ConfigStore::load` 会触达系统钥匙串和 SQLite，所有会话库、设置、导入导出、已知主机、
  密钥、日志等读写都在 `Isolate.run` 中执行，不阻塞 UI 线程。
- 会话的传输与泵线程运行在 bridge 持有的 tokio runtime 上（与核心使用同一 tokio 版本）。
- 轮询只读取版本计数器，界面刷新不产生跨线程回调风暴。

## 配置与数据目录

bridge 在进程内只初始化一次核心存储。若启动时设置了 `XENTERM_DATA_DIR` 或
`--data-dir`，bridge 会在第一次读会话库/已知主机之前固定该显式配置目录；未设置时按
“便携优先 → 系统配置目录”解析，与 CLI 的启动顺序一致。

## 构建与放置

```bash
cargo build --release -p xenterm-bridge
bash apps/xenterm/tool/bundle_native.sh
```

- Windows 产物为 `xenterm_bridge.dll`，macOS/Linux 为 `.dylib`/`.so`；发布包中它必须与
  Flutter 可执行文件位于同一目录（macOS 在 App bundle 的 `Contents/Frameworks`）。
- 开发时可用环境变量 `XENTERM_BRIDGE_PATH` 覆盖查找路径。
- 找不到 bridge 时，Dart 侧抛出明确错误，界面显示错误页而不是静默失败。

## 测试

```bash
cargo test -p xenterm-bridge        # bridge 的会话、SFTP、导入导出、设置等契约
cd apps/xenterm && flutter test     # 终端渲染、页面与组件契约
```

widget 测试使用注入的样本数据与 loader（例如 `kSampleHosts`、假的串口枚举），生产
数据通过真实 bridge 读取；两者不混用。

## 修改桥接接口时的检查清单

- 新能力是否能放进核心的现有门面（`frontend.rs`）而不是复制核心逻辑？
- 返回的数据是否只包含界面需要的字段？凭据是否仍然不跨桥？
- 失败是否返回结构化错误，而不是 panic 或空字符串？
- 是否需要 `Isolate.run`（触达磁盘、钥匙串、SQLite）？
- 是否同时更新了 `native_bridge.dart` 的 DTO、页面调用和 `cargo test -p xenterm-bridge`
  的契约测试？
