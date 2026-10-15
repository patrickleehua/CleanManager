# 手选移入回收站（Windows）

状态：变更 `add-mole-parity-suite` 任务 6.1–6.5 的 Windows 部分已实现，其中 6.2（回收站/废纸篓移动与 D6 竞态夹具）因 macOS 未实现而未勾选；整体未通过 G1 门禁（任务 18.1），没有发布。实现位置：核心 `cm_core::manual_trash`、执行 `cm_app::plans`、平台 `windows/actions.rs` 与 `windows/restart_manager.rs`、命令 `trash`。macOS 一侧未实现，`trash` 在 macOS 返回 `unsupported`。命令形状以 [CLI 协议](cli-protocol.md) 为准。

默认关闭（设计 D15）：手选移入回收站尚未通过门禁 G1，所以它的 CLI 命令组、对应种类计划的审阅与执行、桌面页面和宿主命令都拒绝使用并说明原因。只为在这台电脑上评估时，用 `cleanmanager experimental --state on` 或桌面“设置 › 实验功能”开启（两者共用一个设置，见 [CLI 协议](cli-protocol.md) 的“实验功能”）；开启后能力仍报告为 `unverified`，不算已发布。

## 能做什么，不能做什么

- 只能把**已发布扫描中的条目**（条目 ID，不接受路径）移到系统回收站。计划种类是 `manual_selection`，动作只有原位 `fs_trash`；不能改成隔离或永久删除（MTR-01）。
- 这种计划**永远不被预授权策略匹配**：即使启用了覆盖同一范围的清理策略，`plan apply` 仍返回 `needs_approval`（退出 3），只有在终端里对这份计划逐字审阅（`plan review`）才授权。`trash` 命令本身只创建计划，没有任何“直接执行”的参数。
- 回收站内对象的**永久删除**是另一份单独的计划（`trash purge`，`fs_delete` 且目标限于回收站内），审阅时先要输入 `DELETE` 确认不可恢复，再输入摘要前 8 位（MTR-03）。
- 卷没有可用回收站（网络卷、回收站被禁用、对象过大）时，**创建计划就失败**，不会降级为永久删除，也没有“直接删除”的替代命令。

## 系统目录为什么只读（MTR-02）

以下对象只能查看、复制路径或诊断，`trash` 返回 `policy_denied`，`trash inspect` 的 `available_actions` 只有 `show_path`、`copy_path`：

1. **硬拒绝位置**（系统目录、Program Files、用户配置根、卷根、恢复环境、本工具的存储等，按卷+文件身份判定）及其内部；
2. **默认保护类目**：凭据目录（`.ssh`、`Credentials`…）、数据库文件、浏览器站点存储、AI 会话与配置、Git 工作树、模型权重；
3. **用户保护名单**中的条目。

对象本身、它的任何祖先，以及（选择目录时）目录里任何已知后代命中以上任一层，都会拒绝整体处理，并逐个列出受保护的后代，可以改选未受保护的部分。目录子树没有被完整读取（权限不足的未读范围、未穿越的链接或挂载点、大小未知的文件）同样拒绝整体处理，并说明覆盖缺口。云占位文件也拒绝：删除占位会同时删除云端副本，应在同步客户端里处理；任何预览都不会触发下载。

## 用法

```text
cleanmanager scan --root D:\Projects --json                      # 只读扫描，得到 scan_id
cleanmanager trash inspect --scan s_demo --entry 1842 --json     # 大小、覆盖缺口、受保护对象、占用进程、路径（只读）
cleanmanager trash --scan s_demo --entry 1842,1907 --json        # plan（manual_selection），authorization_status=required
cleanmanager plan review mp_demo                                 # 终端内审阅：列出对象与大小，输入摘要前 8 位
cleanmanager plan apply mp_demo --json                           # operation_report，recovery=trash
cleanmanager trash list --json                                   # 当前用户回收站中的对象（只读）
cleanmanager trash purge --item tb_0123456789abcdef --json       # 另一份计划：回收站内永久删除
```

`trash inspect` 只读取索引并用 Restart Manager 查询占用进程，不打开、不读取、不下载文件；“在资源管理器中显示”属于桌面端，CLI 只给出路径数据（`path`、`parent_display`）。

## 执行时如何核验（design D6）

- 文件：执行前用句柄重新核对身份、父链、大小与修改时间；被占用时结果为 `failed`、代码 `in_use`，消息列出占用进程（如 `notepad.exe (Notepad, pid 4242)`），从不结束进程，关闭后可重试（MTR-04）。
- 目录：作为一个对象移入回收站，但只有在现场子树与计划绑定的成员（身份、类型、指纹，由 `subtree.members_sha256` 纳入计划摘要）完全一致时才移动；移动后再核对回收站中的子树。不一致则跳过（`precondition_tree_mismatch`），重新扫描后再建计划。
- 回收站移动按路径工作，存在“最后一次核验到移动之间名称被替换”的残余竞态：进度回调里会再核对一次身份并拒绝替换后的对象；结果无法确认时记为 `unknown`（操作进入 `reconciliation_required`），消息给出放回指引，绝不报告为成功。
- 回收站内永久删除：每个回收项按“内容 → 目录（自底向上、仅为空时）→ 回收站记录（`$I`）”的顺序执行，任一步未成功则该项其余步骤跳过（`precondition_descendant_kept`）。报告中的 `estimated_freed_bytes` 只计永久删除的对象，并给出操作前后卷空闲空间的观测值。

## 如何放回

CleanManager 不从回收站恢复对象（系统回收站没有可核验的身份记录）。放回方法：打开“回收站”，选中对象，选择“还原”（或拖回原文件夹）。`operations list` 中这类操作的 `recovery.availability` 为 `system_recycle_bin`；回收站内永久删除的操作为 `unavailable`（没有撤销）。
