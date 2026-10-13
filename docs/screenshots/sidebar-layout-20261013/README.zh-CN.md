# 侧栏与 MCP 接入说明：Linux 原生界面验收

- 日期：2026-10-13
- 对应源码：[`2d70e6cdd2fc6657ac99463519e0604ee03a48f8`](https://github.com/xue-ding-e/XenTerm/commit/2d70e6cdd2fc6657ac99463519e0604ee03a48f8)
- 环境：Debian 13 / x86_64，Xfce / X11，1364×1024，Mesa Lavapipe；Rust 1.99.0，XenTerm 0.7.3
- 原生桌面二进制 SHA256：`74e6a864c75a9cbb0f0ddd15cf60872cfcd1794b5ea255e28d7bcf5294b4d36e`
- 独立测试配置，只有 QA 分组和示例地址 `192.0.2.10`；没有连接该地址，没有导入真实凭据

## 截图

| 文件 | 实际验证内容 |
| --- | --- |
| [final-01-compact-groups-dark.png](final-01-compact-groups-dark.png) | 深色快速连接：system 与 QA 分组标题、候选项不重叠；侧栏折叠偏好不会隐藏搜索候选 |
| [final-03-jump-selector.png](final-03-jump-selector.png) | 点击跳板选择按钮主体后菜单展开；添加跳板主体也已实际点击验证 |
| [final-04-color-format.png](final-04-color-format.png) | 点击颜色格式按钮主体后菜单展开；HEX、RGB/RGBA、HSL/HSLA、HSV/HSVA、CMYK 显示正确 |
| [final-05-sidebar-light.png](final-05-sidebar-light.png) | 浅色会话页：system 折叠、空分组和普通分组统一行高；空分组计入管理统计；侧栏可拖宽 |
| [final-06-mcp-help-light.png](final-06-mcp-help-light.png) | 最终程序路径、独立配置参数、完整 JSON、操作说明及复制反馈；复制不启用 MCP |

截图均为同一代码版本的真实界面，不是设计稿。文件名中的序号只用于排列；未纳入旧二进制的第 02 张截图。

## 验证边界

源码版本在云端通过桌面测试 913 项、headless 测试 535 项；桌面忽略项为 7 个已有 benchmark。另通过 CLI/MCP、迁移、多跳 SSH、SFTP 和超时的隔离端到端回归。MCP JSON 已复制并在文本编辑器中核对完整内容。

SFTP 菜单的新增交互由合成 GPUI 组件测试验证：按钮主体、禁用范围、取消无操作、目标与路径参数；这些截图不代表真实服务器文件传输。macOS 和 Windows 的实际桌面操作没有在本次 Linux 环境中验证；跨平台 CI 结果应查看对应提交的工作流，不能由这些截图推断。

本目录提交只增加验收证据，不更改上述源码或构建依赖。
