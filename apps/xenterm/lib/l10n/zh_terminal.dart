/// Simplified-Chinese strings for the terminal workspace (terminal view, SFTP
/// view, tools panel and connection prompts).
///
/// Keys are the English source strings; `{placeholders}` carry runtime values.
/// Shared words (Save/Cancel/Delete/Retry/Copy…) stay in `zh_common`.
const Map<String, String> zhTerminal = <String, String>{
  // Connection state and terminal view chrome.
  'Connecting…': '正在连接…',
  'Connected': '已连接',
  'Disconnected': '已断开',
  'Reconnect': '重新连接',
  'Terminal unavailable': '终端不可用',
  'xenterm_bridge library not found in the app bundle':
      '应用包内未找到终端核心库（xenterm_bridge）',

  // Tools panel: search and sections.
  'Search terminal': '搜索终端',
  '1 match in view': '视图中 1 处匹配',
  '{count} matches in view': '视图中 {count} 处匹配',
  'SESSION': '会话',
  'SYSTEM': '系统',
  'SNIPPETS': '命令片段',
  'TERMINAL': '终端',
  'No saved snippets': '暂无已保存片段',
  'Font size': '字号',
  // 'Clear', 'Refresh' and 'Name' are shared UI words already defined in
  // zh_hosts; duplicate keys in the folded translation map are a compile error.

  // Remote resource monitor.
  'Not available on this host': '此主机不支持',
  'No /proc stats from this host': '此主机未提供 /proc 统计',
  'Sampling paused': '采样已暂停',
  'Last sample is stale': '上次采样已过期',
  'Measuring…': '正在采集…',
  'CPU': 'CPU',
  'Memory': '内存',
  'Disk': '磁盘',
  'Network': '网络',
  'GPU: unavailable': 'GPU：不可用',

  // SFTP view.
  'SFTP needs an SSH host.': 'SFTP 需要 SSH 主机。',
  'SFTP unavailable': 'SFTP 不可用',
  'Folders': '文件夹',
  'Parent folder': '上级文件夹',
  'Download {name}': '下载 {name}',
  'Delete {name}': '删除 {name}',
  'Upload a file here': '上传文件到此处',
  'New folder': '新建文件夹',
  'Folder name': '文件夹名称',
  'Create': '创建',
  'Delete “{name}”? This cannot be undone.': '删除“{name}”？此操作无法撤销。',
  'Size': '大小',
  'Modified': '修改时间',
  'Mode': '权限',
  'Loading folder…': '正在加载文件夹…',
  'This folder is empty': '此文件夹为空',
  '{name}: {message}': '{name}：{message}',
  '{name} uploaded': '{name} 已上传',
  '{name} downloaded': '{name} 已下载',
  '{count} active': '{count} 个进行中',
  '{count} failed': '{count} 个失败',
};
