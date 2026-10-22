# Windows 全功能验收报告：CLI 部分（2026-10-22）

本报告记录 [WFA 文字用例](windows-full-2026-10-22-cases.md) 中 CLI 部分的执行结果。桌面 GUI 正在重新设计，本轮不在范围内。纯桌面用例，以及其他用例中的桌面部分，统一记为“延后：桌面轮”。本轮只报告缺陷，没有修改产品代码；修复由后续轮次处理。

**121 条用例：81 条通过，18 条失败（5 条中等、13 条低），2 条受阻，2 条未执行，14 条延后到桌面轮，4 条需要负责人在场。另有 1 项不属于具体用例的中等性能问题。当前版本不能判为整体验收通过。**

原始证据只保存在本地 `.research/acceptance-raw/windows-full-2026-10-22/`，该目录被 Git 忽略。其中包括每条命令的 stdout/stderr 原始字节与退出码 `commands.jsonl`、82 份控制台审阅记录 `tty/`、注册表基线 `regbase/`（只在本地保存），以及审计库副本。文中路径已按 doctor 规则脱敏：用户主目录写作 `~`，用户名写作 `<user>`，主机名写作 `<host>`。报告不列应用清单、进程清单、启动项清单或回收站清单；凡涉及真实对象，只记录计数或 CMWFA 测试对象。

## 版本与环境

- 被测版本：`main@865297019621c748cb8ceee9ae08607a46dcc294`（“acceptance round 1”）。为避免与正在修改桌面端的其他 agent 冲突，在独立的干净 worktree 中构建：`git status --porcelain` 为 0 行，`git diff` 的 SHA-256 为空串哈希 `e3b0c442…b855`。
- 构建命令：`CARGO_TARGET_DIR=<仓库>\target\acc cargo build --release -p cm-cli --locked`，使用 rustc/cargo 1.99.0。CLI `cleanmanager.exe` 的 SHA-256 为 `40e2530760d38eb9fc3b841e74b211b408834c09e84a455d7fbc42e91012bac8`（11,850,240 字节）。规则目录为 `CM_RULES_DIR=<worktree>\rules`。
- WFA-107/108 另外使用 `python -I tools/package_release.py` 在 worktree 中生成的发行布局（未签名），只用于这两条用例。
- 系统：Windows 11 企业版 25H2，build 10.0.26200.9711，x64；界面语言与区域均为 zh-CN。另记录：Git 2.55.0，Python 3.12.15，Edge 155.0.4283.62（运行期间未运行），Chrome 155.0.8059.85（运行中，因此浏览器规则夹具使用 Edge 布局）。
- **权限上下文**：本机 UAC 已关闭（`EnableLUA=0`），所有进程都以 High（完整管理员令牌）运行，包括经 `explorer.exe` 启动的进程。因此 WFA-85 无法获得 Medium 上下文，WFA-88 无法出现 UAC 提示，相关结论见下文。本轮没有修改这项安全设置。
- 存储与夹具：在 `~\AppData\Local\Temp\CMWFA-20261022a\` 下分别使用 `fx`（夹具）和 `store`（独立 `CM_STORE_DIR`），两者在同一卷。全部 1,203 条命令都通过同一测试驱动运行，并带上该存储和规则目录。唯一例外是开头一次只读的 `capabilities` 调用；运行前后，默认存储的文件清单一致。
- 运行窗口：2026-10-22 08:46–10:15 UTC。运行中按 WFA-01..03 的要求先验证默认关闭，再开启实验功能，结束时已关闭。
- 终端审阅按负责人约定的方式进行：用 `conhost.exe` 启动 `cmd.exe /v:on /c "cleanmanager … & echo 退出码 & pause"`，使 stdin 和 stdout 都是真实控制台。另一个进程用 `AttachConsole` + `WriteConsoleInputW`，在提示出现、输出稳定之后输入摘要前 8 位，或依次输入 `DELETE`/`END`/`export`/`append`/`install`/条目 ID 等确认词；Ctrl+C 通过 `GenerateConsoleCtrlEvent` 发送。每次会话都用 `ReadConsoleOutputCharacterW` 读取屏幕文字作为记录，再用 `plan show` 核对 `authorization_status`。审阅码没有通过直接写回执或改数据库获得；WFA-26 的篡改只用于证明拒绝。
- 写操作授权：负责人已预先授权 30 条真实系统写用例，范围限于带 CMWFA 标记的测试对象，包括真实回收站、HKCU 测试卸载项与 Run 值、启动文件夹快捷方式、一次性测试应用、winget 只读检查、本机模拟 AI 端点和 DNS 刷新。WFA-35 的一次性 VHDX 也属于这一范围。
- “外部进程”结论来自 `Win32_ProcessStartTrace` 进程启动跟踪，只统计父进程为 cleanmanager 的子进程。机器输出用仓库的 v1/diag Schema（jsonschema 4.26.0，取自已忽略的 `.research/python-deps`）共校验 1,074 条记录。

## 统计

| 结果 | 数量 | 用例 |
| --- | --- | --- |
| 通过 | 81 | 见逐项结果 |
| 失败（中等） | 5 | WFA-16、17、35、48、50 |
| 失败（低） | 13 | WFA-03、11、33、34、40、42、43、47、58、63、78、93、113 |
| 受阻 | 2 | WFA-51、85 |
| 未执行 | 2 | WFA-80、105 |
| 延后：桌面轮（纯桌面） | 14 | WFA-14、25、65、92、96、97、98、99、102、115、116、117、119、120 |
| 需要负责人在场 | 4 | WFA-88、100、101、118 |

“通过”只表示该用例中 CLI 可验证的检查点满足预期。“（CLI）”标注的用例，其桌面部分另行延后，不能视为整条用例已经通过。

## 失败详情

### 中等

**BUG-WFA-01（WFA-16）索引损坏时，历史、恢复和新扫描一起不可用，且没有重建途径。**
复现：关闭所有进程后，覆盖 `<S>\index.db` 的前 92 字节（审计库 `audit.db` 完好），然后运行 `operations list`、`recovery list`、`scan show <旧扫描>`，并新建一次 `scan`。
预期：明确提示索引需要重建；操作历史、隔离清单和恢复入口完整可用；新扫描可用。
实际：以上四条命令都返回 `error`/`internal_error`，退出 8，消息为 “index database is damaged … rebuild the index”，没有 `required_action`，也没有对应的重建命令。用户因此无法列出或恢复 1,177 个隔离项。只有手工把 `index.db` 移走后，历史、恢复和新扫描才恢复正常，旧扫描则显示 `not found`。`doctor` 能报告 `store.open: failed`。
疑似组件：[crates/cm-store/src/index.rs](../../crates/cm-store/src/index.rs)（`IndexStore::open` 失败即整体报错），以及 [crates/cm-cli/src/commands.rs](../../crates/cm-cli/src/commands.rs) 中的 `open_stores`（只用审计库的命令也同时打开索引）。

**BUG-WFA-02（WFA-35）卷上回收站不可用时，计划照常生成。**
复现：挂载一次性 VHDX 为 `W:`，在当前用户的 BitBucket 设置中把该卷设为 `NukeOnDelete=1`。扫描 `W:\data` 后运行 `trash --scan S --entry x`；对 `W:\tmp` 中的规则候选运行 `plan create --mode trash` 和 `clean --mode trash`。
预期：创建计划时即以 `unsupported` 拒绝，并说明没有永久删除的替代方式。
实际：`trash inspect` 返回 `trashable: true`，三条命令都生成了计划，退出 0。批准并执行其中一份后，结果为 `failed`/`trash_unavailable`（“would be permanently deleted”），文件保留。执行阶段没有降级为删除，因此安全；问题在于用户会被要求审阅并批准一份不可能执行的计划。
疑似组件：[crates/cm-platform/src/windows/actions.rs](../../crates/cm-platform/src/windows/actions.rs)（只在执行时检测），[crates/cm-cli/src/trash_cmd.rs](../../crates/cm-cli/src/trash_cmd.rs) 与 [crates/cm-cli/src/clean_cmd.rs](../../crates/cm-cli/src/clean_cmd.rs)（建计划前没有检查卷的回收站能力）。

**BUG-WFA-03（WFA-48）保护名单不可读时执行计划，会消耗掉这份计划。**
复现：先在终端批准隔离计划 P7，再把 `<S>\protection.json` 写成非法 JSON，然后运行 `plan apply P7`；修复文件后再次 `plan apply P7`。
预期：以 `protection_unavailable` 拒绝且不产生操作；修复后同一计划可以执行。
实际：第一次执行生成一条 `operation_report`，状态 `partial`、退出 5，3 项全部 `skipped`/`protected`（“protection_unavailable: …”）。修复后再次执行，只回放这份报告，计划已无法使用。另外，v1 `plan create` 在名单损坏期间仍然成功（退出 0）；而 `trash`、`clean` 建计划都被正确拒绝。文件全程未被改动。
疑似组件：[crates/cm-cli/src/apply_cmd.rs](../../crates/cm-cli/src/apply_cmd.rs)、[crates/cm-app/src/execute.rs](../../crates/cm-app/src/execute.rs)（名单故障被逐项记为跳过，而不是在执行前整体拒绝），以及 [crates/cm-cli/src/plan_cmd.rs](../../crates/cm-cli/src/plan_cmd.rs)（v1 建计划时没有因名单故障而关闭）。

**BUG-WFA-04（WFA-50）建计划时的现场核对漏报变化项。**
复现，分两种情况：
(1) 预览后改动默认选中的目录候选 `Code Cache` 内一个文件的大小，再运行 `clean --scan S --mode quarantine --non-interactive`；
(2) 删除一个 review_only 文件后，用 `--candidates` 同时点名它和其他文件。
预期：变化或消失的对象列入 `stale_items`（`changed`/`missing`），其余对象照常进入计划。
实际：(1) `stale_items` 为空，计划仍记录旧的大小（4,000 字节，实际已是 4,500 字节）；(2) 整个请求以 `policy_denied`（“probe_unknown: probe:open_file_handle:unknown”）被拒。单个文件大小变化时 `stale_items`/`plan_stale` 正常，说明问题只出在目录候选和对象消失这两种情况。执行前的逐项核验仍会兜底。
疑似组件：[crates/cm-cli/src/clean_cmd.rs](../../crates/cm-cli/src/clean_cmd.rs)（现场核对只看顶层对象；缺失对象先被占用探测判为 unknown）。

**BUG-WFA-05（WFA-17）`candidates list` 没有给出 ANA-02 要求的解释。**
复现：在 `CM_RESOLVE_TEMP`/`CM_RESOLVE_LOCALAPPDATA` 夹具上运行 `analyze`，再运行 `candidates list --set`。
预期：每个候选说明档位与理由、再生成与离线影响、允许模式、恢复限制，临时文件说明 7 天与修改时间口径。
实际：v1 `candidate` 记录没有档位、再生成/离线影响和恢复限制字段；`reason` 一律为 “matched rule”，7 天与修改时间口径只出现在 `impact` 摘要中。档位本身是正确的：`clean --dry-run` 显示 Code Cache 与 GPU 缓存为 `default_selected`，HTTP 缓存为 `review_only`（`network_redownload`），Local Storage 不是候选；但 `tier_reason` 同样只有 “matched rule”。
疑似组件：[crates/cm-cli/src/analysis_cmd.rs](../../crates/cm-cli/src/analysis_cmd.rs)、[schemas/v1/protocol.schema.json](../../schemas/v1/protocol.schema.json) 中的 `candidate` 定义。

**PERF-WFA-01（无对应用例）打开索引的命令，耗时随索引大小线性增长。**
不打开索引的命令（如 `capabilities`）约 0.02 秒。打开索引的命令（包括 `operations list`、`plan show`）在约 110 万行时约 0.95 秒；索引达到 1,519,971 行、744 MB 时，每条约 4.6 秒。`--debug` 显示时间都花在命令阶段开始之前。原因是每次打开都执行 `PRAGMA quick_check`。对整盘扫描后连续调用 CLI 的 Agent 影响明显。
疑似组件：[crates/cm-store/src/index.rs](../../crates/cm-store/src/index.rs)（`IndexStore::open`），以及 [crates/cm-store/src/audit.rs](../../crates/cm-store/src/audit.rs) 中的同类检查。

### 低

| 编号 | 用例 | 复现与实际 | 预期 | 疑似组件 |
| --- | --- | --- | --- | --- |
| BUG-WFA-06 | WFA-11 | 对 4 万文件树运行 `scan --detach`，宿主约 0.6–1.0 秒（已确认全部行后、发布终态前）被强制结束；`scan show` 返回 `interrupted`、退出 5，但 `issues` 为空（3/3 复现）。30 万文件树在中途结束时则带 `host_exited` | 宿主退出时 issues 含 `host_exited` | [index.rs](../../crates/cm-store/src/index.rs) `reconcile_hosts`：`COALESCE(report_json, …)` 沿用了宿主自己写入、不含 issue 的报告 |
| BUG-WFA-07 | WFA-03 | 实验功能关闭时，`status --experimental` 与 `clean list --bogus` 返回 `unsupported`、退出 7；开启时返回 `invalid_argument`、退出 2 | 未知选项总是 `invalid_argument`、退出 2；门禁仍不可绕过（已确认） | [commands.rs](../../crates/cm-cli/src/commands.rs) 第 24 行：门禁 `guard` 先于各命令的选项校验 |
| BUG-WFA-08 | WFA-33 | A（隔离 1,000 项）运行时执行 B（回收站 100 项），B 返回 `conflict`、退出 6（正确）。A 完成后 B 的 100 项以 `protected` 跳过，消息为 `Denied { layer: HardDeny, reason: "object identity could not be established" }` | 跳过理由应为对象已不存在，不应是保护或硬拒绝 | [crates/cm-core/src/protect.rs](../../crates/cm-core/src/protect.rs)、[crates/cm-app/src/execute.rs](../../crates/cm-app/src/execute.rs) 的前置检查顺序 |
| BUG-WFA-09 | WFA-34 | 执行中强杀进程后，`operations show` 返回 `error`/`internal_error`、退出 8，并提示运行 reconcile；`operations list` 中为 `reconciliation_required`。reconcile 给出 164 succeeded、936 not_started，没有重做 | `operations show` 返回带 `reconciliation_required` 状态的报告，而非 internal_error | [crates/cm-cli/src/apply_cmd.rs](../../crates/cm-cli/src/apply_cmd.rs) |
| BUG-WFA-10 | WFA-40 | 上述操作核对后，历史中的 `authorization_source` 由 `interactive_review` 变为 `reconciliation` | 历史保留原授权来源（人工或策略） | [crates/cm-app/src/execute.rs](../../crates/cm-app/src/execute.rs) 的核对路径 |
| BUG-WFA-11 | WFA-42 | 策略命中后执行成功，但 `plan show` 只给出 `authorization_status: "authorized"`；策略 ID 与版本只能在 `operations list` 中看到（`policy:pol_…@2`） | `plan show` 显示授权来源为该策略的 ID 与版本 | [crates/cm-cli/src/plan_cmd.rs](../../crates/cm-cli/src/plan_cmd.rs) 的 `plan_record` |
| BUG-WFA-12 | WFA-43 | 换模式、GPU Cache、另一扫描根、手选回收站、1,001 个文件的计划都返回 `needs_approval`、退出 3（正确，额度未被消耗），但消息只有 “needs approval”，没有说明未命中策略的原因 | 说明未命中原因 | [crates/cm-app/src/plans.rs](../../crates/cm-app/src/plans.rs) 的策略匹配与 [apply_cmd.rs](../../crates/cm-cli/src/apply_cmd.rs) |
| BUG-WFA-13 | WFA-47、63 | `trash --entry <指向 System32 的 junction>` 返回 `unsupported`、退出 7（“links … are never moved or followed”） | 以 `policy_denied`、退出 3 拒绝，并显示真实目标受保护 | [crates/cm-cli/src/trash_cmd.rs](../../crates/cm-cli/src/trash_cmd.rs)：链接拒绝先于保护判定 |
| BUG-WFA-14 | WFA-58 | `held.zip` 被以 `FileShare.None` 打开时，`installer scan` 直接不列出它（摘要也不计入）；已挂载的 VHDX 则正确为 `mounted`，附推出指引且不可计划 | 被占用的安装包显示为 `skip_reason: in_use`，并附指引 | [crates/cm-platform/src/windows/installer](../../crates/cm-platform/src/windows/installer.rs)：内容识别失败时静默丢弃 |
| BUG-WFA-15 | WFA-78 | 一个 Microsoft Store 通道的行（winget `msstore` 来源）为 `up_to_date`；其余 129 个商店行均为 `cannot_check` 并给出商店更新页 | 商店应用一律为 `cannot_check` | [crates/cm-core/src/updates.rs](../../crates/cm-core/src/updates.rs) 及 Windows 通道解析 |
| BUG-WFA-16 | WFA-93、94 | `process list/show` 的 `started_at` 是 FILETIME 整数（如 `134371365203426535`），`process_end` 计划正文中同样如此 | 时间为 UTC RFC3339（协议通用约定） | [crates/cm-platform/src/windows/processes.rs](../../crates/cm-platform/src/windows/processes.rs) 第 329、359 行 |
| BUG-WFA-17 | WFA-113 | `apps uninstall leftovers` 中启动项条目在其程序仍存在时为 `action: manual`、`tier: ""`；两条记录未通过 v1 Schema（tier 枚举） | 记录符合 Schema | [crates/cm-platform/src/windows/uninstall.rs](../../crates/cm-platform/src/windows/uninstall.rs)（条目以 `tier: String::new()` 构造，manual 条目未被分类） |
| BUG-WFA-18 | WFA-108（用例通过） | `self uninstall` 预览称存储中的 `features.json` 与 `diagnoses\` “not recognised as CleanManager data and is kept” | 识别为自身数据（配置／诊断记录） | [crates/cm-cli/src/uninstall_self.rs](../../crates/cm-cli/src/uninstall_self.rs) |
| BUG-WFA-19 | 多处（外观问题） | 逐项 `message` 中出现 Rust Debug 文本，例如 `IdentityChanged`、`OwnerRunning`、`PendingEvidence`、`MetadataChanged("…")`、`Denied { … }`，调试轨迹中有 `exit=Some(0)`。`message` 不作判据，因此不影响结论 | 面向人的说明 | 执行器与计划路径的错误格式化 |

## 逐项结果

“（CLI）”表示桌面部分延后到桌面轮。“替代”表示覆盖层次有降级，并已注明。

| 用例 | 结果 | 实际观察（关键字段） |
| --- | --- | --- |
| WFA-01 | 通过（CLI） | 14 项能力 `enabled=false`、`gate_passed=false`；对标能力 `unsupported`，原因写明门禁；13 条命令均 `unsupported`、退出 7，`required_action` 为开启命令；存储只多出 `lifecycle.lock`；没有子进程；核心命令可用（`self install` 因开发布局拒绝，见 WFA-107） |
| WFA-02 | 通过（CLI） | 关闭时 `plan show` 正常；终端审阅与 `apply` 均 `unsupported`、退出 7，`details.plan_id=P`；没有授权或操作；重新开启后可以审阅（输入错误码，退出 130） |
| WFA-03 | 失败（低） | `features.json` 随开关变化；开启后能力为 `unverified`，没有 `available`；`CM_EXPERIMENTAL=1` 等变量不改变门禁（仍为退出 7）。偏差：关闭时未知选项返回 7 而非 2，见 BUG-WFA-07 |
| WFA-04 | 通过 | `completed`、退出 0，31 个条目；17 个清单对象的大小全部一致；junction 记为 `boundary: junction`，其内没有条目，没有进入 sentinel 或 System32；硬链接两个名字都可浏览（`size_basis: entity`），根合计 4,279,330，不重复计算；中文名与 319 字符路径的 `raw_base64` 解码一致；JSONL 首条 `progress` 的 `sequence` 为 0、`total_units` 为 null，末行恰有一条 `scan_result`；无 BOM、无 ANSI；清单 SHA-256 不变 |
| WFA-05 | 通过 | 三个文件根均为 `failed`、`not_a_directory`、退出 8，`entries_seen` 为 0，大小为 null，条目 0 项，中文路径完整，文件不变 |
| WFA-06 | 通过 | jroot、sroot、dangling 以及带结尾 `\` 的路径均为 `failed`、`root_is_link`、退出 8，消息提示直接扫描目标，0 条目 |
| WFA-07 | 通过（CLI） | `failed`、`root_unavailable`、退出 8 |
| WFA-08 | 通过（CLI） | `partial`、退出 5；`skipped_ranges=1`，issue `permission_required` 指向 `denied`；`logical_bytes` 为 1,000；`denied` 大小为 null、`size_basis: unknown`；没有提权读取 |
| WFA-09 | 通过 | 2 万文件目录在取消前已完成（`already_terminal`，按用例要求不计）。换用 20 万文件单目录：回执 `accepted: true`/`running`，随后状态 `stopping`，终态 `cancelled`、`partial_scope`、退出 130；69,632 行分页后无重复、无遗漏 |
| WFA-10 | 通过 | 4 万文件树在 1 秒内完成，按用例换用 30 万文件树。(a) `cancelled` 带 `timeout` issue（“nothing was rolled back”），退出 8，已确认的行可查；(b) 等待 1 秒时为 `running` 的 `scan_status`、退出 8，之后 `completed` |
| WFA-11 | 失败（低） | 30 万文件树：`interrupted`、`partial_scope`、`host_exited`、退出 5，行可查；4 万文件树缺少 `host_exited`，见 BUG-WFA-06 |
| WFA-12 | 通过 | limit 2 共 16 页、31 项不重复，末页游标为 null；默认 50；`--limit 0`/`501` 为 `invalid_argument`、退出 2；改变排序、名称、limit 或扫描后，游标为 `cursor_stale`、退出 6 |
| WFA-13 | 通过（CLI） | 文件中 `big.bin` 最大；多次查询顺序稳定；跨页降序单调，未知大小排在最后；文件与目录过滤准确；中文名命中 1 项，不存在的词 0 项；`--parent` 正确 |
| WFA-14 | 延后：桌面轮 | 纯桌面 treemap |
| WFA-15 | 通过（CLI） | 只有 `tagged` 带 `cleanable_hint`（`cachedir_tag`，`flow: manual_trash`）；`badtag` 与 `.ssh` 没有；没有生成计划或操作 |
| WFA-16 | 失败（中） | 正常情况下跨进程查询旧扫描与时间正常；索引损坏后全部不可用，见 BUG-WFA-01 |
| WFA-17 | 失败（中） | 有 `resolver_override`、规则 ID 与版本、风险、允许模式、绑定扫描修订号；新文件不是候选；缺少解释字段，见 BUG-WFA-05 |
| WFA-18 | 通过 | user-temp 规则只匹配根下直接子项，所以 `temp\project.db` 与 `cachey\…` 都不是候选。补充探测：根下直接放置的 `project.db` 为 `blocked`（`protected:database`）。观察：根下直接放置的 `id_ed25519` 是 `review_only` 候选，未按凭据保护（保护类按目录判断） |
| WFA-19 | 通过 | 扫描 `partial`；可读的 `top1`/`top2` 照常列出；`denied` 中没有候选；没有以整个目录为对象的动作。覆盖缺口只通过 `scan_completeness: partial_scope` 表达 |
| WFA-20 | 通过（CLI） | 名称在 JSON 中原样转义；档位不受影响；`apply --non-interactive` 为 `needs_approval`、退出 3 |
| WFA-21 | 通过 | quarantine 与 trash 两份计划的 ID 和 64 位摘要都不同，各自单一模式、3 个带身份的对象，`expires_at` 为创建后 900 秒，授权状态为 required；`--mode delete` 为 `policy_denied`、不生成计划；两次 `plan show` 摘要相同；后放入的文件不在计划中；文件未被移动 |
| WFA-22 | 通过 | 审阅界面显示模式、数量、20 项与 `plan show` 全量入口、大小与摘要；错误码退出 130，Ctrl+C 中断，两者都没有授权；正确码后为 `authorized`（`interactive_review`、主体与时间）；`revoke` 后为 `revoked`，`apply` 为 `needs_approval`，文件不变 |
| WFA-23 | 通过 | (a) `--non-interactive`、(b) 管道输入、(c) 控制台中 stdout 重定向（`invalid_argument`，没有等待输入）；(d) `--yes`/`--approve`/`--force`/`-y` 均退出 2；(e) `needs_approval`、退出 3 并带 plan_id；(f) 非交互启用策略退出 2；最终仍为 required，文件不变 |
| WFA-24 | 通过 | 重做后的结果：第一次运行中，测试驱动没有强制核对时钟，在过期前就做了审阅与执行，该次作废并记录。过期后 `plan show` 为 `expired` 且摘要不变；终端审阅与两次 `apply` 均 `plan_stale`、退出 6；没有操作；文件不变 |
| WFA-25 | 延后：桌面轮 | 桌面审阅与批准 |
| WFA-26 | 通过 | 把 `audit.db` 中 P6 正文的 mode 改为 delete、摘要不变：`plan show`/`apply` 返回 `internal_error`（“digest does not match its body”），退出 8；没有操作，文件不变 |
| WFA-27 | 通过（CLI，替代） | 改为终端批准并用 CLI 执行（桌面批准与执行延后）：3 项 `succeeded`；隔离位置在 `<S>\quarantine\op_…` 下，同卷，SHA-256 不变；`processed`=`quarantined`=3,145,728，`estimated_freed` 为 "0"，带卷空闲前后值与有符号差值；恢复入口为 `in_app_restore` |
| WFA-28 | 通过 | 3 项进入当前用户回收站，原路径正确；`estimated_freed` 为 "0"；恢复入口为 `system_recycle_bin` 并附指引；对其中 1 项调用回收站对象的 `undelete` 动词（即资源管理器“还原”）后，文件回到原处，SHA-256 不变 |
| WFA-29 | 通过（CLI） | 100 MiB 安装包以 delete 模式执行：文件消失且不在回收站；`estimated_freed`=104,858,827；观测差值 104,828,928（如实给出，不等于文件大小）；恢复为 `unavailable`；审阅界面写明 “no undo” |
| WFA-30 | 通过（CLI） | 连续两次 `apply` 返回同一个 `operation_id` 和原报告，没有新的移动 |
| WFA-31 | 通过 | 被替换的文件为 `skipped`/`precondition_identity_changed`，替代文件原地保留；另一项 `succeeded`；整体 `partial`、退出 5 |
| WFA-32 | 通过 | 失败项为 `in_use`，消息为 “powershell.exe (Windows PowerShell, pid N)”，与测试进程一致；进程仍在，文件大小不变；同一计划再次执行回放原报告；释放后新建计划成功进入回收站 |
| WFA-33 | 失败（低） | 重叠时 B 返回 `conflict`、退出 6，没有对象被处理两次；之后 B 的跳过理由不当，见 BUG-WFA-08。注：Windows 命令行长度限制 `--candidates` 最多约 1,100 个 ID，因此用 1,000 项代替 2,000 项 |
| WFA-34 | 失败（低） | 1,100 项计划执行中被强杀：已移动 164 项；`operations list` 为 `reconciliation_required`/`unknown`；reconcile 给出 164 succeeded、936 not_started，没有重做；再次 `apply` 回放；164 项都可在恢复列表中查到。偏差见 BUG-WFA-09 |
| WFA-35 | 失败（中） | 见 BUG-WFA-02。VHDX、盘符和 BitBucket 设置都已移除 |
| WFA-36 | 通过 | 336 字符路径与中文名文件的隔离、恢复与回收站往返均成功，UTF-16 原名一致，隔离往返的 SHA-256 不变。观察：长路径项在 `trash list` 中的 `original_path` 带 `\\?\` 前缀（340 字符），资源管理器的“原位置”列为空；还原时需按名称在回收站中操作 |
| WFA-37 | 通过 | 列表含计划、操作、原位置、隔离位置、`recoverable`、`reminder_due: false`、`restore_capability`；恢复计划为 recovery/v1，有独立摘要、15 分钟有效期，未批准时退出 3；执行后文件回到原处，SHA-256 与隔离前一致，并从列表移除 |
| WFA-38 | 通过 | `skipped`/`destination_exists`，退出 5；新文件不变；隔离项仍在 |
| WFA-39 | 通过 | 清空计划需要单独授权（未批准退出 3）；执行后对象被删除，`estimated_freed`=1,048,576，历史为 `unavailable`；其他项仍在 |
| WFA-40 | 失败（低） | 19 条操作都有计划类型、授权来源、规则版本、时间与计数；恢复入口与类型对应（隔离/回收站/删除与清空/未确认）。偏差见 BUG-WFA-10 |
| WFA-41 | 通过（CLI） | 隔离 104,857,600 字节：报告的已处理量与已隔离量均为该值，预计释放 "0"，观测差值 0（`Get-Volume` 独立测得 −20,480）；对照 WFA-29 的删除报告，预计释放等于文件大小，观测差值如实给出 |
| WFA-42 | 失败（低） | 新策略为 paused，此时 `apply` 为 `needs_approval`；终端输入策略 ID 后启用；新计划无需审阅即执行，用量记录为 53,072 字节/3 项。说明：`--all-default-selected` 同时带入 GPU Cache 和 Profile 1，不能被策略匹配，因此命中改用点名的 Default Code Cache。偏差见 BUG-WFA-11 |
| WFA-43 | 失败（低） | (a)–(e) 均 `needs_approval`、退出 3，(e) 不消耗额度；(f) 暂停或撤销后需要人工审阅，撤销后不能再启用（`conflict`），已完成的操作不受影响。偏差见 BUG-WFA-12 |
| WFA-44 | 通过（CLI） | 条目显示层级、类型与备注；`protect check` 逐层给出理由；手选计划为 `policy_denied`、退出 3 |
| WFA-45 | 通过 | f1 `succeeded`；f2 `skipped`/`protected`；整体 `partial`、退出 5 |
| WFA-46 | 通过（CLI） | 非交互移除被拒且没有等待输入；终端输入条目 ID 后移除；内置条目不可移除 |
| WFA-47 | 失败（低） | 两种大小写都判为硬拒绝；对 `C:\Windows`、`..\x`、`\\?\C:\Windows` 的 add 都被拒；`fake-home` 被拒并列出 `.ssh`；`link-sys` 的拒绝方式有偏差，见 BUG-WFA-13 |
| WFA-48 | 失败（中） | `protect list` 为 `protection_unavailable`、退出 8；只读扫描与查询继续工作；`trash`/`clean` 拒绝建计划；名单恢复后正常。偏差见 BUG-WFA-03 |
| WFA-49 | 通过（CLI） | 15 个类目带规则 ID、版本、状态、`tested_range`/`tested_at`/`installed_version`；只有 experimental 规则的分组为 `unverified`；回收站类目为 `unsupported`；适配器只运行只读预览（只启动过 `node` 的 `config get cache`），不可用的适配器给出原因 |
| WFA-50 | 失败（中） | 非交互计划的对象集合与预览的 `selected_by_default` 集合相同；预览不产生计划、授权或操作；review_only 只能通过点名进入且仍需授权；没有可选项时为 `policy_denied`。偏差见 BUG-WFA-04 |
| WFA-51 | 受阻 | `blocked` 行为 `policy_denied`、退出 3 并给出理由；任何预览中都没有“所属工具行”（适配器只出现在 `clean list`，真实 npm 缓存扫描也只产生文件规则行），这一部分无法验证；没有“大项提示行” |
| WFA-52 | 通过 | 被占用的下载为 `in_use` 并跳过；释放后为 `not_in_use`，仍为 review_only；Edge（无界面、一次性配置目录）运行时，Code Cache 两项为 `skipped`/`precondition_owner_running`；浏览器没有被结束，之后按 PID 关闭 |
| WFA-53 | 通过（CLI） | 默认根只有一个（本机工作区）；添加或移除只写入 `purge-settings.json`；移除默认根记为排除，重新添加后恢复；结束时已移除测试根 |
| WFA-54 | 通过 | `rustold\target` 默认选中；`node_modules` 为 review_only（需联网重装）；`dist` 为 blocked（`vcs_tracked:1_files`）；`recent` 不预选；`nomarker` 不是候选；`withsecret` 为 blocked（密钥文件）；`brokengit` 为 `vcs_unknown`，不可计划；非交互计划只含 rustold |
| WFA-55 | 通过 | 默认模式为 trash；`node_modules` 的动作为 `protected`（`vcs_tracked`）；`x.o` 进入回收站；含新文件的目录为 `precondition_new_descendant`，没有递归删除 |
| WFA-56 | 通过 | 位置注明来自验证夹具覆盖；appx/del100/held/replace/desk 为候选，带来源标签与大小，全部 review_only 且无预选；docs.zip/fake.msi/note.pdf 不出现；`--source desktop` 只返回桌面来源 |
| WFA-57 | 通过 | 非交互 `--source` 在命令运行前被拒（退出 2）；终端中先做一次完整发现，再生成只含 downloads 来源 4 项的计划，仍需审阅。第一次终端尝试沿用了“最近一次发现”（当时只扫了 desktop），属于驱动的顺序问题 |
| WFA-58 | 失败（低） | 已挂载的 VHDX 为 `mounted`、附推出指引、不可计划；被占用的安装包未被列出，见 BUG-WFA-14 |
| WFA-59 | 通过 | 默认模式为 trash；`replace.zip` 为 `precondition_metadata_changed`，新文件保留；`appx.zip` 进入回收站；`partial`、退出 5 |
| WFA-60 | 通过（CLI） | 返回大小、覆盖缺口、受保护后代、占用者（名称与 PID）、路径；受保护、链接或覆盖不全的对象只有 `show_path`/`copy_path` |
| WFA-61 | 通过 | `manual_selection`，动作只有 `fs_trash`，`policy_matchable: false`；`--mode` 为未知选项、退出 2；`apply` 为 `needs_approval`、退出 3 |
| WFA-62 | 通过（CLI） | e1、e2 进入回收站，原路径正确；e1 用回收站对象动词还原后 SHA-256 不变；历史为 `system_recycle_bin` |
| WFA-63 | 失败（低） | 凭据、数据库与受保护后代均为 `policy_denied`、退出 3；扫描根本身为 `invalid_argument`（“scan root itself cannot be selected”）；对非根的不完整目录另行验证，为 `policy_denied` 并说明覆盖缺口；`link-sys` 见 BUG-WFA-13 |
| WFA-64 | 通过（CLI） | 独立的 `manual_selection`/`fs_delete` 计划；审阅时先输入 `DELETE` 再输入摘要；两项（4 步）从回收站消失，`estimated_freed`=1,055,936，带卷空闲观测；历史为 `unavailable`；非测试回收站项为 0，未受影响 |
| WFA-65 | 延后：桌面轮 | 桌面手选回收站旅程 |
| WFA-66 | 通过（CLI） | 9 项已知对象（分页、交换、休眠、WSL、Docker、传递优化、卷影、WinSxS 等），带用途、处理途径（系统设置或所属工具）与 `requires_admin`；WinSxS 为 null 并说明原因；没有删除入口；没有启动外部工具 |
| WFA-67 | 通过（CLI） | 每张卡片要么有已知大小，要么为 null 加 `size_note`，并带 `observed_at` 与链接；快照卡片给出数量 |
| WFA-68 | 通过 | 真实 Downloads 中的测试文件出现在按大小排序的列表中，全部 `selected: false`，`handling: manual_trash_review`；经授权的破坏性部分只扫描并移走该测试文件，Downloads 的其他文件未变（16→15） |
| WFA-69 | 通过（CLI） | FooApp 与 FooTwin 是两个独立实例（ID 与位置不同）；`uninstall_capability` 与 `apps uninstall list` 一致；读取清单时注册表不变 |
| WFA-70 | 通过 | ScriptApp 为 `unsupported`、退出 7，并指向 `ms-settings:appsfeatures`；Mismatch 为 `policy_denied`（“signed by Microsoft Windows, not by Contoso Test”）；受保护的系统组件为 `policy_denied`；两个 npm AI CLI 显示 npm 路由，本轮未为其建计划 |
| WFA-71 | 通过 | `uninstall` 计划含 `app_quit` 与 `tool_adapter`（程序路径、SHA-256、参数），附 `leftover_estimate`；非交互为 `needs_approval`；执行后卸载程序以退出码 0 运行约 114 ms；重新扫描后为 `partial`/`leftovers_remain`，`payload` 已删除，安装目录与登记仍在 |
| WFA-72 | 通过 | 关闭请求后有限等待约 16 秒，结果为 `still_running` 并列出 PID，卸载程序没有运行，进程仍在；非交互 `--end-running` 退出 2；可选的终端强制计划在输入 `END` 后只结束了该测试 PID |
| WFA-73 | 通过 | 有 FooTwin 时同名数据为 `shared_owner`/`kept`；删除 FooTwin 后分组为：本体、缓存与日志（默认选中）、设置（含注册表键，不预选）、个人数据（models 附重新下载说明；credentials 有敏感警告）、启动项（Run 值为 `manual`，开始菜单快捷方式）。每项都有证据 |
| WFA-74 | 通过 | 单独审阅的 `uninstall` 计划：文件与快捷方式进入回收站，空目录保留；两个注册表键先备份再删除；缓存与日志默认选中并一并纳入；未选的 models/settings/credentials 保留；`completed`。程序消失后，Run 值在第二次残留审阅中可以计划，备份后删除 |
| WFA-75 | 通过 | `recovery` 计划按备份重建两个键与 Run 值，值与类型一致；手工重建同名键后，新的恢复计划为 `destination_exists`，没有覆盖 |
| WFA-76 | 通过（CLI） | 一份计划含 Bar 与 Baz，Mismatch 列入 `uninstall_refused`（退出 5）；审阅界面按应用分组；Bar 为 `uninstaller_failed`（退出码 3），Baz 为 `leftovers_remain`，整体 `partial`；残留中 Bar 为 0 项，Bar 仍处于安装状态 |
| WFA-77 | 通过 | Orphan 及同名数据目录为 review_only、不预选，并说明缺失的程序路径；Orphan2 的数据目录为 `owner_unknown`/`kept`；Windows 上建计划为 `unsupported`、退出 7；没有通用名称误匹配 |
| WFA-78 | 失败（低） | 343 行都带当前与可用版本、通道、通道内标识与检查时间；winget 有 28 行最新、11 行有更新；没有通道的为 `cannot_check` 并说明原因；调试轨迹只显示 winget 的退出码与耗时；只启动了 `winget.exe`。偏差见 BUG-WFA-15；本机没有自更新类应用可供验证 |
| WFA-79 | 通过（CLI） | 关闭检查后：没有启动 winget（进程跟踪），调试轨迹只有 29 ms 的本地处理，结果为上次结果与原检查时间；设置已恢复为开启 |
| WFA-80 | 未执行 | 需要负责人指定可更新的 winget 包；安装不在授权范围内 |
| WFA-81 | 通过（CLI） | CMWFA-Run 的来源、目标、范围、启用状态与可启停性均正确；启动文件夹快捷方式正确；Broken 为 `broken`；Offline 为 `target_unreachable`；系统、安全与本工具的条目受保护；临时 HKCU RunOnce 测试值为 `unsupported`（随后已删除）；计划任务为 `unverified` 并附指引；本机没有策略 Run 项。注：在 High 上下文中运行 |
| WFA-82 | 通过 | 停用只写入 `StartupApproved\Run`（`03…` 加时间），Run 值不变；启用写回 `02…`；恢复方式为 `in_app_restore`；被其他操作改过后恢复为 `changed_since`。观察：原本没有记录时，恢复会写入显式的 `02…`。未目视检查任务管理器 |
| WFA-83 | 通过 | 只改 `StartupApproved\StartupFolder`，`.lnk` 的 SHA-256 不变；对快捷方式的 `remove-stale` 为 `unsupported` |
| WFA-84 | 通过 | 失效值先备份再删除；恢复后值与类型一致；手工重建同名值后为 `destination_exists`；Offline 与目标存在的 Run 值分别以 `conflict` 拒绝 |
| WFA-85 | 受阻 | UAC 关闭，`explorer.exe` 启动的进程同样为 High，无法获得 Medium 上下文；本机没有未受保护的 HKLM Run 项，`permission_required` 路径无法验证。已验证部分：受保护的 HKLM Run 项为 `policy_denied`、退出 3；计划任务为 `unsupported`（`unverified`），退出 7；没有为真实条目生成计划 |
| WFA-86 | 通过（CLI） | 5 项任务都有 ID 与版本、问题、影响、是否需管理员、适用性与预计耗时；搜索“注册表优化”只返回禁止项及原因；没有百分比承诺 |
| WFA-87 | 通过 | 非交互为 `needs_approval`；执行后为 `applied`/`flushed`，六类计数正确，`all_clear`，退出 0；历史含任务版本、入口与执行者；操作为 `maintenance`，恢复为 `unavailable` |
| WFA-88 | 需要负责人在场 | 见下节；本机 UAC 已关闭 |
| WFA-89 | 通过 | 列表显示 `user_protected`；批量计划不含该任务（未执行）；点名为 `policy_denied`；保护已在终端移除 |
| WFA-90 | 通过（CLI） | 负载进程以 `busy_process` 列出（名称、PID、路径、5 秒内 99%）；`read_only` 为真；进程没有被结束 |
| WFA-91 | 通过（CLI） | 不可得的 4 项（内存压缩、故障预测、风扇、GPU 温度）为 null 并附原因；健康分 100，权重 30/25/20/15/10，有 `lowered_by`；诊断为 `normal`；网络合计只计物理网卡、排除回环，虚拟网卡单列；没有僵尸进程字段；JSONL 3 个快照带 `sampled_at`（采样及时，未出现 `stale`） |
| WFA-92 | 延后：桌面轮 | |
| WFA-93 | 失败（低） | 按 cpu、memory、name 排序正常；签名为 `catalog_signed` 等状态；副本未被识别，并提供 AI 诊断入口；置顶项排在最前，取消置顶正常；时间格式偏差见 BUG-WFA-16 |
| WFA-94 | 通过 | 计划恰好一个 `process_end`，绑定 PID、开始时间、路径与 SHA-256；未批准为 `needs_approval`；批准后只结束测试进程；目标已退出时为 `precondition_gone`；PID 4、csrss、lsass、dwm、winlogon、CleanManager 自身（扫描宿主）与安全软件均为 `policy_denied` |
| WFA-95 | 通过 | 默认关闭；开启后 80 秒的状态流中，第一条提醒对应负载进程甲；冷却期内换成负载进程乙，没有重复提醒；冷却期满后才出现第二条；关闭后没有提醒。观察：第二条的 `since` 沿用了进程甲的时间 |
| WFA-96–99 | 延后：桌面轮 | 托盘、登录启动值、保持亮屏、清洁屏幕 |
| WFA-100 | 需要负责人在场 | 另含桌面部分 |
| WFA-101 | 需要负责人在场 | 另含桌面部分 |
| WFA-102 | 延后：桌面轮 | |
| WFA-103 | 通过（CLI） | `configured: false`，不含密钥；`preview` 在 0.02 秒内返回 `unsupported`、退出 7，没有等待输入 |
| WFA-104 | 通过（CLI，替代） | 提供方改为直接写入 `<S>\diag-providers.json`，指向 127.0.0.1 的模拟端点（桌面设置、测试连接与取消延后）。配置与三次预览都没有产生请求；错误摘要为 `conflict`、退出 6，没有请求；正确摘要时只发出一次 POST，请求体与预览逐字节相同，没有认证头；载荷不含完整路径与 `.ssh` 名称，指令样文件名被省略并记录缺口 `name_instruction_like`；回答中的 `rm -rf`/`Remove-Item`/`del` 与“已批准”被剔除（`sanitized` 中 4 项），推荐降为 `needs_context`，没有生成计划；`show`/`list` 为同一 ID；配置与 doctor 中没有密钥 |
| WFA-105 | 未执行 | 没有负责人密钥 |
| WFA-106 | 通过 | 关闭时为 `unsupported`、退出 7（WFA-01）；开启后为 `cannot_check`、退出 7（非发行布局）；本地未签名清单同样如此；没有子进程；程序 SHA-256 不变 |
| WFA-107 | 通过 | 使用发行布局：非交互只给出预览，目录为空；非空目录为 `conflict`、退出 6；终端输入 `install` 后复制 95 个文件，哈希全部与清单一致，用户 PATH 不变；重复安装为 `conflict` |
| WFA-108 | 通过 | 预览列出缓存，发行安装时还列出 101 个程序文件；配置、保护名单、历史与隔离区默认保留；隔离区非空时给出 1,177 项、114,422,352 字节以及恢复与清空命令；`--remove-user-data` 不生效（`user_data_kept`）；`CleanManager.Resident` 显示为不存在；非交互不移除任何东西。“值存在”的一半延后到桌面轮。另见 BUG-WFA-18 |
| WFA-109 | 通过 | `print --format text` 可在 pwsh 中加载；非交互为 `preview`、`written: false`，文件未创建；终端输入 `append` 后追加带标记的块；第二次为 `already_installed`；真实 `$PROFILE` 未改动 |
| WFA-110 | 通过 | 报告含版本、平台、能力、权限上下文、配置与存储自检、13 个错误码、最近的非成功代码；默认输出不含用户名、主机名或 8.3 短名；非交互导出为 `written: false`；终端输入 `export` 后写出新文件（同样已脱敏）；目标已存在时为 `conflict`；`--no-redact` 只在本地查看 |
| WFA-111 | 通过（CLI） | `help` 文本随 `--lang`/`CLEANMANAGER_LANG` 变化；`scan show`、`capabilities` 与错误记录在三种设置下，除 request_id 外 JSON 逐字节相同；error message 为英文。桌面语言（Q3）延后 |
| WFA-112 | 通过 | 带与不带 `--debug` 时，stdout 除 request_id 外相同，退出码相同；不带时 stderr 为空；轨迹每行符合 `[debug +Nms] 阶段: …`，含阶段耗时、后端与理由；路径显示为 `~`；`diag config` 只写 `key=absent` |
| WFA-113 | 失败（低） | 未知选项、重复选项、`-y`、重复 `--format`、`help --json` 与缺值均为 `invalid_argument`、退出 2；非 TTY 默认输出 JSON；1,074 条记录中只有 2 条未通过 Schema（BUG-WFA-17）；错误码与退出码对照 0 处不符 |
| WFA-114 | 通过（静态） | 先生成被忽略的打包件，之后 `package_skill.py --check` 通过；干净检出时因打包件缺失，`--check` 报告 “out of date”（观察）。SKILL.md 明确禁止自批准与 shell 删除，管理类变更只在用户明确要求时发起。桌面 Agent 页延后 |
| WFA-115–117、119、120 | 延后：桌面轮 | |
| WFA-118 | 需要负责人在场 | 另含桌面部分 |
| WFA-121 | 通过 | 清理前：F-01 与 sentinel 的清单逐项相同；注册表、启动文件夹、开始菜单、Downloads 与回收站的变化只涉及 CMWFA 对象，非测试项的计数都不变；没有残留测试进程；默认存储的文件清单不变；各卷根没有 `.cleanmanager-quarantine`。清理后实验功能已关闭，`CleanManager.Resident` 不存在 |

## 受阻与未执行

- **WFA-51（受阻）**：缺少会产生“所属工具行”的预览。适配器只出现在 `clean list`；本机可用的 npm 适配器所在缓存，扫描后也只得到文件规则行。需要一个能产生适配器行的扫描范围，或由产品说明这类行在 `clean` 预览中的预期形态。
- **WFA-85（受阻）**：缺少 Medium 上下文（UAC 已关闭，`EnableLUA=0`），也缺少未受保护的 HKLM Run 项。可在启用 UAC 的普通终端中重跑，并需要一个可测试的系统级 Run 项（只能由负责人决定是否创建）。
- **WFA-80（未执行）**：负责人没有指定可安全更新的 winget 包，安装不在授权范围内。
- **WFA-105（未执行）**：没有真实 AI 提供方的密钥与披露同意。
- 其他标为“（CLI）”的用例，其桌面部分以及 WFA-14、25、65、92、96–99、102、115–117、119、120 都属于桌面轮。

## 需要负责人在场的用例（步骤供主会话使用）

所有命令都在新的独立存储中运行：`$env:CM_STORE_DIR = '<新目录>'`，`$env:CM_RULES_DIR = '<仓库>\rules'`，并先运行 `cleanmanager experimental --state on`。结束时运行 `--state off`，再删除该存储。

1. **WFA-88（一次 UAC：同意与拒绝）**：前提是本机 UAC 当前为关闭状态（`EnableLUA=0`），所有进程都已是完整管理员，因此不会出现 UAC，用例无法按原样执行。是否临时启用 UAC 属于安全设置变更，需要负责人决定，并重启后在**普通（非提升）**终端中执行，用 `whoami /groups` 确认为 Medium。
   (1) 运行 `cleanmanager optimize plan create --tasks windows.dns-flush,windows.image-health-check --json` 得到计划 A，在终端运行 `cleanmanager plan review <A>` 并输入摘要前 8 位，然后运行 `cleanmanager plan apply <A> --json`。负责人在唯一一次 UAC 中点“是”。核对：只提示一次；`elevation=granted`；image-health-check 为 `unchanged`（健康，只读）；dns 为 `applied`；结束后没有残留的提权 cleanmanager 进程（`Get-Process cleanmanager`）。
   (2) 内容相同的计划 B，审阅后执行，负责人在 UAC 中点“否”。核对：image-health-check 为 `skipped`/`elevation_declined`，dns 仍为 `applied`，退出 5，没有运行计划外的任务。
2. **WFA-100（摄像头与麦克风提示，需桌面）**：在桌面打开“状态 › 常驻工具”。负责人打开系统“相机”（或“录音机”）后再关闭。核对：3 秒内显示正在使用的应用名，关闭后提示消失；对比前后 `<S>` 的文件列表，没有新增使用历史；记录不可读时显示“不可用”及原因。本机没有设备时记为受阻。
3. **WFA-101（电量与低电量提醒，需桌面与笔记本电池）**：页面显示本机电量与充电状态，蓝牙区域显示“没有已连接的蓝牙设备”。开启低电量提醒，阈值不低于当前电量；负责人拔掉电源。核对：只提醒一次（托盘读数、页面与读屏播报，按 Q8 判定），接回电源后不再提醒。再分别输入阈值 4 和 51，确认只接受 5–50%。
4. **WFA-118（100/125/150% 缩放，需桌面）**：负责人在“设置 › 系统 › 屏幕 › 缩放”中依次设为 100%、125%、150%，每次重启应用。检查概览、磁盘（表格与 treemap）、计划审阅、结果页与设置，并把窗口缩到约 800 px 宽。核对：主要操作可用，文字不截断，页面无横向溢出，treemap 重绘正确。最后恢复原缩放。

## 观察（不改判）

- 每次 `--candidates` 只能传约 1,100 个 ID（受 Windows 命令行长度限制），也没有文件或 stdin 输入方式。
- `installer plan create --source` 只使用最近一次发现运行的候选集。
- 用户临时目录规则会把根下直接放置的私钥形文件名（如 `id_ed25519`）列为 review_only 候选，没有按凭据保护；构建产物与卸载残留则各自有密钥名清单。
- 扫描 issue 的 `message` 中含本地化的系统错误文本（例如“拒绝访问。(os error 5)”）。各语言设置下 JSON 仍逐字节相同。
- 由于 UAC 关闭（High 上下文），HKLM 启动项的可启停性按管理员显示；WFA-87 历史中 `elevated: true`，而 `elevation: not_needed`。

## 清理与核对

- 测试进程：所有 ping、负载、占用者、无界面 Edge 与模拟端点都按记录的 PID 结束；最后检查没有残留。
- 回收站：通过产品的 `trash purge` 计划（终端先输入 `DELETE`，再输入摘要）删除全部 10 个 CMWFA/夹具项（20 步全部成功）。清理后回收站为 0 项，与运行前一致。整个过程中非测试项一直为 0。
- 注册表：已删除 6 个 `CMWFA-*` 卸载登记（FooTwin 在 WFA-73 中删除）、`HKCU\Software\CMWFA-FooApp`、6 个 CMWFA Run 值，以及 `StartupApproved\Run` 与 `StartupApproved\StartupFolder` 中的 CMWFA 记录。RunOnce 测试值已当场删除。清理后对 Run、`StartupApproved\Run`、`StartupApproved\StartupFolder` 与 Uninstall 四处 `reg export`，与运行前的基线**逐字节相同**。`CleanManager.Resident` 不存在。
- 用户文件夹：已删除启动文件夹与开始菜单中的 CMWFA 快捷方式（开始菜单那个已随回收站清理）、`%LOCALAPPDATA%`/`%APPDATA%` 下的 5 个 CMWFA* 目录，以及 Downloads 中的测试文件。Downloads 计数回到 15。
- VHD：已分离并删除，BitBucket 卷设置回到原来的 4 项，`W:` 不存在。
- ACL 与链接：3 处拒绝 ACE 已用 `icacls /remove:d` 去除。4 个 junction/符号链接逐个用 `rmdir`（不带 `/s`）删除；dangling 符号链接被列为文件项，删除时没有跟随。删除前后 System32 顶层条目数都是 4,900，sentinel 不变。
- 存储与夹具：确认 1,177 个隔离项的原位置全部在夹具根内之后，删除了整个 `~\AppData\Local\Temp\CMWFA-20261022a\`；`audit.db` 与设置文件的副本只在本地保留。各卷根都没有 `.cleanmanager-quarantine`。
- 最终核对：注册表、用户文件夹、回收站、卷根与默认存储的快照，都与运行前的基线一致。
- 构建：worktree 已用 `git worktree remove --force` 删除，其中包括发行布局与打包件。`target\acc` 构建缓存保留在已被 Git 忽略的 `target\` 下。本轮没有提交或推送。
