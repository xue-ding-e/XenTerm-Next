/// Simplified-Chinese strings for the keychain / known-hosts / logs / snippets /
/// port-forwarding screens and the shared connect-time prompt.
///
/// Keys are the exact English source literals; `{placeholders}` mirror the
/// arguments passed to `AppLanguage.tr`. Words already covered by
/// `zh_common.dart` (Save/Cancel/Delete/Retry/Close/…) are not repeated here.
const Map<String, String> zhScreens = <String, String>{
  // Shared within this group's screens.
  'No matches': '无匹配结果',
  'Try a different search.': '请尝试其他搜索关键词。',

  // Keychain screen.
  'Search keys': '搜索密钥',
  'Loading keys…': '正在加载密钥…',
  'No SSH keys yet': '暂无 SSH 密钥',
  'Set a private key path on a host to see it here.': '在主机上设置私钥路径后即可在此查看。',
  'Path to the private key': '私钥路径',
  'Remove “{name}” from its hosts? The key file stays on disk.':
      '是否从使用它的主机中移除“{name}”？密钥文件仍保留在磁盘上。',
  'Remove': '移除',
  'Key file missing': '密钥文件缺失',
  'Encrypted key · unlocked on connect': '加密密钥 · 连接时解锁',
  'Key': '密钥',
  'Pasted key': '粘贴的密钥',
  'Key file': '密钥文件',
  'Edit key path': '编辑密钥路径',
  'Remove key reference': '移除密钥引用',
  'Copy key file name': '复制密钥文件名',
  'Enter the private key path': '请输入私钥路径',
  'Could not update the key': '无法更新密钥',
  'Key path updated': '密钥路径已更新',
  'Could not remove the key': '无法移除密钥',
  'Key removed from its hosts': '已从使用它的主机中移除密钥',

  // Known hosts screen.
  'Search known hosts': '搜索已知主机',
  'Loading known hosts…': '正在加载已知主机…',
  'No known hosts yet': '暂无已知主机',
  'Host keys are remembered after you accept them on connect.':
      '连接时接受主机密钥后即会记住。',
  'unreadable key entry': '无法读取的密钥记录',
  'Forget this key': '忘记此密钥',

  // Logs screen.
  'Delete the only session log? This cannot be undone.': '删除唯一的会话日志？此操作无法撤销。',
  'Delete all {count} session logs? This cannot be undone.':
      '删除全部 {count} 条会话日志？此操作无法撤销。',
  'Delete all': '全部删除',
  'Loading session logs…': '正在加载会话日志…',
  'No session logs yet': '暂无会话日志',
  'Enable “Save session logs” in Settings → Logs to record new sessions.':
      '在“设置 → 日志”中开启“保存会话日志”，即可记录新的会话。',
  'Delete log': '删除日志',
  'Copy transcript': '复制会话记录',
  'Close viewer': '关闭查看器',
  'Opening transcript…': '正在打开会话记录…',
  'This session produced no terminal output.': '该会话没有产生终端输出。',
  'Session closed: {reason}': '会话已关闭：{reason}',
  'Could not delete the log': '无法删除日志',

  // Snippets screen.
  'New snippet': '新建片段',
  'Search snippets': '搜索片段',
  'Loading snippets…': '正在加载片段…',
  'No snippets yet': '暂无片段',
  'Save commands here; the connected terminal sidebar runs them.':
      '在此保存命令，已连接终端的侧边栏即可运行它们。',
  'Snippet': '片段',
  'Run': '运行',
  'Edit': '编辑',
  'Delete snippet': '删除片段',
  'Save snippet': '保存片段',
  'Group (optional)': '分组（可选）',
  'Command': '命令',
  'Press Enter after running': '运行后按回车',
  'The command cannot be empty.': '命令不能为空。',

  // Port-forwarding screen.
  'Tunnels & Jumps': '隧道与跳板',
  'Port forwards and the jump hosts their sessions use.': '端口转发及其会话使用的跳板主机。',
  'New rule': '新建规则',
  'Search rules': '搜索规则',
  'Add an SSH host before creating a rule.': '请先添加 SSH 主机再创建规则。',
  'Loading port forwards…': '正在加载端口转发…',
  'Active tunnels': '活动隧道',
  'Port forwarding': '端口转发',
  'No forwarding rules': '暂无转发规则',
  'Rules open their tunnels when the owning host connects.':
      '规则会在所属主机连接时打开对应隧道。',
  'Jump chains': '跳板链',
  'No jump hosts configured': '尚未配置跳板主机',
  'Pick a jump host under a host\'s SSH details to route it through another server.':
      '在主机的 SSH 详情中选择跳板主机，即可经由另一台服务器转发。',
  'Local': '本地',
  'Remote': '远程',
  'Dynamic': '动态',
  'Dynamic (SOCKS5)': '动态 (SOCKS5)',
  'via 1 jump': '经 1 个跳板',
  'via {count} jumps': '经 {count} 个跳板',
  '{kind} tunnel': '{kind}隧道',
  'Edit rule': '编辑规则',
  'Delete rule': '删除规则',
  'Forwarding rule': '转发规则',
  'Save rule': '保存规则',
  'Host': '主机',
  'Type': '类型',
  'Name (optional)': '名称（可选）',
  'Listen address': '监听地址',
  'Listen port': '监听端口',
  'Target host': '目标主机',
  'Target port': '目标端口',
  'Enter a valid listen port (1-65535).': '请输入有效的监听端口（1-65535）。',
  'Enter a target host and a valid target port.': '请输入目标主机和有效的目标端口。',

  // Connect-time prompt (connection_prompt.dart).
  'Host key CHANGED for {host}:{port} — possible MITM.':
      '{host}:{port} 的主机密钥已变更 — 可能存在中间人攻击。',
  'Unknown host key for {host}:{port}': '{host}:{port} 的主机密钥未知',
  'Verification code': '验证码',
  'Code': '验证码',
  'Trust': '信任',
  'Continue': '继续',
};
