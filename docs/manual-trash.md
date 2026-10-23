# 手选移入回收站 / 废纸篓

状态：变更 `add-mole-parity-suite` 任务 6.1–6.5 的 Windows 部分已实现；macOS 一侧（废纸篓移动、列出、废纸篓内永久删除、占用进程）已实现但只在托管 CI runner 上运行过，没有参考设备证据，见下文“macOS”一节，6.2 仍未勾选；整体未通过 G1 门禁（任务 18.1），没有发布。实现位置：核心 `cm_core::manual_trash`、CLI 与桌面共用的检查与回收站列出 `cm_app::trash`、执行 `cm_app::plans`、平台 `windows/actions.rs` 与 `windows/restart_manager.rs`、`macos/actions/trash.rs`，命令 `trash`。命令形状以 [CLI 协议](cli-protocol.md) 为准。

默认关闭（设计 D15）：手选移入回收站尚未通过门禁 G1，所以它的 CLI 命令组、对应种类计划的审阅与执行、桌面页面和宿主命令都拒绝使用并说明原因。只为在这台电脑上评估时，用 `cleanmanager experimental --state on` 或桌面“设置 › 实验功能”开启（两者共用一个设置，见 [CLI 协议](cli-protocol.md) 的“实验功能”）；开启后能力仍报告为 `unverified`，不算已发布。

## 能做什么，不能做什么

- 只能把**已发布扫描中的条目**（条目 ID，不接受路径）移到系统回收站。计划种类是 `manual_selection`，动作只有原位 `fs_trash`；不能改成隔离或永久删除（MTR-01）。
- 这种计划**永远不被预授权策略匹配**：即使启用了覆盖同一范围的清理策略，`plan apply` 仍返回 `needs_approval`（退出 3），只有在终端里对这份计划逐字审阅（`plan review`）才授权。`trash` 命令本身只创建计划，没有任何“直接执行”的参数。
- 回收站内对象的**永久删除**是另一份单独的计划（`trash purge`，`fs_delete` 且目标限于回收站内），审阅时先要输入 `DELETE` 确认不可恢复，再输入摘要前 8 位（MTR-03）。
- 卷没有可用回收站（网络卷、该卷在回收站属性中设为“不将文件移到回收站”、组策略“不将删除的文件移到回收站”、系统查询失败）时，**创建计划就失败**：`code: "unsupported"`（退出 7），`message` 以 `trash_unavailable:` 开头并列出受影响的卷与原因，`required_action` 说明先为该卷打开回收站；`trash inspect` 对这样的对象给出 `trashable: false`、`refusal.reason: "trash_unavailable"`。不会降级为永久删除，也没有“直接删除”的替代命令。对象过大等只有执行时才知道的情况，执行时仍以 `trash_unavailable` 失败并保留原文件。规则清理（`clean`/`plan create`/`purge`/`installer`）的 trash 模式同样在建计划时检查，并提示可改用 `--mode quarantine`。
- 指向受保护系统位置的链接（例如指向 `C:\Windows\System32` 的目录联接）以 `policy_denied`（退出 3）拒绝，`message` 写明链接解析到的受保护位置；解析只读进行，从不经链接写入。指向普通位置的链接仍为 `unsupported`（链接从不移动、也不跟随）。

## 桌面

桌面与 CLI 走同一套检查（`cm_app::trash`）：

- 检查对话框对指向受保护系统位置的链接显示“受保护”并写明链接指向的位置（宿主字段 `link_target_display`），生成计划时同样以 `policy_denied` 拒绝；卷没有可用回收站时，检查阶段就标为不可移动（`refusal.reason: "recycle_bin_unavailable"`），不会先显示“可以移到回收站”。
- “清理 › 回收站”页面通过宿主命令 `trash_list`（只读，对应 `trash list`）列出当前用户回收站中的项目，最新的在前；选中同一个卷的项目后，`trash_purge_plan`（对应 `trash purge --item`）把它们冻结成单独的永久删除计划。这份计划在桌面只能查看，批准仍须在终端 `plan review` 中先输入 `DELETE` 再输入摘要前 8 位（WFA-64），桌面不提供批准。

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

## macOS

与 Windows 共用同一套命令、计划种类和审阅；以下是不同之处（实现见 `cm_platform::macos::actions::trash` 与 `crates/cm-cli/src/trash_cmd.rs`，平台背景见 [macOS 平台说明](platforms/macos.md) 第 6、7、11 节）。

- **就绪检查**：创建计划前，对每个所选对象调用 `-[NSFileManager URLForDirectory:NSTrashDirectory inDomain:NSUserDomainMask appropriateForURL:<对象> create:NO error:]`，只问系统“这个卷的废纸篓在哪里”，不创建、不列出、不移动。系统返回错误（卷没有废纸篓，如部分网络卷、只读卷）时整份计划被拒绝：`code: "unsupported"`（退出 7），`message` 以 `trash_unavailable:` 开头并写明没有永久删除替代。
- **只移文件**：macOS 的移动经 NSFileManager 按路径进行，移动前后用描述符核对身份（design D6，见 [macOS 平台说明](platforms/macos.md) 第 7 节）；本构建不能把整个文件夹作为一个已核验对象移动，所以选择文件夹时 `trash inspect` 显示 `trashable: false`、`refusal.reason: "folder_move_unavailable"`，`trash` 返回 `unsupported`，可以改选文件夹里的文件。
- **占用进程**（MTR-04）：从本用户进程的打开描述符与工作目录查询（`procs::FdIndex`），显示为 `名称 (pid N)`；其他用户的进程没有权限检查，`trash inspect` 的 `guidance` 会写明有多少个进程没能检查，执行结果里被占用的消息也会附上这一句。没有看到占用者不等于没有占用者；从不结束进程。
- **列出废纸篓**：`trash list` 列出 `~/.Trash` 与已挂载卷上的 `/Volumes/<卷>/.Trashes/<uid>`。访达自己的记录 `.DS_Store`（其中有“放回原处”的位置）从不列出、不删除；也因此 `original_path` 与 `deleted_at` 始终为 `null`，CleanManager 不声称知道原位置。`~/.Trash` 受隐私保护（TCC）或权限阻止时，命令返回 `permission_required`（退出 4，`required_action` 说明给运行 cleanmanager 的程序授予“完全磁盘访问权限”或改用访达），绝不当作空废纸篓；某个卷的废纸篓读不了时作为 `issues` 中的一条（`permission_required` 或 `bin_unreadable`），退出 5。含链接、挂载点、云占位、硬链接或被锁定对象的项 `purge_capability: "unsupported"`，请在访达中清倒。
- **废纸篓内永久删除**（MTR-03）：`trash purge` 与 Windows 相同是单独的计划，审阅时先输入 `DELETE`；执行走与清理删除相同的核验（逐级描述符、可信命名空间门槛、身份与元数据），目录在其内容之后、仅为空时删除。
- **如何放回**：在访达中打开废纸篓，选中对象，选择“文件 › 放回原处”（或拖出）。

测试：`crates/cm-cli/tests/mac_trash_flow.rs` 在临时目录夹具上覆盖检查、计划、占用进程与夹具废纸篓的列出和永久删除（`CM_RESOLVE_HOME` 重定向，此时只列 `$CM_RESOLVE_HOME/.Trash`），并用模式 000 的夹具模拟 TCC 拒绝；把文件真正移入用户废纸篓的一段只在 `CM_ALLOW_MAC_DEVICE_TESTS=1` 或托管 runner（`GITHUB_ACTIONS=true`）上运行，并删除它放进去的文件。
