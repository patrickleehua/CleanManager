# Windows 全功能验收复测报告（2026-10-23）

本报告记录对 [WFA 文字用例](windows-full-2026-10-22-cases.md) 的复测：A 部分复测 [上一轮报告](windows-full-2026-10-22-report.md) 中 CLI 失败的用例、PERF-WFA-01、BUG-WFA-18/19，以及修复涉及的回归子集；B 部分执行纯桌面用例与“（CLI）”用例的桌面部分。本轮只报告问题，没有修改产品代码。

**结论：上一轮 18 条 CLI 失败中 15 条已修复并通过，3 条（WFA-40、58、113）因新发现的问题仍为失败；PERF-WFA-01 已修复；BUG-WFA-01..19 全部按原现象复测通过。桌面部分发现 1 个高严重度问题（大扫描的概览一直停在“正在读取扫描状态…”，期间所有依赖存储的页面都被阻塞），以及 16 个低严重度问题。两个开发者报告的桌面缺口都得到确认。当前版本不能判为整体验收通过。**

原始证据只保存在本地被 Git 忽略的 `.research/acceptance-raw/windows-full-retest-2026-10-23/`：501 条命令记录（`commands.jsonl` 与 `out/` 中的原始输出）、28 份交互终端记录（`tty/`）、桌面驱动日志 `desk-log.md` 与 88 张应用窗口截图（含本机路径，不提交）、基线与夹具清单（`json/`）、模拟 AI 端点收到的请求，以及审计库副本。本文按约定脱敏：用户主目录写作 `~`，用户名 `<user>`，主机名 `<host>`，SID `<sid>`。不列应用、进程、启动项、更新或回收站清单，只记录 CMWFA 测试对象与计数。

## 版本与环境

- **被测版本**：`main@079d1e3b6ff847e2686094e1c645e9b962035554`（“fix: Windows full-feature acceptance fixes; desktop language switch”）。运行期间该提交被原地改写为 `92d4f08`，两者只差 `apps/desktop/src-tauri/src/host.rs` 测试模块中的一处测试用 PID，产品代码相同，本报告结论同样适用于 `92d4f08`。
- **构建**：只在独立干净 worktree（`git status --porcelain` 0 行、`git diff` 为空）中构建。
  - CLI：`CARGO_TARGET_DIR=<仓库>\target\acc2 cargo build --release -p cm-cli --locked`，`cleanmanager.exe` SHA-256 `30f6d2e3032d9e3dbec4d8ebcceec2b5e0d1f2706e59179903afa1f36aa53138`（12,089,856 字节），规则目录 `CM_RULES_DIR=<worktree>\rules`。
  - 桌面：`npm ci` 后 `npm run tauri -- build --debug --no-bundle`（`CARGO_TARGET_DIR=<仓库>\target\acc2-desk`），`cleanmanager-desktop.exe` SHA-256 `59240a05556ddf129bdd3fe3685a23bda27e58bfd867285030baaf1a47c8e144`（调试构建）。
  - 仅为确认性能问题，另从同一 worktree 构建了发行版桌面程序（`target\acc2-desk-rel`，SHA-256 `6eb20d7f029cb4446a266ee3367aff08743d60ff3f47e966dd0d8f3224b30cff`），只用于“大扫描概览”一项测量。
  - 工具链：rustc/cargo 1.99.0，Node 24.21.0，npm 11.19.0，Git 2.55.0，Python 3.12.15。
- **系统**：Windows 11 企业版 25H2，build 26200.9711，x64；界面语言与区域 zh-CN（微软拼音输入法）；显示缩放 200%；WebView2 运行时 155.0.4283.62。机器带电池。
- **权限上下文**：UAC 关闭（`EnableLUA=0`，本轮未改动），所有进程为 High。
- **存储**：`%TEMP%\CMWFA-20261023a\` 下的独立存储：`store`（主存储）、`store-d0`（桌面首次启动与门禁用例的全新存储）、`perfstore`（性能索引）。桌面宿主同样读取 `CM_STORE_DIR`（见 [host.rs](../../apps/desktop/src-tauri/src/host.rs) 与 `cm_platform::paths::default_store_dir`），因此默认存储没有被使用；运行前后默认存储的文件清单逐项一致。
- **桌面驱动方式（偏离指令）**：按要求设置了 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`，但本应用的 WebView2 浏览器进程命令行中没有该参数：wry 总是通过环境选项传入自己的 AdditionalBrowserArguments，环境变量因此不起作用，CDP 无法连接。通过注册表策略开启调试端口会修改策略设置，本轮没有采用。桌面改用 **Windows UI Automation**（读取 WebView2 暴露的无障碍树与文本、调用控件模式）加 **系统级输入**（`SendKeys`、`keybd_event`、`mouse_event`，操作前确认窗口在前台）驱动，截图用 `PrintWindow` 只截应用窗口。`WEBVIEW2_USER_DATA_FOLDER` 生效，桌面使用 `%TEMP%` 下的独立 WebView2 配置目录；运行前后真实配置目录的 Local Storage 文件哈希完全一致（主题、语言等偏好没有写进真实配置）。
- **运行窗口**：2026-10-22 19:45–21:58 UTC。
- **授权**：沿用负责人已给出的授权（CMWFA 测试对象上的真实回收站、HKCU 测试卸载项与 Run 值/启动快捷方式、一次性测试应用、winget 只读检查、本机模拟 AI 端点、DNS 刷新，结束后清理）。终端审阅码由执行者在真实控制台（conhost + `WriteConsoleInputW`）中输入摘要前 8 位，保存了会话记录。
- **一次意外的只读访问**：WFA-116 中驱动原生文件夹对话框失败后，“开始只读扫描”按页面默认选中的推荐根（本人的“下载”文件夹）启动了一次只读扫描，记录了 16 个条目的名称与大小。没有任何写入；该扫描只存在于测试存储中，已随存储删除。报告不列其内容。

## 统计

### A 部分：复测上一轮失败与回归

| 范围 | 结果 |
| --- | --- |
| 上一轮 CLI 失败的 18 条用例 | 通过 15 条：WFA-03、11、16、17、33、34、35、42、43、47、48、50、63、78、93；失败 3 条：WFA-40、58、113（均为本轮新发现的低严重度问题，原 BUG 已修复） |
| PERF-WFA-01 | 已修复 |
| BUG-WFA-18、BUG-WFA-19（CLI 输出） | 已修复 |
| 回归子集 22 条：WFA-01、04、21、22、27、28、29、37、38、39、56、59、61、62、64、71、73、74、75、94、108、110 | 全部通过 |

### B 部分：桌面

| 范围 | 通过 | 失败 | 受阻 | 未执行 |
| --- | --- | --- | --- | --- |
| 纯桌面用例 14 条（WFA-14、25、65、92、96–99、102、115–117、119、120） | 9：WFA-14、25、92、96、97、102、115、117、119 | 3：WFA-65（低）、98（低，待确认）、116（高） | 2：WFA-99（长按 Esc）、120（读屏） | 0 |
| “（CLI）”用例的桌面部分（本轮涉及 43 项，含 WFA-111 的语言切换） | 28 | 10：WFA-02、16、35、40、44、48、63、76、91、104（均为低） | 0 | 5：WFA-07、49、68、79、90 |
| 需要负责人在场 | — | — | — | 4：WFA-88、100、101、118（另有 3 项需人工补测，见下文） |

“通过”只表示本轮实际执行的检查点满足预期。桌面部分没有在 100%/125%/150% 缩放下检查（WFA-118，负责人在场项）。

## 上一轮问题的复测结论

| 编号 | 用例 | 本轮观察 | 结论 |
| --- | --- | --- | --- |
| BUG-WFA-01 | WFA-16 | 覆盖 `index.db` 文件头后：`operations list/show`、`recovery list`、`plan show`、`protect list`、`policy list` 照常；`scan show`、`entries list`、新建 `scan` 返回 `index_unavailable`、退出 8，`required_action` 为 `cleanmanager doctor --repair-index`；索引损坏期间可以建恢复计划并在终端批准后恢复隔离项；`doctor` 中 `store.index` 为 `failed`；`doctor --repair-index` 把损坏文件改名为 `index.db.damaged-<UTC>`（连同 `-wal`/`-shm`）保留，新建空索引，`migrations.log` 记一行 `rebuilt`；再次修复为 `not_needed`；之后扫描正常，旧扫描为 `not found`，历史记录条数不变 | 已修复（CLI）。桌面部分见 NEW-04 |
| BUG-WFA-02 | WFA-35 | 一次性 VHDX `W:` 设为“不将文件移到回收站”后：`trash inspect` 为 `trashable: false`（`refusal.reason: trash_unavailable`，只剩 `show_path`/`copy_path`）；`trash`、`plan create --mode trash`、`clean --mode trash`、`purge plan create`（默认 trash）、`installer plan create`（默认 trash）都在建计划时以 `unsupported`、退出 7 拒绝，消息以 `trash_unavailable:` 开头并给出下一步（改用隔离或为该卷打开回收站）；同卷 `--mode quarantine` 可以建计划并执行；文件未被改动 | 已修复（CLI）。桌面拒绝成立，但检查对话框有误导，见 NEW-05 |
| BUG-WFA-03 | WFA-48 | 名单写坏后：`protect list` 为 `protection_unavailable`、退出 8；只读扫描与查询正常；已批准计划 P7 的 `plan apply` 返回 `protection_unavailable`、退出 8，**没有登记操作**（操作数 7→7），授权仍为 `authorized`；v1 `plan create`、`clean`、`trash`、`purge plan create`、`installer plan create` 都被拒；名单恢复后同一 P7 在有效期内执行成功（3 项 `succeeded`） | 已修复（CLI）。观察：`clean` 与 `trash` 的这类错误没有 `required_action`。桌面部分见 NEW-07 |
| BUG-WFA-04 | WFA-50 | (1) 预览后改动默认选中的 Code Cache 目录内一个文件：该目录列入 `stale_items`（`changed`，`detail` 写明是哪个文件、扫描时与现在的大小），计划只含 GPU Cache；(2) 删除一个 review_only 文件后点名它和其他两个：它列入 `stale_items`（`missing`），其余两个照常进入计划；只点名已删除的那一个时为 `plan_stale`、退出 6 | 已修复（CLI）。桌面构建产物/安装包计划的同类检查见 NEW-09 |
| BUG-WFA-05 | WFA-17 | `candidates list` 的每条候选带 `tier`、`tier_reason`、规则自己的 `reason`（临时文件写明 7 天与“修改时间，不是最后使用时间”）、`rebuild_class`、`regeneration`、`offline_effect`、`age_threshold`（`basis: modified_time`、`min_age_days: 7`）、每种模式的 `recovery_limits`、`allowed_modes`、`risk`、规则版本与 `scan_revision`；档位与 `clean --dry-run` 一致（Code Cache/GPU 为 default_selected，HTTP 缓存为 review_only），Local Storage 与新文件不是候选 | 已修复 |
| PERF-WFA-01 | — | 在 28 次扫描、1,131,233 行（538 MB）的独立索引上：`capabilities` 0.017 秒，`operations list` 0.020，`plan show` 0.022，`recovery list` 0.020，`candidates list` 0.028，`entries list --limit 50` 0.059，`scan show`（4 万条的扫描）0.090（各 5 次的中位数）；`doctor` 的完整检查 0.92 秒。观察：扫描宿主在终态前被强杀后，下两条打开索引的命令分别用了 0.67 与 0.94 秒，之后恢复到 0.05 秒 | 已修复 |
| BUG-WFA-06 | WFA-11 | 4 万文件树 `--detach` 后在 0.15–1.0 秒的 6 个时点强杀宿主：6 次都是 `interrupted`、`partial_scope`、issue `host_exited`、退出 5；1.3 秒时已正常完成 | 已修复。桌面历史显示“已中断” |
| BUG-WFA-07 | WFA-03 | 实验功能关闭与开启时，`status --experimental`、`clean list --bogus`、`trash --bogus`、`process show`（缺 PID）都是 `invalid_argument`、退出 2；`CM_EXPERIMENTAL=1`、`CLEANMANAGER_EXPERIMENTAL=1` 下 `status` 仍为 `unsupported`、退出 7 | 已修复 |
| BUG-WFA-08 | WFA-33 | A（隔离 1,000 项）运行中执行 B（回收站 100 项）为 `conflict`、退出 6；A 完成后 B 的 100 项都是 `skipped`/`precondition_missing`（“the object no longer exists at its planned place”）；回收站中没有 wfa33 的项目 | 已修复 |
| BUG-WFA-09 | WFA-34 | 1,100 项计划执行中强杀：`operations show` 返回临时 `operation_report`，`status: reconciliation_required`、退出 8，464 项 `succeeded`、1 项 `unknown`/`outcome_unconfirmed`（提示先 reconcile）、635 项 `not_started`；reconcile 后为 `partial`，未知项判为 `not_started`/`reconciled_not_performed`，没有重做；再次 apply 回放同一操作 | 已修复 |
| BUG-WFA-10 | WFA-40 | 核对后 `authorization_source` 仍为 `interactive_review`，另有 `reconciled_at` | 已修复。WFA-40 因 NEW-12 仍失败 |
| BUG-WFA-11 | WFA-42 | 策略命中后 `plan show` 的 `authorized_by` 为 `source: policy` 并带策略 ID 与版本；`operations list` 为 `policy:<id>@2`；用量 53,072 字节/3 项 | 已修复 |
| BUG-WFA-12 | WFA-43 | (a)–(e) 都是 `needs_approval`、退出 3，`details.policy_matches` 分别给出 `mode_differs`、`rule_not_covered`、`scope_differs`、`single_limit`（1,001 项超出单次 1,000 项，额度不变），暂停时为 `not_enabled`；手选计划的消息说明“manual_selection 只能由人工审阅授权”；撤销后启用为 `conflict`，已完成的策略操作不受影响 | 已修复 |
| BUG-WFA-13 | WFA-47、63 | `trash --entry <指向 System32 的 junction>` 为 `policy_denied`、退出 3，消息写明链接解析到硬拒绝位置 System32；`trash inspect` 同为 `policy_denied`/`protected`；指向普通位置的链接仍为 `unsupported`（`link_or_reparse`） | 已修复（CLI）。桌面仍是旧行为，见 NEW-08 |
| BUG-WFA-14 | WFA-58 | 被 `FileShare.None` 占用的 `held.zip` 列出为 `skip_reason: in_use`，附占用者名称与 PID，建计划被拒；已挂载的 VHDX 为 `mounted` 并附推出指引 | 已修复。WFA-58 因 NEW-10 仍失败 |
| BUG-WFA-15 | WFA-78 | 343 行：130 个商店行全部 `cannot_check` 并指向商店更新页；winget 26 行最新、12 行有更新；没有通道的 175 行为 `cannot_check`；调试轨迹只出现 winget 一次调用 | 已修复。桌面更新页计数一致 |
| BUG-WFA-16 | WFA-93、94 | `process list` 与 `process_end` 计划都带 `started_at_utc`（RFC 3339），`started_at` 仍为计划绑定用的精确刻度值 | 已修复 |
| BUG-WFA-17 | WFA-113 | 程序仍存在时的启动项残留为 `action: manual`、`tier: review_only` | 已修复。WFA-113 因 NEW-11 仍失败 |
| BUG-WFA-18 | WFA-108 | 存储中有 `features.json`、`resident.json` 与 `diagnoses\` 时，`self uninstall` 预览不再出现 “not recognised”，分别归入配置与“历史（含保存的 AI 诊断）”；`--remove-user-data` 在隔离区非空时不生效 | 已修复 |
| BUG-WFA-19 | 多处 | 本轮 465 份 CLI 输出的 `message`/`detail`/`guidance` 等字段中没有 Rust Debug 文本，调试轨迹为 `exit=0` | 已修复（CLI）。桌面中文界面仍有未翻译的后端英文与未知代码回退，见 NEW-13 |

## 新发现的问题

### 高

**NEW-01（WFA-116、WFA-14）扫描条目较多时，桌面概览一直停在“正在读取扫描状态…”，并阻塞所有依赖存储的页面。**
复现：用 CLI 扫描一个 4 万条目的目录树（`fx\wide`，400 个目录 × 100 个文件），启动桌面，在“分析 › 概览 › 其他扫描”中打开它。
预期：几秒内显示范围、时间、完整性与大小。
实际：发行版桌面程序 150 秒后仍显示“正在读取扫描状态…”，进程持续占满一个 CPU 核心；同一期间“计划”页显示“正在读取计划…”。20,001 条目的单层目录要约 37 秒才显示；122,880 条目的已取消扫描 150 秒内没有显示；调试构建下 300,601 条目的扫描累计 400 多秒 CPU 仍未显示。启动时概览会自动打开最近一次扫描，因此扫描过一个大文件夹后，每次启动都会卡住。“状态”等不读存储的页面仍可用。CLI 对同样的扫描 `scan show` 在 0.1 秒内返回。
影响：真实用户文件夹的条目数通常远超 4 万，桌面在这种情况下事实上不可用。本轮因此无法在桌面上完成“大目录扫描中切换页面、取消后先显示‘正在停止’再显示‘已取消’”的检查。
疑似组件：桌面宿主读取扫描状态/汇总的路径（[commands.rs](../../apps/desktop/src-tauri/src/commands.rs)、[host.rs](../../apps/desktop/src-tauri/src/host.rs) 中的扫描汇总与存储锁），具体函数未定位。

### 低

| 编号 | 用例 | 复现与实际 | 预期 | 疑似组件 |
| --- | --- | --- | --- | --- |
| NEW-02 | WFA-27、117、65、76 | 在桌面执行任何计划后（隔离、手选回收站两次、恢复、批量卸载，共 5 次），结果页都显示红色错误“找不到这条记录。它可能来自已被清理的旧索引。”；从“活动”重新打开同一操作仍然显示。逐项结果与汇总本身正确 | 成功的操作不显示错误 | [operation.ts](../../apps/desktop/src/views/operation.ts)（`actionError` 来自某次失败的查询且未清除）及宿主对应查询 |
| NEW-03 | WFA-02（桌面） | 实验功能关闭后打开已存的手选回收站计划：仍显示摘要输入框和可点击的“批准这份计划”，没有门禁说明；点击后才提示“这项功能默认关闭”。授权仍为 `required`，没有操作 | 门禁关闭时不提供批准与执行，并直接说明门禁 | [plan.ts](../../apps/desktop/src/views/plan.ts) |
| NEW-04 | WFA-16（桌面） | 索引损坏时启动桌面：“概览”和“磁盘”显示“还没有扫描结果 / 还没有可浏览的结果”，像是从未扫描过；只有“设置 › 数据与 CLI”与开始新扫描时才说明索引已损坏和修复命令（开始扫描后任务条还显示“扫描准备中 / 已发现 0 项”）。活动与隔离区正常 | 概览与磁盘页说明索引需要重建，不把损坏显示为空历史 | [overview.ts](../../apps/desktop/src/views/overview.ts)、[disk.ts](../../apps/desktop/src/views/disk.ts) |
| NEW-05 | WFA-35（桌面） | 在回收站关闭的 `W:` 上对 `x.bin` 点“移到回收站…”：检查对话框把原因以英文显示（“the Recycle Bin is turned off for this volume …”），同时写着“1 / 1 项可以移到回收站”，“生成计划”可点；点击后才显示“所在的卷没有可用的回收站。不会改为永久删除。”且不生成计划 | 检查阶段就标明不可处理（0 / 1），用中文说明 | [suite.rs](../../apps/desktop/src-tauri/src/suite.rs) `trash_inspect`，回收站对话框视图 |
| NEW-06 | WFA-44（桌面） | 磁盘页详情只有“选择 / 在资源管理器中显示 / AI 诊断 / 技术详情”，没有“保护”；只能在“设置 › 保护清单”用原生对话框保护文件夹 | 磁盘页可以对选中对象选择“保护” | [disk.ts](../../apps/desktop/src/views/disk.ts) |
| NEW-07 | WFA-48（桌面） | 名单写坏后，“设置 › 保护清单”说明原因、文件路径和“所有写操作已停止”，但没有修复或恢复默认的入口，“保护一个文件夹…”仍可点；写操作前的检查对话框正确阻止 | 提供修复或恢复默认的入口 | [settings.ts](../../apps/desktop/src/views/settings.ts) |
| NEW-08 | WFA-47、63（桌面）——开发者报告的缺口一，**确认** | 在磁盘页选中 `link-sys`（指向 System32 的 junction）后“移到回收站…”：对话框只显示“这是链接或重分析点，不会被跟随，也不会被移动。”，没有说明它解析到受保护的 System32；CLI 已改为 `policy_denied` 并写明目标。没有生成计划。另外桌面没有右键菜单（右键只是选中），受保护对象也能先“选择”，到检查对话框才被拒 | 与 CLI 一致，写明链接指向的受保护位置 | [suite.rs](../../apps/desktop/src-tauri/src/suite.rs) `trash_inspect`/`trash_plan` 没有做 CLI `trash_cmd.rs` 中的 `link_protection` 映射 |
| NEW-09 | WFA-50（桌面）——开发者报告的缺口二，**确认（构建产物/安装包计划）** | 桌面“构建产物”发现后，把 `rustold\target\debug\x.o` 加长 1,000 字节，再在桌面勾选 `rustold\target` 生成计划：计划照常生成，仍按扫描时的 2,097,152 字节冻结，没有列为变化项。CLI 对同样的改动返回 `plan_stale`（`changed`）。手选回收站方面：桌面与 CLI 都不在建计划时核对目录内容；在桌面改动 `dirA\sub\inner2.bin` 后对 `dirA` 生成计划并执行，执行器以 `precondition_tree_mismatch` 跳过、文件夹未被移动，执行侧是安全的，但结果页写成“未识别的问题（代码 precondition_tree_mismatch）” | 桌面与 CLI 一样逐个核对目录下的文件，变化项列为 stale | [suite.rs](../../apps/desktop/src-tauri/src/suite.rs) `stale_reason` 只看顶层对象，且只比较文件大小 |
| NEW-10 | WFA-58 | 对已挂载的 VHDX 候选 `installer plan create --candidates <id>` 返回 `plan_stale`（“every selected item changed since discovery … run the preview again”），重新预览后仍然如此；被占用的 `held.zip` 则正确为 `policy_denied` | 已挂载镜像以“不可计划（mounted）”拒绝，不提示重新预览 | [review.rs](../../crates/cm-cli/src/purge_cmd/review.rs) `plan_from_sets`：现场核对先于可计划判定，挂载中的镜像文件一直在变 |
| NEW-11 | WFA-113、WFA-43 | Code Cache 目录的内容被隔离后，重新扫描分析时这个空目录（0 字节、没有文件）仍是 `default_selected` 候选，`plan create` 生成并保存了一份 0 个动作的计划（`items: []`）；该 `plan` 记录未通过 v1 Schema（`body` oneOf）。本轮其余 458 条记录全部通过，错误码与退出码 0 处不符 | 没有文件的目录不成为候选；空选择不生成计划 | [plan.rs](../../crates/cm-core/src/plan.rs)、[analysis.rs](../../crates/cm-core/src/analysis.rs)、[plan_cmd.rs](../../crates/cm-cli/src/plan_cmd.rs) |
| NEW-12 | WFA-40 | (1) 尚未核对的中断操作在 `operations list` 中 `authorization_source` 与 `counts` 为 null（实际经人工审阅授权）；(2) 已完成的 plan/v2 结束进程、卸载与残留操作，`action` 为 null，恢复为 `unknown`，说明“The plan of this operation cannot be read”——结束进程应为不可恢复，带注册表备份的残留可以用 `apps uninstall restore` 恢复。上一轮的原始数据中已有 (2) 的现象，当时未报告。桌面“活动”照样显示“恢复情况未知，需要核对”，并把注册表还原操作标成“从隔离区恢复” | 每条历史都有授权来源与准确的恢复入口 | [recovery.rs](../../crates/cm-app/src/recovery.rs) `availability` 依赖 `plans::history_facts`（[plans.rs](../../crates/cm-app/src/plans.rs)）对这些计划种类返回空 |
| NEW-13 | WFA-111、91、66、93 | 中文界面中有后端英文原文：空间洞察的说明与官方用法（例如 WinSxS 与分页文件），进程用途说明，卸载残留的归属依据，回收站不可用原因；未知代码直接显示为“未识别的问题（代码 …）”，其中系统状态页把正常状态显示为“未识别的问题（代码 normal）” | 界面语言下的文字都本地化；已知代码都有说明 | [codes.ts](../../apps/desktop/src/lib/codes.ts) 及宿主 DTO 中的英文字符串 |
| NEW-14 | WFA-76（桌面） | 批量卸载勾选 Bar、Baz、Mismatch：计划正确只含 Bar 与 Baz，但 Mismatch 的拒绝原因显示为“策略额度不足或不再覆盖此计划”；实际原因是签名者与发布者不符（CLI 为 `policy_denied`，“signed by Microsoft Windows, not by Contoso Test”） | 显示真实拒绝原因 | [uninstallBatch.ts](../../apps/desktop/src/views/uninstallBatch.ts) 的拒绝原因映射 |
| NEW-15 | WFA-65（桌面） | “清理 › 回收站”页面是“仅命令行”说明页，不列出回收站项目，执行后无法在这里看到刚移入的两个文件；桌面没有右键菜单 | 回收站页列出本次移入的对象 | [trash.ts](../../apps/desktop/src/views/trash.ts) |
| NEW-16 | WFA-98（待确认） | “保持亮屏”开始后，`powercfg /requests` 中 SYSTEM、执行、ACTIVELOCKSCREEN 各多一条匿名的“旧的内核调用程序”，结束后消失；DISPLAY 一直为“无”。电源计划的息屏/睡眠超时前后不变 | 用例要求本进程列于 DISPLAY 与 SYSTEM。实现使用 `SetThreadExecutionState(ES_DISPLAY_REQUIRED \| ES_SYSTEM_REQUIRED)`，本机系统可能这样归类；屏幕是否真的保持点亮需要负责人以超过息屏时间的空闲来确认 | [resident.rs](../../crates/cm-platform/src/windows/resident.rs) |
| NEW-17 | WFA-104（桌面） | AI 诊断预览写着“查看实际发送的 4.5 KB（4,606 字节）”，展开后是格式化的 JSON；模拟端点实际收到的请求体为 4,212 字节 | 预览显示的就是逐字节相同的请求体及其长度 | [diagPreview.ts](../../apps/desktop/src/views/diagPreview.ts) |

## 逐项结果

### A 部分（CLI）

| 用例 | 结果 | 实际观察 |
| --- | --- | --- |
| WFA-01 | 通过（回归） | 关闭时 14 项能力 `enabled=false`、`gate_passed=false`；13 条对标命令都是 `unsupported`、退出 7，`required_action` 为开启命令；存储只多出 `lifecycle.lock` |
| WFA-03 | 通过 | 见 BUG-WFA-07；开启后能力为 `unverified`，没有 `available` |
| WFA-04 | 通过（回归） | `completed`，31 个条目，17 个清单对象大小一致，链接内没有条目，夹具与 sentinel 的清单不变 |
| WFA-11 | 通过 | 见 BUG-WFA-06 |
| WFA-16 | 通过 | 见 BUG-WFA-01 |
| WFA-17 | 通过 | 见 BUG-WFA-05 |
| WFA-21、22 | 通过（回归） | quarantine 与 trash 两份计划的 ID 与摘要不同、900 秒有效期、required；delete 为 `policy_denied`；终端输错码退出 130 且不授权，正确码后 `authorized_by.source: interactive_review` |
| WFA-27、28 | 通过（回归） | CLI 执行隔离 3 项 `succeeded`（已处理 3,145,728、预计释放 "0"、带卷观测）；回收站计划 3 项进入回收站，用资源管理器“还原”放回一项；历史为 `in_app_restore`/`system_recycle_bin`。说明：先用同一批文件做的回收站计划在文件已隔离后为 3 项 `precondition_missing`（符合 BUG-WFA-08 修复） |
| WFA-29 | 通过（回归） | 安装包 delete 模式：审阅显示 “Permanent delete: there is no undo.”；文件消失且不在回收站；`estimated_freed` 104,858,827，观测差值 104,796,160；历史 `unavailable` |
| WFA-33 | 通过 | 见 BUG-WFA-08 |
| WFA-34 | 通过 | 见 BUG-WFA-09 |
| WFA-35 | 通过 | 见 BUG-WFA-02。VHDX、盘符与该卷的 BitBucket 设置已移除 |
| WFA-37、38、39 | 通过（回归） | 恢复计划独立摘要、未批准退出 3、执行后放回原处；原位置已有文件时 `destination_exists`；清空计划执行后 `estimated_freed` 1,048,576 |
| WFA-40 | 失败（低） | 核对后授权来源保留并带 `reconciled_at`（BUG-WFA-10 已修复）；见 NEW-12 |
| WFA-42、43 | 通过 | 见 BUG-WFA-11、12 |
| WFA-47、63 | 通过 | 见 BUG-WFA-13；大小写变体都是硬拒绝；`C:\Windows`、`..\x`、`\\?\C:\Windows` 的 add 都被拒；凭据、数据库、受保护后代、覆盖不全的目录为 `policy_denied`，扫描根本身为 `invalid_argument` |
| WFA-48 | 通过 | 见 BUG-WFA-03 |
| WFA-50 | 通过 | 见 BUG-WFA-04；无可选项时 `policy_denied` |
| WFA-56、59 | 通过（回归） | 5 个安装包候选全部 review_only、带来源标签；`replace.zip` 被替换后 `precondition_metadata_changed` 跳过并保留，`appx.zip` 进入回收站，`partial`、退出 5 |
| WFA-58 | 失败（低） | 被占用的安装包已列出（BUG-WFA-14 已修复）；见 NEW-10 |
| WFA-61、62 | 通过（回归） | `manual_selection`、只有 `fs_trash`、`policy_matchable: false`；`--mode` 退出 2；非交互 apply 退出 3；两项进入回收站，`trash list` 只列出这两项，资源管理器还原 e1 后文件回到原处 |
| WFA-64 | 通过（回归） | 回收站内 11 个 CMWFA 项目（22 个动作）的 `trash purge` 计划，终端先输入 `DELETE` 再输入摘要，22 项全部成功，回收站回到 0 项 |
| WFA-71/73/74/75 | 通过（回归） | FooApp 卸载：`app_quit` + `tool_adapter`，`leftovers_remain`；有 FooTwin 时同名数据为 `kept`，去掉后缓存与日志默认选中、个人数据与设置不预选；第二段 7 个动作全部成功（注册表键先备份），models/settings/credentials 保留；程序删除后 Run 值以 `remove_with_backup` 删除；恢复计划按备份重建注册表键与值 |
| WFA-78 | 通过 | 见 BUG-WFA-15 |
| WFA-93、94 | 通过 | 见 BUG-WFA-16；只结束测试进程；已退出的目标为 `precondition_gone`；PID 4、lsass、csrss、安全软件进程为 `policy_denied` |
| WFA-108 | 通过 | 见 BUG-WFA-18；托盘登录值存在时预览列出 `remove_run_value` |
| WFA-110 | 通过（回归） | 默认输出不含用户名、主机名或 `C:\Users\` |
| WFA-113 | 失败（低） | 未知选项、重复选项、`-y`、重复 `--format`、`help --json`、缺值都是 `invalid_argument`、退出 2；非 TTY 默认 JSON；见 NEW-11 |
| PERF-WFA-01 | 通过 | 见上表 |

### B 部分（桌面）

| 用例 | 结果 | 实际观察 |
| --- | --- | --- |
| WFA-14 | 通过 | 选中色块后详情与表格同步（ID 与大小一致）；文件夹色块双击或 Enter 下钻，面包屑与“返回上一级”可用，Backspace 返回后焦点回到原色块；中文过滤得到 1 项，不存在的词显示“没有符合过滤条件的条目”；大小未知的 `link-out`/`link-sys` 不画面积并单列；Delete 键无动作。未覆盖：逻辑/分配口径切换（页面只写“面积与排序按逻辑大小”，没找到切换控件）、`empty` 文件夹空态 |
| WFA-25 | 通过 | 按 ID 打开 CLI 计划 P5：摘要 `3b9444c3…` 与 CLI 相同，3 项完整清单；输入前 8 位批准后 CLI `plan show` 为 `authorized`、`interactive_review` |
| WFA-65 | 失败（低） | 选中两个文件，选择栏显示“已选 2 项 32.6 KiB”；检查对话框、审阅（写明“移到回收站”）、批准与执行正常，2 项成功，结果页提示在系统回收站中还原；见 NEW-15、NEW-02 |
| WFA-92 | 通过 | 隐藏“网络”面板、把“指标”上移、刷新间隔改为 10 秒，重启后全部保留（“每 10 秒刷新”）；最后恢复默认。进程页按内存排序是否保留未确认 |
| WFA-96 | 通过 | 开启托盘后通知区域出现图标，提示文字为处理器、内存与系统盘读数；右键菜单只有读数（灰色）、“打开系统状态 / 打开进程 / 打开清理 / 常驻工具设置”、“关闭常驻组件”与“退出 CleanManager”，没有写操作入口；关闭主窗口只隐藏，从托盘“打开系统状态”恢复窗口 |
| WFA-97 | 通过 | “登录时只启动托盘”写入 `CleanManager.Resident = "<桌面程序路径>" --resident`，页面显示“已创建”；关闭托盘即删除该值；桌面关闭实验功能时删除该值并停止常驻工具；**托盘运行时用 CLI `experimental --state off`：约 1.6 秒内托盘图标消失、登录值删除、保持亮屏请求撤销**，状态与安装包页显示“默认关闭”；重启后托盘不启动、值不重建；值存在时 `self uninstall` 预览列出 `remove_run_value` |
| WFA-98 | 失败（低，待确认） | 见 NEW-16 |
| WFA-99 | 受阻 | 30 秒超时：黑色置顶窗口 `CleanManager 清洁屏幕` 出现，31 秒时自动退出；0.5 秒短按 Esc 不退出。用 `keybd_event` 注入的 2.6 秒长按（包括模拟自动重复）没有触发退出，无法区分是注入输入的问题还是产品问题，需要物理键盘复核（见下文） |
| WFA-102 | 通过 | “此平台不提供”：风扇控制、电池充电上限、网络低延迟、Touch ID 各有原因，没有控件 |
| WFA-115 | 通过 | 全新存储首次启动：五个工具与六个扩展的顺序、名称与快捷键表一致；Ctrl+1、4–9、0、Ctrl+, 都跳到对应页面，没有白屏；清理页引导先扫描，不显示“可清理”数字；快捷键按钮打开快捷键表，Esc 关闭后焦点回到按钮。环境限制：Ctrl+2、Ctrl+3 是本机其他程序注册的全局热键（`RegisterHotKey` 返回 1409），到不了应用；`?` 键在中文输入法下未验证 |
| WFA-116 | 失败（高） | 扫描范围页显示推荐根与“只读取名称、大小和时间”；原生文件夹对话框（标题“选择要扫描的文件夹”）能打开，但自动化无法可靠选择路径（底部的“选择文件夹”按钮不在 UIA 树中，中文输入法吞掉了反斜杠），已取消对话框；部分完成的扫描在概览中显示缺口与“不会自动提权”；大目录扫描的切换与取消检查因 NEW-01 无法完成 |
| WFA-117 | 通过 | 只用系统级按键：Ctrl+6 → Tab 到计划行 → Enter 打开 → Tab 到摘要框输入前 8 位 → Tab → Enter 批准 → 焦点自动移到“执行：移入同卷隔离区（2 项）”→ Enter 执行；Ctrl+8 → Tab 到两个隔离项各按空格 → Tab 到“创建恢复计划（2 项）”→ Enter → 输入摘要 → 批准 → 执行，两个文件回到原处、SHA-256 不变；磁盘页 Tab 到色块 Enter 下钻、方向键移动、Backspace 返回且焦点回到原色块，表格行 Enter 打开、Delete 无动作。说明：输入十六进制摘要时先按一次 Shift 把中文输入法切到英文（结束时已切回）；页面切换后焦点落在文档根，下一次 Tab 从页首开始；隔离区每次只列 50 项，“显示更多”后需要逐项 Tab，项目很多时只用键盘很难到达最新项（本轮先在 CLI 清空了 1,464 个夹具隔离项） |
| WFA-119 | 通过 | 主题按钮依次切到深色、浅色、跟随系统，深色截图正常；选择在重启后保留；最后恢复为跟随系统。对比度未做仪器测量 |
| WFA-120 | 受阻 | 未运行 Narrator。UIA 层面：色块名称含名称、大小、占比与“按 Enter 打开”，表格行、计划审阅、按钮都有可访问名称；播报行为需负责人用读屏验证 |
| 语言切换（LIF-05、WFA-111 桌面部分） | 通过 | “设置 › 界面语言”选 English 后侧栏与页面立即变为英文（清理、状态、计划、活动、恢复、Agent、AI 诊断等页面除应用名外没有中文）；重启后仍为英文；选简体中文后立即恢复，重启后保留；最后恢复为跟随系统。后端英文原文的问题见 NEW-13 |

“（CLI）”用例的桌面部分：

| 用例 | 结果 | 实际观察 |
| --- | --- | --- |
| WFA-01 | 通过 | 关闭时构建产物、安装包、回收站、卸载、更新、启动项、空间洞察、进程、常驻工具、性能诊断、AI 诊断都原位显示“默认关闭”、它会做什么与开启方法；浏览这些页面没有在存储中写入设置 |
| WFA-02 | 失败（低） | 见 NEW-03 |
| WFA-03 | 通过 | CLI 开启后桌面无需重启即显示“实验功能”标记，“设置 › 实验功能”为“已开启”，各工具“门禁 G1/G3 未通过，已开启”；在桌面关闭后 CLI `experimental` 为 false |
| WFA-08 | 通过 | 部分完成的扫描：概览写明 1 处位置无法读取、不按 0 计算、不会自动提权；磁盘页 `denied` 记为大小未知 |
| WFA-09、11 | 通过 | “活动 › 扫描”中取消的扫描为“已取消”，强杀宿主的为“已中断” |
| WFA-13 | 通过 | 同时选中两个硬链接名：“已选 2 项 64.0 KiB，去重后逻辑大小” |
| WFA-15 | 通过 | `tagged` 行显示“可手动清理，依据：文件夹内有有效的 CACHEDIR.TAG 缓存标记”，`badtag` 与 `.ssh` 没有 |
| WFA-16 | 失败（低） | 历史与隔离区正常；见 NEW-04 |
| WFA-20 | 通过 | 指令样文件名在表格中原样显示 |
| WFA-24 | 通过 | 过期计划显示“已过期”，没有批准入口，说明过期后需要重新生成 |
| WFA-27、41 | 通过 | 执行按钮写明“执行：移入同卷隔离区（3 项）”；结果页分列已处理 3.0 MiB、仍在隔离区 3.0 MiB（空间未释放）、卷可用空间变化 −208.0 KiB 并注明可能包含其他程序的写入；CLI `operations show` 一致。见 NEW-02 |
| WFA-29 | 通过 | 删除模式计划写明“无法撤销”“永久删除 1 个安装包”，按钮为“批准这份永久删除计划”。默认焦点未检查 |
| WFA-30 | 通过 | 已执行的 P5 只提供“查看执行结果”，没有再次执行入口 |
| WFA-35 | 失败（低） | 拒绝成立；见 NEW-05 |
| WFA-40 | 失败（低） | 见 NEW-12 |
| WFA-44 | 失败（低） | 见 NEW-06 |
| WFA-48 | 失败（低） | 见 NEW-07 |
| WFA-53 | 通过 | “设置 › 数据与 CLI › 构建产物扫描根目录”与 CLI `purge roots` 同一份：只有测试根（默认根已在测试存储中排除） |
| WFA-60 | 通过 | “在资源管理器中显示”打开所在文件夹并选中 `a.txt` |
| WFA-62 | 通过 | 结果页与历史写“在系统回收站中还原”，没有产品内恢复按钮 |
| WFA-63 | 失败（低） | 见 NEW-08 |
| WFA-64 | 通过 | 回收站内永久删除计划在桌面只提供“复制：在终端中审阅并批准/执行”，没有批准控件 |
| WFA-66、67 | 通过 | 空间洞察卡片有已知大小或数量与观测时间，系统对象只给官方用法、没有删除入口，WinSxS 不求和。英文原文见 NEW-13 |
| WFA-69 | 通过 | 应用页搜索 CMWFA 得到 6 个测试应用，带发布者、版本与来源；FooTwin 已在 WFA-73 中删除，未再核对同名不合并 |
| WFA-76 | 失败（低） | 一份计划含 Bar 与 Baz，Mismatch 不在计划中；执行后 Bar 卸载失败（退出码 3）、Baz 已卸载有残留，整体部分完成，逐个应用报告；第 2 步残留只含 Baz，Bar 全部保留。见 NEW-14 |
| WFA-78 | 通过 | 更新页计数与 CLI 一致（343 行：有更新 12、无法检查 304、已是最新 26），只在点击时检查 |
| WFA-81 | 通过 | CMWFA-Run 已启用，Broken 为“目标缺失”，Offline 为“目标暂不可达” |
| WFA-86 | 通过 | 5 项维护任务，没有预先勾选，适用性为只读探测结果 |
| WFA-91 | 失败（低） | 健康分 100/100，权重 30/25/20/15/10，指标与“不可用”的显卡温度；诊断行显示“未识别的问题（代码 normal）”，见 NEW-13 |
| WFA-93 | 通过 | 进程页有签名、用途、置顶与未识别计数 |
| WFA-103 | 通过 | 未配置时页面写明“还没有配置提供方，诊断入口会显示‘未配置’，不会发送任何内容”，密钥只存凭据管理器 |
| WFA-104 | 失败（低） | 在“设置 › AI 诊断”添加兼容端点 `http://127.0.0.1:18765/v1`；测试连接只发出一次 `GET /v1/models`（无正文、无认证头）；预览期间与取消后模拟端点没有收到请求；“发送并诊断”只发出一次 POST，正文不含用户目录路径与用户名；回答中的命令与“已批准”被移除（4 处），建议降为“需要补充信息”，没有生成计划；`diag-providers.json` 不含密钥。见 NEW-17 |
| WFA-108 | 通过 | 见 A 部分 |
| WFA-114 | 通过 | Agent 页说明桌面与终端共用计划编号与摘要、自动化调用不能批准计划 |
| WFA-07、49、68、79、90 | 未执行 | WFA-07 需要原生对话框选中 `jroot`（同 WFA-116 的对话框问题）；WFA-49 只看到“按规则分析这次扫描”的入口，没有完成分析后的目录核对；WFA-68 本轮没有建立 F-11 测试下载文件（卡片显示 0 个文件）；WFA-79 未在桌面关闭检查；WFA-90 未运行诊断 |

## 开发者报告的两个桌面缺口

1. **桌面手选回收站对“链接到受保护系统位置”的拒绝**：确认。CLI 已写明 System32，桌面仍显示泛化的“链接或重分析点”说明（NEW-08）。没有生成计划，安全性不受影响。
2. **桌面现场核对只看顶层对象**：确认，范围是桌面的构建产物/安装包计划（`suite.rs` 的 `stale_reason` 只比较顶层文件大小，目录内的变化不会被列出，计划按旧大小冻结，NEW-09）。桌面手选回收站在建计划时没有现场核对（CLI 也没有），但执行器按 `subtree.members_sha256` 核对目录成员，改动过的目录以 `precondition_tree_mismatch` 跳过，没有被移动。

## 需要负责人在场的用例（步骤供主会话使用）

所有 CLI 命令都在新的独立存储中运行：`$env:CM_STORE_DIR = '<新目录>'`、`$env:CM_RULES_DIR = '<仓库>\rules'`，先 `cleanmanager experimental --state on`，结束时 `--state off` 并删除该存储。桌面用同样的环境变量启动 `cleanmanager-desktop.exe`。

1. **WFA-88（一次 UAC：同意与拒绝）**：本机 UAC 关闭（`EnableLUA=0`），所有进程都是完整管理员，不会出现 UAC。是否临时启用 UAC 属于安全设置变更，由负责人决定；启用并重启后，在普通终端中用 `whoami /groups` 确认为 Medium。(1) `cleanmanager optimize plan create --tasks windows.dns-flush,windows.image-health-check --json` 得到计划 A，`cleanmanager plan review <A>` 输入摘要前 8 位，`cleanmanager plan apply <A> --json`，负责人在 UAC 中点“是”。核对只提示一次、`elevation=granted`、image-health-check 为 `unchanged`、dns 为 `applied`，结束后 `Get-Process cleanmanager` 没有残留的提权进程。(2) 内容相同的计划 B，负责人在 UAC 中点“否”：image-health-check 为 `skipped`/`elevation_declined`，dns 仍 `applied`，退出 5，没有运行计划外任务。
2. **WFA-100（摄像头与麦克风）**：桌面打开“状态 › 常驻工具”；负责人打开系统“相机”或“录音机”再关闭。核对 3 秒内显示正在使用的应用名、关闭后消失；对比前后存储目录的文件列表，没有新增使用记录；记录不可读时显示“不可用”及原因。
3. **WFA-101（电量与低电量提醒）**：本机有电池（本轮读数 96%，正在充电）。页面显示本机电量与“没有已连接的蓝牙设备”；先开启托盘，再开启低电量提醒并把阈值设为不低于当前电量；负责人拔掉电源。核对只提醒一次（托盘读数、页面与读屏播报），接回电源后不再提醒；再输入阈值 4 与 51，确认只接受 5–50%。
4. **WFA-118（100/125/150% 缩放）**：本机当前为 200%。负责人在“设置 › 系统 › 屏幕 › 缩放”中依次设为 100%、125%、150%，每次重启应用，检查概览、磁盘（表格与 treemap）、计划审阅、结果页与设置，并把窗口缩到约 800 px 宽；核对主要操作可用、文字不截断、页面无横向溢出、treemap 重绘正确；最后恢复为 200%。

另需人工补测（本轮的自动化无法替代）：

- **WFA-99 长按 Esc**：桌面“状态 › 常驻工具 › 清洁屏幕”选 30 秒后开始，用物理键盘短按 Esc（不应退出），再按住 Esc 约 2.6 秒（应退出，原因 `esc_held`），确认输入立即恢复。
- **WFA-116 原生文件夹对话框**：在“分析 › 扫描范围 › 选择其他文件夹…”中用鼠标选中测试目录并开始扫描；对 `jroot`（WFA-07）确认开始前即以链接原因拒绝。大目录的切换与取消检查需等 NEW-01 修复后再做。
- **WFA-120 读屏**：打开 Narrator，浏览磁盘表一页与计划审阅页，并等待一次扫描完成，核对条目名称、大小、选择与展开状态、计划模式和错误都被读出，进度合并播报、完成只播报一次。

## 观察（不改判）

- 桌面运行期间，CLI 新建的扫描不会出现在概览的“其他扫描”与“活动 › 扫描”中，重启桌面后才显示。
- 部分流程之后（例如关闭检查对话框后），快捷键不起作用，直到在页面内点击一次；原因未确认，可能与 WebView 的键盘焦点有关。
- 模拟 AI 端点的回答中模型自己的英文句子（例如 “The file is not used by any program”）原样出现在中文结果里，属于模型文本。
- `self uninstall` 预览把 `index.db.damaged-*` 列为可删除的缓存，与 `doctor --repair-index` “从不删除损坏文件”的说法不冲突，但两处口径值得在文档中对齐。

## 清理与核对

- 进程：测试 ping 与副本、占用者、模拟端点、桌面程序（调试版与发行版）都已结束；最后没有 CleanManager 进程，模拟端口 18765 已关闭。
- 托盘与启动项：`CleanManager.Resident` 不存在；Windows 为测试程序路径创建的通知区域图标记录（`HKCU\Control Panel\NotifyIconSettings` 下 1 个子键，路径指向 `target\acc2-desk`）已删除。
- 注册表：6 个 `CMWFA-*` 卸载登记、`HKCU\Software\CMWFA-FooApp`、5 个 CMWFA Run 值已删除（FooTwin 在 WFA-73 中删除，FooApp 的 Run 值在 WFA-74 中由产品删除）。清理后 Run、`StartupApproved\Run`、`StartupApproved\StartupFolder`、Uninstall、`HKCU\Software` 下的 CMWFA 计数与总数都与运行前基线一致，BitBucket 卷设置回到原来的 4 项；没有创建任何策略键。
- 用户文件夹：启动文件夹中的 `CMWFA-Startup.lnk`、`%LOCALAPPDATA%`/`%APPDATA%` 下的 5 个 CMWFA 目录已删除（开始菜单快捷方式在 WFA-74 中移入回收站并已永久删除）；Downloads 计数与运行前相同。
- 回收站：本轮移入的 11 个 CMWFA 项目经产品的 `trash purge` 计划（终端 `DELETE` + 摘要）永久删除；清理后回收站为 0 项，与运行前一致，过程中没有非测试项目。
- VHD：已分离并删除，`W:` 不存在（其中的 `.cleanmanager-quarantine` 随 VHD 删除）。
- ACL 与链接：2 处拒绝 ACE 已用 `icacls /remove:d` 去除；4 个 junction/符号链接逐个用 `rmdir`（不带 `/s`）删除，前后 System32 顶层条目数都是 4,900，sentinel 不变；只读夹具 F-01 与 sentinel 的清单在清理前逐项相同。
- 存储与夹具：审计库副本留在本地证据目录后，删除了整个 `%TEMP%\CMWFA-20261023a\`（夹具、三个存储与独立 WebView2 配置）；各卷根没有 `.cleanmanager-quarantine`。
- 未改动：默认存储的文件清单、真实 WebView2 配置的 Local Storage、`EnableLUA`、中文输入法的转换模式（键盘旅程中切换过一次，已切回）。
- 构建：worktree 已用 `git worktree remove --force` 删除；`target\acc2`、`target\acc2-desk`、`target\acc2-desk-rel` 构建缓存保留在被 Git 忽略的 `target\` 下。本轮没有提交或推送。
