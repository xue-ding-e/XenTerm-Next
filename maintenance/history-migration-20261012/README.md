# 管理员公开迁移入口

此目录是维护交付工具，不是 XenTerm 产品功能，也不会自动合入 PR93/PR95。

已发布的线性主线候选：
https://github.com/xue-ding-e/XenTerm/tree/history/linear-main-20261012

候选固定 SHA：`be4d0e484ebc9b4d5b0684b2ea726ee1b2120cb3`。它与原主线 `d641af3b924757fc26806ffbb68e82217033c41f` 逐主线快照及最终 tree 等价，62 个提交，0 merge。

## 最便捷步骤

需要 Git、Python 3.10+、curl。执行 ref 切换时还需要 GitHub CLI 已登录有上游写权限的管理员账号。脚本不收集、不保存 token，不改账号/保护设置。

从公开交付分支获取代码并先审阅脚本；实际交接评论会给出该分支的固定提交 SHA，管理员应核对或 checkout 到那个 SHA，不盲信未来发生变化的分支头：

```sh
git clone --depth 1 --branch history/linear-admin-final-v2-20261012 https://github.com/xue-ding-e/XenTerm.git xenterm-history-kit
cd xenterm-history-kit
python maintenance/history-migration-20261012/prepare_admin.py --handoff-commit $(git rev-parse HEAD) --directory ../xenterm-migration-prepared
```

Windows PowerShell 同样可执行；macOS/Linux 若 Python 命令是 `python3`，替换上面的 `python`。

准备命令只读取公开仓库和发布资产，在新的空目录生成完整 mirror/bundle、逐提交映射验证、12 个 beta 资产备份并运行脚本测试。大约需要数百 MiB 下载和额外本地磁盘空间。远端 main/tag/candidate 状态不匹配会停止，输出目录已存在也会停止；不会自动覆盖、删除或推送。

准备完成后，管理员协调暂停写入、冻结 Release workflow，并等待已有发布任务结束。不能以跳过 CI 字样代替冻结。然后：

```sh
cd ../xenterm-migration-prepared
python admin_apply.py --git-dir migration.git
```

这仍只是预检查。通过后显式切换：

```sh
python admin_apply.py --git-dir migration.git --execute --confirm-repository ixbaicn/XenTerm --acknowledge-history-rewrite --acknowledge-release-impact
```

回滚预检查与执行：

```sh
python admin_apply.py --git-dir migration.git --rollback
python admin_apply.py --git-dir migration.git --rollback --execute --confirm-repository ixbaicn/XenTerm --acknowledge-history-rewrite --acknowledge-release-impact
```

管理员必须阅读 [完整迁移说明](管理员迁移说明.md)，尤其是 Release 替换、fork/PR 重接和之后恢复自动化的阶段。这些不是 ref push 的隐式副作用。脚本故意在上游无 write、保护规则存在、发布未冻结、旧 SHA 变化或备份损坏时停止，绝不绕过门禁。

## 重要边界

- 普通 PR 的 Squash/Rebase 合并不能把目标分支已有祖先整段替换；历史切换需要管理员执行上面的精确 ref 操作。
- 当前执行器仅准备并发布了 fork 候选及本工具，上游 main/beta 未改。
- 现有 beta 二进制来自旧 SHA。迁移 tag 后须构建并验收新 SHA 对应的产物，再依项目流程重新发布、恢复自动发布，不能把旧文件当新构建。
- PR93、PR95、规范 PR97 仍独立审核；重接时使用最新头和新验证，不能由历史迁移自动批准产品变更。
- 本工具分支是临时维护交付。迁移完成并可靠离线保存交付内容后可由维护者列入清理清单，避免长期留下维护分叉；脚本不会自动删除它。
- GitHub 只读 `refs/pull/*` 与旧 PR 页面不保证消失；可写开发 branches/tags 的线性图和平台历史对象是不同范围。

## 完整 fork 单线阶段

公开工具包含 82 个旧分支离线归档后删除的逐项精确清单，和 main、规范 #97、#93、#95、旧维护交付分支的接续计划。所有保留 heads 指向同一祖先链的不同位置。上游 main/beta 仍只指 tree 等价主线；尚未获批的产品功能只在审核分支。

准备脚本从固定交付提交解析自身 SHA，输出没有占位符的 fork-plan.json，并把完整 fork bundle 校验值绑定到计划。fork 所有者在协调冻结窗口执行：

```sh
python fork_apply.py --git-dir fork-backup.git
python fork_apply.py --git-dir fork-backup.git --execute --confirm-repository xue-ding-e/XenTerm --acknowledge-history-rewrite --acknowledge-branch-cleanup --acknowledge-collaboration-freeze
```

回滚使用同一命令加 --rollback，先不加 --execute 预检，再显式执行。回滚只接受本次真实成功回执和精确切换后 refs，删除分支仅在远端仍不存在时恢复，不覆盖新工作。不可直接改计划中的旧 SHA 来强行通过。

审核顺序为 #97 → #93 → #95。后置分支包含前置变更，不能提前合入；每次合并后重新核验并接续剩余贡献。GitHub 上游 PR 不能把 fork 分支设为 base。管理员若希望独立差异，可自行建立位于同一链上的临时上游审核分支；本工具不会创建它们或自动修改 PR。

注意 SOURCE_SHA256SUMS 验证包内文件一致性，不替代对固定 Git 提交与脚本的人工信任审阅。所有脚本默认只读；真正的历史切换仍由有权限的管理员或 fork 所有者显式执行。
