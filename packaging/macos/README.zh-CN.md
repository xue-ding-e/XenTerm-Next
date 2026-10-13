# macOS 可选命令行入口安装包

保留现有便携 ZIP；另提供 Apple Installer `.pkg`。构建输入是现有流水线已经组装的 `xenterm.app`，不重复编译应用。

```sh
bash packaging/macos/build-pkg.sh xenterm.app 0.4.2 xenterm-macos-aarch64.pkg
```

## 安装行为

- 应用必选，安装到 `/Applications/xenterm.app`，可在访达及系统应用启动器打开。
- `添加 xenterm 命令` 默认勾选，可在安装器“自定”中取消；链接为 `/usr/local/bin/xenterm` → `/Applications/xenterm.app/Contents/MacOS/xenterm`。
- 使用系统常规命令目录，不改 `.zshrc`、`.bashrc`、系统 PATH 文件。若用户自行删掉 PATH 中的 `/usr/local/bin`，需自行恢复该目录或用应用的绝对路径。
- 不定位登录用户、不把快捷方式写到 root 桌面、不修改 Dock。macOS 没有 Windows 开始菜单；安装到 Applications 是其原生入口。
- 不覆盖其他程序占用的同名文件或不同目标的符号链接；遇到冲突会报错。应用与 CLI 是两个组件，CLI 失败不代表应用组件自动回滚。
- 升级会保留已经建立的 CLI 链接。每次安装界面仍默认勾选；取消只跳过 CLI 组件，不删除已有入口。**尚未实现跨升级保存“取消勾选”偏好**，不能宣称已实现该行为。
- 输入必须为正确 bundle ID、固定可执行文件名称及可执行权限的应用；组件禁止 bundle 自动重定位，避免升级误写其他位置的同名应用。

## 安全卸载

移除 CLI：检查仓库脚本后运行 `sudo sh packaging/macos/uninstall-cli.sh`。它只删除完全匹配的符号链接和 CLI 安装收据；不会删应用、连接、密钥或用户数据。

卸载应用请在访达将 `/Applications/xenterm.app` 移到废纸篓。脚本不使用对应用目录的递归删除，不猜测用户数据位置。

## 签名与验证边界

默认生成**未签名 installer**；现有 app 的 ad-hoc 签名不等于 Developer ID 或公证。若发布者已自行准备有效 Developer ID Installer 证书，可显式设置 `XENTERM_INSTALLER_SIGN_IDENTITY`，脚本将传给 `productbuild --sign`。不申请凭据、不做公证、不修改 Gatekeeper、不绕过系统安全提示。

Linux 可运行结构和隔离脚本测试：

```sh
python3 -m unittest discover -s tests -p 'macos_installer_test.py'
```

macOS CI 需执行真实 Apple 工具打包、展开、选择项检查（**不会安装到主机**）：

```sh
bash tests/macos_installer_build.sh
```

这不等于真实 Installer GUI、实际升级/卸载、签名/公证验收。该部分需要有 macOS 的维护者测试。

## 设计依据

- [Apple Distribution XML](https://developer.apple.com/library/archive/documentation/DeveloperTools/Reference/DistributionDefinitionRef/Chapters/Distribution_XML_Ref.html)：`start_selected` 是初始布尔值；可交互 choice 不设置持续 `selected` 表达式，避免覆盖用户操作。
- [Apple 打包分发](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution)：使用 `pkgbuild` / `productbuild`，签名与公证为独立发布步骤。
