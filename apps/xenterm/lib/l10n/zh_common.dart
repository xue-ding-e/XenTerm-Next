/// Shared Chinese strings: navigation rail, Settings screen, About and the
/// controls reused across screens. Screen-group translations live in the
/// sibling `zh_*.dart` files.
const Map<String, String> zhCommon = <String, String>{
  // Navigation rail and window-level titles.
  'Home': '首页',
  'Keychain': '密钥管理',
  'Tunnels': '隧道与跳板',
  'Snippets': '命令片段',
  'Known Hosts': '已知主机',
  'Logs': '日志',
  'Settings': '设置',

  // Settings tabs and language picker.
  'Terminal': '终端',
  'SFTP': 'SFTP',
  'Shortcuts': '快捷键',
  'About': '关于',
  'Language': '语言',
  'Interface language': '界面语言',

  // Terminal tab.
  'Terminal settings': '终端设置',
  'Cursor style': '光标样式',
  'Block': '方块',
  'Bar': '竖线',
  'Underline': '下划线',
  'Line spacing': '行间距',
  'Compact': '紧凑',
  'Normal': '正常',
  'Relaxed': '宽松',
  'Inset terminal text': '终端文字内边距',
  'Keep a small margin around the character grid': '在字符网格周围留出少量空白',
  'Use a bold face for terminal text': '终端文字使用粗体',
  'Font': '字体',
  'Default': '默认',
  'Text Size': '文字大小',
  'Terminal theme': '终端主题',
  'Theme': '主题',
  'Dark': '深色',
  'Light': '浅色',
  'Interface': '界面',
  'Import font (TTF/OTF)': '导入字体 (TTF/OTF)',
  "Imported fonts are stored in this device's app data.": '导入的字体保存在本机应用数据中。',

  // SFTP tab.
  'SFTP settings': 'SFTP 设置',
  'Follow the terminal directory': '跟随终端目录',
  "Open the file browser at the shell's current folder when supported":
      '支持时在终端当前目录打开文件浏览器',
  'Download folder': '下载文件夹',
  'Ask for a folder each time': '每次询问文件夹',
  'Clear download folder': '清除下载文件夹',
  'Downloaded files are written to this folder; the picker appears when no folder is set.':
      '下载文件将保存到此文件夹；未设置时会显示文件夹选择器。',

  // Logs tab.
  'Session logs': '会话日志',
  'Save session logs': '保存会话日志',
  'Record new terminal sessions as text transcripts. Existing sessions are not recorded retroactively.':
      '将新终端会话记录为文字日志，已有会话不会追溯记录。',
  'Transcripts are stored in the app data folder and appear in Logs, where they can be opened or deleted. Nothing is uploaded.':
      '会话记录保存在本机应用数据文件夹，可在日志中查看或删除；不会上传。',
  'Profile data': '配置数据',
  'Export sessions (JSON)': '导出会话 (JSON)',
  'Import sessions (JSON)': '导入会话 (JSON)',
  'Exports carry every saved host; secrets are written in the portable encrypted form. Importing never overwrites existing sessions — new ones are appended.':
      '导出包含全部已保存主机，密钥使用可移植加密形式写入；导入不会覆盖已有会话，只追加新会话。',

  // Shortcuts tab.
  'Keyboard shortcuts': '键盘快捷键',
  'Search the terminal buffer': '搜索终端缓冲区',
  'Clear the screen (sent to the shell)': '清屏（发送到 shell）',
  'Interrupt the running command': '中断正在运行的命令',
  'Backtab': '反向切换',
  'Move through the terminal buffer': '浏览终端缓冲区',
  'Run the command': '运行命令',

  // Shared controls.
  'Choose…': '选择…',
  'Remove host': '删除主机',
  'Cancel': '取消',
  'Delete': '删除',
  'Save': '保存',
  'Retry': '重试',
  'Close': '关闭',
  'Copy': '复制',
  'Copied': '已复制',

  // About tab.
  'A lightweight SSH / SFTP / Telnet / serial terminal with a Rust core and a Flutter desktop UI.':
      '轻量级 SSH / SFTP / Telnet / 串口终端，Rust 核心 + Flutter 桌面界面。',
  'Features': '功能特性',
  'Developer & credits': '开发者与鸣谢',
  'Maintained by 小白 and the XenTerm contributors.': '由 小白 与 XenTerm 贡献者共同维护。',
  'Thanks to xue-ding-e and noerror6 for their contributions, and to the open-source projects this app is built on: Flutter, shadcn_flutter, Rust, russh, rio-vt, Lucide, Simple Icons and Twemoji.':
      '感谢 xue-ding-e 与 noerror6 的贡献，以及本项目所依赖的开源项目：Flutter、shadcn_flutter、Rust、russh、rio-vt、Lucide、Simple Icons 与 Twemoji。',
  'Telnet / Serial': 'Telnet / 串口',
  'File browsing and transfers': '文件浏览与传输',
  'Network devices and serial consoles': '网络设备与串口控制台',
  'Tunnels and jump hosts': '隧道与多级跳板',
  'Automation without a JVM': '无需 JVM 的自动化接口',
  'Links': '相关链接',
  'Source code': '源代码',
  'Issue tracker': '问题反馈',
  'Releases': '版本发布',
  'License': '许可证',
  'Tap a link to copy it to the clipboard.': '点击链接即可复制到剪贴板。',
  'Exported 1 session to {file}': '已导出 1 个会话到 {file}',
  'Exported {count} sessions to {file}': '已导出 {count} 个会话到 {file}',
  'Imported 1 session ({skipped} already present, skipped)':
      '已导入 1 个会话（{skipped} 个已存在，已跳过）',
  'Imported {count} sessions ({skipped} already present, skipped)':
      '已导入 {count} 个会话（{skipped} 个已存在，已跳过）',
};
