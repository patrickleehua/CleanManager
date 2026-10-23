# Windows 桌面验收复测报告（2026-10-23b）

本报告记录对 [WFA 文字用例](windows-full-2026-10-22-cases.md) 的第三轮执行，范围是 [上一轮复测报告](windows-full-2026-10-23-retest-report.md) 的遗留项：(A) 全部桌面失败与“（CLI）”用例桌面部分的失败（含高严重度的大扫描概览问题与性能测量）、上一轮未执行的 5 个桌面部分、3 个 CLI 低严重度问题；(B) 本轮改动涉及的桌面旅程回归；(C) 需要负责人在场的用例，写成可直接照做的步骤。本轮只报告问题，没有修改产品代码。

**结论：上一轮 17 个 NEW 问题中 13 个已按原现象复测通过；NEW-01（高，大扫描概览卡住）已修复，发行版在 2 万、4 万、12 万条目的扫描上概览约 0.23 秒显示，扫描发布期间其他页面 0.1–0.4 秒内渲染。仍未完全修复的是 NEW-08（链接到 System32 的对象仍可“选择”并出现“移到回收站…”）、NEW-12 的桌面部分（未核对操作的授权来源显示为“—”，残留操作在“活动”中标成“可在隔离区恢复”）和 NEW-13 的一处残留英文；NEW-16 因本机 `powercfg` 看不到任何进程级电源请求而无法判定。本轮新发现 7 个低严重度问题（含 1 个测试钩子问题），没有高或中严重度问题。A 部分 23 项：通过 19、失败 3（均为低）、受阻 1。B 部分 8 个旅程全部通过（键盘旅程有 1 个低严重度观察）。当前版本仍不能判为整体验收通过：负责人在场项（C）尚未执行，WFA-88 仍受阻。**

原始证据只保存在本地被 Git 忽略的 `.research/acceptance-raw/windows-desktop-2026-10-23b/`：171 条 CLI 命令记录（`commands.jsonl`、`out/`）、11 份真实控制台审阅记录（`tty/`）、桌面驱动日志 `desk-log.md` 与应用窗口截图（`desk-shots/`，含本机路径，不提交）、基线（`json/`）、模拟 AI 端点收到的请求、`powercfg` 输出与审计库副本。本文按约定脱敏：用户主目录写作 `~`，用户名 `<user>`，主机名 `<host>`，SID `<sid>`；不列应用、进程、启动项、更新或回收站清单，只记录 CMWFA 测试对象与计数。

## 版本与环境

- **被测版本**：`main@9393a6c4851f9b891cfe7db580630b6cedf0323b`（“fix: desktop acceptance round — large-scan performance, CLI parity, history facts, desktop pages”）。只在独立干净 worktree（`git status --porcelain` 0 行）中构建，结束后已删除该 worktree。
  - CLI：`CARGO_TARGET_DIR=<仓库>\target\acc3 cargo build --release -p cm-cli --locked`，`cleanmanager.exe` SHA-256 `2804d5109b7d1181727804d61ecf0f3f55d46c7b84fdc1ec47196a9c4f7df74c`（12,127,744 字节），`CM_RULES_DIR=<worktree>\rules`。
  - 桌面调试构建：`npm ci` 后 `npm run tauri -- build --debug --no-bundle`（`CARGO_TARGET_DIR=<仓库>\target\acc3-desk`），`cleanmanager-desktop.exe` SHA-256 `3ddadc4d7b3f3e89661bfaaa53b9055b42858f4a898166f2b44665b3cdf208af`。除性能项外，桌面结果都来自这个**调试构建**。
  - 桌面发行构建（仅性能项）：同一 worktree `npm run tauri -- build --no-bundle`（`target\acc3-desk-rel`），SHA-256 `543f71fb6ff418ba0bea1747998cdfb5c739cecaded9830b5951559ee49a95a5`。
  - 工具链：rustc 1.99.0，Node 24.21.0，npm 11.19.0，Git 2.55.0，Python 3.12.15。
- **系统**：Windows 11 企业版 build 26200.9711，x64；界面 zh-CN（微软拼音）；显示缩放 200%（AppliedDPI 192）；WebView2 155.0.4283.62；电池 95–96%，接电源。UAC 关闭（`EnableLUA=0`，本轮未改动），进程为 High 完整性。
- **存储**：`%TEMP%\CMWFA-20261023b\` 下的独立存储 `store`（主）、`store16`（索引损坏）、`store48`（保护名单损坏）、`perfstore`（性能）；独立 WebView2 配置目录 `webview*`。默认存储与真实 WebView2 配置的 Local Storage 在运行前后逐项一致。
- **扫描根的设置方式**：桌面没有路径输入框，推荐根来自 `USERPROFILE`。桌面启动时把进程内 `USERPROFILE` 指向测试目录 `fx\home`（含 `Downloads`、`Documents\big120`、`Desktop`），推荐根因此都是测试目录；WFA-07 另用 `fx\home2`（其 `Downloads` 是指向夹具的 junction）。每次点击“开始只读扫描”前，脚本核对选中的单选项是测试目录且只有它被选中。原生文件夹对话框没有使用（见 C 部分）。其余扫描由 CLI 写入同一存储后在桌面打开。
- **桌面驱动方式（偏离指令）**：按 [验证计划 2.2](../verification-plan.md) 用 `CM_WEBVIEW_DEBUG_PORT=9222`（另试 19333）启动调试构建：stderr 打印了“WebView remote debugging on 127.0.0.1:9222”，WebView2 浏览器进程命令行里也有 `--remote-debugging-port=<端口>`，但没有任何进程监听该端口，数据目录中也没有 `DevToolsActivePort`（见 R3-06）。同机 headless Edge 155 的 CDP 端口与本地 HTTP 监听都正常。桌面因此改用 **UI Automation**（读 WebView2 的无障碍树与文本、调用控件模式）加系统级输入（`SendKeys`、鼠标事件），截图用 `PrintWindow`，与上一轮相同。性能项本来就要用发行构建（没有调试端口），同样用 UIA 计时，计时含约 50–150 ms 的 UIA 轮询开销。
- **运行窗口**：2026-10-23 02:20–03:35 UTC。
- **授权**：沿用负责人已给出的授权（CMWFA 测试对象上的真实回收站、HKCU 测试卸载项、一次性测试应用、winget 只读检查、本机模拟 AI 端点；结束后清理）。终端审阅码由执行者在真实控制台（conhost + `WriteConsoleInputW`）中输入，保存了会话记录；回收站内永久删除先输入 `DELETE`。
- **一次意外的只读访问**：WFA-68 中第一次 `insights downloads` 用 `CM_INSTALLER_LOCATIONS` 重定向，CLI 不认这个变量，读取了真实的“下载”文件夹的元数据；返回 `item_count: 0`，没有记录任何文件名，没有写入。之后改用 `USERPROFILE` 重定向。

## 统计

### A 部分：上一轮失败、未执行项与 CLI 低严重度问题

| 范围 | 通过 | 失败 | 受阻 |
| --- | --- | --- | --- |
| 纯桌面失败 3 条（WFA-65、98、116） | 2：WFA-65、116 | 0 | 1：WFA-98 |
| 桌面部分失败 10 条（WFA-02、16、35、40、44、48、63、76、91、104）及 NEW-09（WFA-50 桌面） | 9：WFA-02、16、35、44、48、50、76、91、104 | 2：WFA-40（低）、63（低） | 0 |
| 上一轮未执行的桌面部分 5 条（WFA-07、49、68、79、90） | 4：WFA-07、68、79、90 | 1：WFA-49（低，待确认需求） | 0 |
| CLI 低严重度 3 条（WFA-40、58、113） | 3 | 0 | 0 |
| 大扫描概览性能（NEW-01，发行构建） | 1 | 0 | 0 |
| **合计 23** | **19** | **3** | **1** |

### B 部分：本轮改动涉及的桌面旅程回归

8 个旅程全部通过：执行后的结果页、门禁关闭时的已存计划、索引损坏与重建按钮、在磁盘页保护、回收站页清单与永久删除计划（只能在终端批准）、界面语言双向切换、浅色/深色、只用键盘的写旅程（有观察 R3-05）。

### C 部分：需要负责人在场

未执行 6 项（WFA-99 物理长按 Esc、WFA-100、101、116/07 的原生对话框、118、120），步骤见下文；WFA-88 仍受阻（UAC 关闭）。

“通过”只表示本轮实际执行的检查点满足预期。

## 上一轮问题的复测结论

| 编号 | 用例 | 本轮观察 | 结论 |
| --- | --- | --- | --- |
| NEW-01（高） | WFA-116、14 | 发行构建：启动后概览自动打开最近的 12 万条目扫描，窗口出现后 0.40 秒显示“扫描完成”；从“其他扫描”打开 20,001 / 40,401 / 120,601 条目的扫描，各 3 次分别为 0.23–0.23 / 0.24–0.24 / 0.22–0.24 秒；打开磁盘页 0.13–0.22 秒；进程累计 CPU 约 1 秒。桌面发起 120,602 条目扫描（约 3.9 秒完成）期间连续切换“计划/活动/恢复/设置”24 次，每次 0.07–0.38 秒渲染，没有“正在读取…”卡住；完成后概览正常显示。CLI `scan show` 0.02 秒 | 已修复 |
| NEW-02 | WFA-27、65、76 | 桌面执行隔离、手选回收站、卸载、残留计划后，结果页都没有“找不到这条记录”；从“活动”重新打开也没有 | 已修复 |
| NEW-03 | WFA-02 | 门禁关闭后打开已存的手选回收站计划：页面写明“默认关闭……现在不能批准，也不能执行”，只有“打开实验功能设置”和“在终端中使用同一份计划”，没有摘要输入框和批准按钮；重新开启后两者恢复 | 已修复 |
| NEW-04 | WFA-16 | 索引损坏时概览与磁盘页都显示“扫描索引已损坏，需要重建”并提供“重建扫描索引…”；确认后损坏文件改名为 `index.db.damaged-<UTC>`、`migrations.log` 记 `rebuilt`，隔离区 3 项与操作历史保留，旧扫描为 not found，新扫描正常 | 已修复 |
| NEW-05 | WFA-35 | `W:`（回收站关闭）上的 `x.bin`：检查对话框写“所在的卷没有可用的回收站。不会改为永久删除。”、“0 / 1 项可以移到回收站”，按钮为“没有可以处理的对象” | 已修复 |
| NEW-06 | WFA-44 | 磁盘页详情有“保护此文件/保护此文件夹”；保护后 CLI `protect list` 出现该条（`layer: user`、`exact`），`trash --entry` 为 `policy_denied`、退出 3；该文件在桌面无法再被选入回收站对话框 | 已修复 |
| NEW-07 | WFA-48 | 名单写坏后“设置 › 保护清单”写明原因、文件路径、“所有写操作都已停止”，给出修复或移开文件的步骤与“重新读取”；“保护一个文件夹…”变为不可用；执行前对话框阻止执行；移开文件后“重新读取”恢复写操作，已批准的 P7 由 CLI 执行成功 | 已修复（见观察 3） |
| NEW-08 | WFA-47、63 | 检查对话框写“这是指向受保护系统位置 `\\?\C:\Windows\System32` 的链接，不会被跟随，也不会被移动”，0/1 可处理；`fake-home`、`data` 分别写明受保护的子项（凭据、数据库）。但 `link-sys` 仍可“选择”，选择栏照样出现“移到回收站…”，到检查对话框才被拒 | 部分修复，WFA-63 仍失败（R3-03） |
| NEW-09 | WFA-50 | 构建产物发现后给 `rustold\target\debug\x.o` 加长 1,000 字节：只选它时提示“所选对象在扫描后都已变化，请重新扫描”，不生成计划；同时选 `rustold` 与 `rustold2` 时计划只含 `rustold2`（1.0 MiB）。手选回收站文件夹在批准后内部改动，执行跳过，结果页写“文件夹的内容与计划时不一致，没有操作。” | 已修复（审阅页没有说明被排除的项，见 R3-07） |
| NEW-10 | WFA-58 | 挂载中的 `w.vhdx`：`skip_reason: mounted`，`installer plan create` 为 `policy_denied`、退出 3，说明含 `mounted_image: eject it first (File Explorer > Eject, or Dismount-DiskImage)`，`--mode delete` 同样拒绝；不再出现 `plan_stale` | 已修复 |
| NEW-11 | WFA-113 | Code Cache 内容隔离后该目录只剩空的 `js\`：重新分析不再列为候选；只有空子目录的 Code Cache 没有候选；`clean --mode quarantine --non-interactive` 与 `plan create --all-default-selected` 都是 `policy_denied`（“empty selection … a folder that holds no files is left out”），没有生成计划 | 已修复 |
| NEW-12 | WFA-40 | CLI：未核对的中断操作在 `operations list` 中 `authorization_source: interactive_review`、`counts` 完整、恢复 `unknown` 并提示 reconcile；结束进程为 `unavailable`（“An ended process cannot be brought back”）；卸载为 `unavailable` 并指向 `apps uninstall leftovers`；带注册表备份的残留为 `in_app_restore` 并给出 `apps uninstall restore --operation`。桌面：见 R3-01、R3-02 | CLI 已修复；桌面部分仍失败 |
| NEW-13 | WFA-111、91、66、93 | 中文界面的系统状态（“没有读数超过诊断阈值。”）、空间洞察、进程页、回收站原因、链接拒绝、卸载拒绝原因都已是中文，没有“未识别的问题（代码 …）”。卸载计划审阅与结果页的动作说明仍带英文“(vendor uninstaller, its own UI)” | 基本修复，残留见 R3-04 |
| NEW-14 | WFA-76 | Mismatch 不在计划中，原因写“卸载程序的签名者与登记的发布者不一致，所以不会运行。可以在系统‘设置 › 应用’中卸载。” | 已修复 |
| NEW-15 | WFA-65 | “清理 › 回收站”列出刚移入的 `e1.bin`、`e2.bin`（原位置、大小、删除时间），可勾选生成永久删除计划 | 已修复 |
| NEW-16 | WFA-98 | 见 WFA-98：本机无法观察 | 无法判定（受阻） |
| NEW-17 | WFA-104 | 预览写“查看实际发送的 4.4 KB（4,536 字节）”，展开即单行请求体；模拟端点收到的 POST 正文 4,536 字节，与页面显示的正文逐字节相同，不含用户名与 `C:\Users` 路径 | 已修复 |

## 新发现的问题

均为低严重度；没有新的高或中严重度问题。

| 编号 | 用例 | 复现与实际 | 预期 | 疑似组件 |
| --- | --- | --- | --- | --- |
| R3-01 | WFA-40 | 用 CLI 执行 1,100 项隔离计划，约 1.2 秒后强杀；在桌面“活动”打开这条“需要核对”的操作：结果页“授权来源”为“—”，“已处理”也为“—”。CLI `operations list` 同一操作为 `interactive_review` 与完整计数，`operations show` 的 `authorization_source` 为 null（已完成的操作也是 null，桌面对已完成操作显示“人工审阅”）。核对后桌面显示“人工审阅” | 未核对的操作也显示记录中的授权来源，桌面与 CLI 一致 | [operation.ts](../../apps/desktop/src/views/operation.ts) 与宿主的操作报告查询；`operations show` 的报告字段 |
| R3-02 | WFA-40、76 | 执行“CMWFA-BazApp”的残留计划（2 个文件移到回收站、1 个注册表键备份后删除）后，“活动”列表该行写“可在隔离区恢复”；实际文件在回收站、注册表从备份还原（结果页与 CLI 的说明是对的） | 列表的恢复标签区分隔离区、回收站与备份还原 | [words.ts](../../apps/desktop/src/lib/words.ts) `AVAILABILITY_LABEL`（`in_app_restore` 一律译为“可在隔离区恢复”）与 [activity.ts](../../apps/desktop/src/views/activity.ts) |
| R3-03 | WFA-63（NEW-08 余项） | 磁盘页选中 `link-sys`（指向 System32 的 junction），详情仍有“选择”，选择后选择栏出现“移到回收站…”，到检查对话框才拒绝；`fake-home`、`data` 同样。用户保护的 `p1.bin` 已不能被选入。另外对话框中目标带 `\\?\` 前缀，大小写成“0 B”（磁盘页为“大小未知”） | 被硬拒绝或含受保护子项的对象不提供进入回收站流程的入口，与用户保护项一致 | [disk.ts](../../apps/desktop/src/views/disk.ts) 的选择入口；[suite.rs](../../apps/desktop/src-tauri/src/suite.rs) `trash_inspect` 的显示路径与大小 |
| R3-04 | WFA-111、76（NEW-13 余项） | 中文界面中，卸载计划审阅的“完整清单”与执行结果页的动作文字为 `"…\uninst.exe" /c exit 3 (vendor uninstaller, its own UI)`；应用列表与审阅摘要处已译为“（厂商的卸载程序，使用它自己的界面）” | 所有位置都本地化 | [uninstall.rs](../../crates/cm-core/src/uninstall.rs) 生成的显示文本；[plan.ts](../../apps/desktop/src/views/plan.ts)、[operation.ts](../../apps/desktop/src/views/operation.ts) 的动作行没有经过 `adapterDisplay` |
| R3-05 | WFA-117 | 只用键盘在“恢复”中找刚隔离的项目：清单按隔离时间从旧到新排列，每次 50 项；在“显示更多”上按 Enter 后焦点回到文档根，下一次 Tab 从页首重新开始（本轮 152 项时到达目标用了 316 次 Tab 和 3 次“显示更多”） | “显示更多”后焦点落在新出现的第一项；最近的项目容易到达 | [activity.ts](../../apps/desktop/src/views/activity.ts) 隔离区分页 |
| R3-06 | 测试钩子（验证计划 2.2） | 调试构建设置 `CM_WEBVIEW_DEBUG_PORT=9222`（或 19333）并使用独立 `WEBVIEW2_USER_DATA_FOLDER`：宿主打印端口已打开，浏览器进程命令行带 `--remote-debugging-port`，但端口没有监听，也没有 `DevToolsActivePort`；同机 headless Edge 155 的 CDP 正常 | 按验证计划能连上 CDP | [main.rs](../../apps/desktop/src-tauri/src/main.rs) `webview_debug`，或 WebView2 155 对该参数的处理（未定位）；只影响测试自动化 |
| R3-07 | WFA-50（NEW-09 余项） | 同时勾选 `rustold`（扫描后已变化）与 `rustold2` 生成计划：审阅页只写“把 1 个构建产物中的文件移入同卷隔离区”，没有说明 `rustold` 因变化被排除；CLI 对应情况在 `stale_items` 中列出 | 审阅页或生成提示列出被排除的变化项 | [suite.rs](../../apps/desktop/src-tauri/src/suite.rs) 构建产物计划与桌面构建产物页 |

## 逐项结果

### A 部分

| 用例 | 结果 | 实际观察 |
| --- | --- | --- |
| WFA-65 | 通过 | 磁盘页两个文件各点“选择”，选择栏“已选 2 项 32.2 KiB 去重后逻辑大小”；检查对话框 2/2 可处理；审阅写明“移到回收站”、完整 2 项与摘要；输入前 8 位批准、执行，2 项成功，结果页提示在系统回收站中还原；“清理 › 回收站”列出两项；回收站中只有这两个 CMWFA 项目。桌面仍没有右键菜单 |
| WFA-98 | 受阻 | “保持亮屏 30 分钟”开始后页面显示剩余时间，结束后恢复。`powercfg /requests` 开始前、期间、结束后完全相同（只有 5 个匿名“旧的内核调用程序”）；另起一个测试进程用 `PowerCreateRequest` + `PowerSetRequest(Display, System)`（都返回成功）也不出现在列表中，说明本机 `powercfg` 看不到进程级请求；`CallNtPowerInformation(SystemExecutionState)` 前后都是 0x3（其他进程已持有）。息屏/睡眠超时设置未变（交直流 1800 秒 / 0）。缺失条件：可观察的电源请求列表，或负责人按 C 部分做空闲超时确认 |
| WFA-116 | 通过（原生对话框步骤见 C） | 扫描范围页显示推荐根（测试目录）与“只读取名称、大小和时间”；开始后先“扫描准备中”再“正在扫描 / 已发现 N 项”，分母未知时写“扫描进度（总量未知）”不显示百分比；扫描中切换活动、计划、设置、概览不中断；取消后约 0.1 秒显示“正在停止扫描”，约 2 秒后“扫描已取消 / 取消前已保留 80,002 个条目，结果不完整”；已扫描过的范围提供“查看结果 / 重新扫描”。观察：扫描期间“活动 › 扫描”中的条目数停在打开页面时的值（12,288），顶部任务条为实时值 |
| WFA-02 | 通过 | CLI：门禁关闭时 `plan show` 正常（`required`），终端审阅与 `plan apply` 都是退出 7；桌面见 NEW-03；重新开启后终端审阅出现摘要提示（以 Ctrl+C 退出，未授权），桌面恢复批准控件。没有授权或执行记录 |
| WFA-16 | 通过 | 见 NEW-04；CLI `scan show` 为 `index_unavailable`、退出 8，`operations list`/`recovery list` 正常；桌面重建后 `doctor` 正常 |
| WFA-35 | 通过 | 见 NEW-05；文件未改动 |
| WFA-40 | 失败（低） | CLI 部分通过（见 NEW-12）；桌面“活动”各行的状态、计数与“无法撤销 / 可在隔离区恢复 / 需要核对”与 CLI 一致，桌面核对后为“部分完成”、“人工审阅”；问题见 R3-01、R3-02 |
| WFA-44 | 通过 | 见 NEW-06 |
| WFA-48 | 通过 | 见 NEW-07；CLI `protect list` 为 `protection_unavailable`、退出 8，`required_action` 写明修复或移开该文件 |
| WFA-50（桌面，NEW-09） | 通过 | 见 NEW-09；构建产物根只用测试目录（默认根 `D:\workplace` 已在测试存储中移除）；观察见 R3-07 |
| WFA-63 | 失败（低） | 拒绝原因都正确、没有生成计划；见 R3-03。`<R>\partial` 根的覆盖缺口未在桌面检查 |
| WFA-76 | 通过 | 搜索 CMWFA 得到 3 个测试应用；三者都可勾选，计划只含 Bar 与 Baz，Mismatch 以签名者原因排除（NEW-14）；执行后 Bar 失败（退出码 3）、Baz 已卸载有残留，整体部分完成，逐个应用报告；第 2 步残留只有 Baz（缓存与日志默认选中，安装目录与注册表键需审阅，注册表键先备份），执行 4 项成功。英文残留见 R3-04 |
| WFA-91 | 通过 | 健康分 100/100，权重 30/25/20/15/10，显卡温度“不可用”并写明原因，诊断行为“没有读数超过诊断阈值。”，没有未翻译代码 |
| WFA-104 | 通过 | 在“设置 › AI 诊断”添加兼容端点 `http://127.0.0.1:18765/v1`，保存时 0 个请求，测试连接 1 个 GET 且页面写“没有发送任何文件资料”；预览与取消 0 个请求；发送 1 个 POST，正文与预览逐字节相同（NEW-17），无认证头；回答中的命令与“已批准”被移除，建议降为“需要补充信息”，没有生成计划入口。模型自己的英文句子原样出现（模型文本） |
| WFA-07 | 通过（原生对话框步骤见 C） | CLI：`nope` 为 `failed`/`root_unavailable`，`jroot` 为 `root_is_link`，文件根为 `not_a_directory`，都退出 8、大小为 null。桌面：推荐根中的 junction“下载”在开始前就灰显并写“所选路径是链接（目录联接或符号链接），请直接选择它指向的文件夹”，其他根可选；历史中三条为“失败”，打开后分别写明链接、找不到文件夹、是文件不是文件夹，没有“部分完成”或“0 B”。替代说明：链接根经推荐根列表而不是原生对话框选到，两者使用同一 `root_problem` 检查 |
| WFA-49 | 失败（低，待确认） | CLI `clean list` 正常（15 个类目，规则 ID、版本、状态、`registry_status`、`tested_at`、`tested_range`）。桌面“清理 › 规则清理”只在分析后显示候选与“有规则不适用于这台电脑，已跳过（38 处，列出规则 ID）”，没有按平台分组的目录、版本与实测登记，“设置 › 实验功能”把它写作“清理目录（命令行 clean）”。若产品意图是目录只在 CLI 提供，应改用例；否则桌面缺目录视图 |
| WFA-68 | 通过（重定向） | “下载”经 `USERPROFILE` 重定向到测试目录，放入 120 天前的 `CMWFA-old-download.bin`（1 MiB）、200 天前的 4 KiB 文件和 3 天前的文件：CLI `insights downloads --older-than-days 90` 只列两个旧文件，`selected: false`；桌面旧下载卡片“2 个文件，共 1.0 MiB”，按大小排列，复选框都未勾选，说明只会先生成“移到回收站”计划。破坏性部分未执行。覆盖层次：重定向，不是真实 Downloads |
| WFA-79 | 通过 | 先做一次 winget 只读检查（339 行，调试轨迹 1 次 winget）；`--checks off` 后 `apps updates --debug` 33 ms 返回 `checked: false`、保留原检查时间、调试轨迹没有通道调用；`apps updates show` 同；桌面更新页写“更新检查已关闭：不会运行任何更新通道……保留原来的检查时间”，计数与上次一致；最后恢复为开启。说明：进程启动跟踪漏掉了短命进程，没有作为判据 |
| WFA-90 | 通过 | CPU 负载进程运行时，桌面“优化 › 性能诊断”列出“进程在采样期间繁忙 powershell.exe”，最忙进程表含其 PID 与约 100% 占用，入口指向进程页；CLI `optimize diagnose --window 5` 为 `busy_process`（名称、PID、路径、5 秒）、`read_only: true`；负载进程诊断后仍在运行（由执行者结束） |
| WFA-40（CLI） | 通过 | 见 NEW-12 |
| WFA-58 | 通过 | 见 NEW-10；镜像未被改动 |
| WFA-113 | 通过 | 未知选项、重复选项、`-y`、重复 `--format`、`help --json`、缺值都是 `invalid_argument`、退出 2；非 TTY 默认 JSON；本轮 152 条 v1 记录全部通过 Schema，错误码与退出码 0 处不符；NEW-11 已修复。观察：`scan show <id> --bogus`（末尾无值）的消息为“--bogus needs a value”而不是“unknown option”，代码与退出码正确 |
| 性能（NEW-01） | 通过 | 见上表 NEW-01；目标“概览 < 2 秒、扫描发布期间其他页面可用”满足 |

### B 部分

| 旅程 | 结果 | 实际观察 |
| --- | --- | --- |
| 执行后的结果页 | 通过 | 见 NEW-02；隔离结果页分列已处理 3.0 MiB、仍在隔离区 3.0 MiB、卷可用空间变化，CLI `operations show` 为 `processed`/`quarantined` 3,145,728、`estimated_freed_bytes` "0" |
| 门禁关闭时的已存计划 | 通过 | 见 NEW-03 |
| 索引损坏与重建按钮 | 通过 | 见 NEW-04（独立存储 `store16`） |
| 在磁盘页保护 | 通过 | 见 NEW-06 |
| 回收站页清单与永久删除计划 | 通过 | 回收站页勾选两项生成“永久删除回收站项目计划”（4 个动作，含 `$I`/`$R` 文件），计划页写“无法撤销”，只有“复制：在终端中审阅并批准 / 执行”，没有批准控件；终端先输入 `DELETE` 再输入摘要后批准，`plan apply` 4 项 `purged`，回收站回到 0 项，历史为 `unavailable` |
| 界面语言双向切换 | 通过 | “设置 › 界面语言”选 English 后侧栏与 16 个页面立即为英文（剩下的中文只是应用名、网卡名等数据），重启后仍为英文；选简体中文后立即恢复，重启后保留；最后恢复“跟随系统” |
| 浅色/深色 | 通过 | 主题按钮依次为深色（背景取样 rgb(35,39,37)）、浅色、跟随系统；深色在重启后保留；最后恢复跟随系统。观察：深色下窗口标题栏仍为浅色（系统标题栏） |
| 只用键盘的写旅程 | 通过（观察 R3-05） | Ctrl+6 → Tab 到计划行 → Enter → Tab 到摘要框输入前 8 位 → Tab → Enter 批准 → 焦点自动到“执行：移入同卷隔离区（3 项）”→ Enter；Ctrl+8 → Tab（经 3 次“显示更多”）到两个隔离项各按空格 → Tab 到“创建恢复计划（2 项）”→ Enter → 输入摘要 → 批准 → 执行，两个文件回到原处、SHA-256 不变。中文输入法下第一次输入摘要被组合成中文，按 Shift 切换后重输（结束时已切回） |

## 观察（不改判）

1. 桌面运行期间 CLI 新建的扫描仍不会出现在概览“其他扫描”中，重启后才显示（计划和操作会出现）。
2. WebView2 中“取消扫描”等按钮的 UIA InvokePattern 有时报“不支持的模式”，改用鼠标点击正常；属于驱动层现象。
3. 保护名单损坏时，执行前对话框写“下面的问题解决前不能开始”，同时显示“以当前权限继续”按钮；该按钮是否可用本轮未核对。CLI 同时拒绝执行。
4. 设置页“授权身份”显示 `user:<host>\<user>`；截图与原始记录因此不提交。
5. 本机 `powercfg /requests` 看不到任何进程发起的电源请求（含独立探针），以后的 WFA-98 需要别的观察方法。

## 需要负责人在场的用例（主会话照做）

**准备（一次）**：被测二进制保留在 `<仓库>\target\acc3\release\cleanmanager.exe`、`<仓库>\target\acc3-desk\debug\cleanmanager-desktop.exe`（调试）与 `<仓库>\target\acc3-desk-rel\release\cleanmanager-desktop.exe`（发行），规则目录用 `<仓库>\rules`（`main` 仍为 `9393a6c`；若已有新提交，先重新构建并记录 SHA-256）。在一个 PowerShell 中：

```text
$S = "$env:TEMP\CMWFA-owner"; New-Item -ItemType Directory -Force $S\store, $S\fx | Out-Null
$env:CM_STORE_DIR = "$S\store"; $env:CM_RULES_DIR = '<仓库>\rules'; $env:WEBVIEW2_USER_DATA_FOLDER = "$S\webview"
& '<仓库>\target\acc3\release\cleanmanager.exe' experimental --state on
& '<仓库>\target\acc3-desk-rel\release\cleanmanager-desktop.exe'      # 从同一个窗口启动，继承上面的变量
```

记录方式：每一步记下时间（到秒）、看到的文字；截图用 Win+Shift+S 只截应用窗口，存到 `.research/acceptance-raw/<run>/owner/`（不提交）。结束时 `cleanmanager experimental --state off`，关闭应用，删除 `$S`。

1. **WFA-99 清洁屏幕（物理键盘）**：桌面“状态 › 常驻工具 › 清洁屏幕”，自动退出选“30 秒”，点“开始清洁屏幕”。观察：所有显示器变黑、指针隐藏、底部有提示与倒计时；按几个字母键、点鼠标，都不应传到其他程序。短按 Esc（约 0.5 秒）不应退出；按住 Esc 约 2.5 秒应退出，键盘鼠标立即恢复。再开始一次不碰任何键，30 秒后应自动退出。随后在 CLI 中查看 `operations list` 不应有新增操作。判据：长按退出与超时退出都发生，输入不卡住；任何一步无法退出即失败（Ctrl+Alt+Del 始终可用作逃生）。
2. **WFA-100 摄像头与麦克风**：先运行 `Get-ChildItem $env:CM_STORE_DIR -Recurse | Select FullName,Length,LastWriteTime > before.txt`。桌面打开“状态 › 常驻工具”，看“摄像头与麦克风”一栏为“现在没有应用在使用……”。负责人打开系统“相机”应用，计时：3 秒内该栏应出现“相机”等应用名；关闭相机后 3 秒内消失。再用“录音机”开始/停止录音重复一次。最后同样导出 `after.txt` 并比较，存储中不应新增使用记录类文件。若系统隐私设置禁止读取，应显示“不可用”及原因（记录原文）。不修改任何隐私设置。
3. **WFA-101 电量与低电量提醒**：“状态 › 常驻工具”先打开“在通知区域显示 CleanManager”（托盘），确认“电量 · 本机电池 · 正在充电或使用电源 · NN%”与“没有已连接的蓝牙设备”。阈值只能设 5–50%，而提醒要求“阈值不低于当前电量”：本轮电量 95–96%，需先用电池把电量放到 50% 以下（或在电量本来较低时做），再把阈值设为不低于当前电量的值并开启低电量提醒；电量无法降到 50% 以下时记为受阻。负责人拔掉电源：应只提醒一次（托盘提示、页面文字），记录时间与文字；等待 2 分钟确认不重复；接回电源后不再提醒。再在阈值框分别输入 4 与 51，均应被拒绝或纠正为 5–50 之间。结束后关闭低电量提醒与托盘，确认 `HKCU\...\Run\CleanManager.Resident` 不存在。
4. **WFA-116 / WFA-07 原生文件夹对话框**：先建测试目录 `$S\fx\pick\a.txt` 与 junction：`cmd /c mklink /J "$S\fx\jroot" "$S\fx\pick"`。桌面“分析 › 扫描范围 › 选择其他文件夹…”，在原生对话框中**用鼠标**导航到 `$S\fx\pick`，点“选择文件夹”：列表中应出现该目录并被选中，点“开始只读扫描”，扫描完成后概览显示范围、时间与完整性。再次“选择其他文件夹…”选 `$S\fx\jroot`：开始前就应以链接原因拒绝并可重新选择，不得开始扫描。注意：对话框默认位置可能是真实用户文件夹，**只确认测试目录，绝不在对话框中确认其他文件夹**；若误选了真实文件夹，不要点“开始只读扫描”。结束时 `cmd /c rmdir "$S\fx\jroot"`（不带 /s）。
5. **WFA-118 显示缩放 100/125/150%**：本机当前 200%。负责人在“设置 › 系统 › 屏幕 › 缩放”依次选 100%、125%、150%（每次之后完全退出并重启 CleanManager）。每档检查：概览、磁盘（表格与 treemap，点一个色块、下钻一层）、计划审阅（打开任一测试计划）、执行结果页、设置页；再把窗口拖到约 800 px 宽重复看一遍。判据：主要按钮可见可点、文字不截断、页面无横向滚动条（宽表格可在表内横向滚动且表头清楚），treemap 重绘后色块与表格一致。每档截一张概览和一张磁盘页。最后恢复为 200% 并确认。
6. **WFA-120 读屏（Narrator）**：Ctrl+Win+Enter 开启讲述人。① 磁盘页 Tab 到条目表格，用上下方向键读一页：每行应读出名称、大小，已选行读出“已选择”，文件夹读出可展开/按 Enter 打开；② 打开一份测试计划的审阅页，逐段读：模式（例如“移入同卷隔离区”）、对象数、摘要提示与错误提示；③ 在扫描范围页开始一次测试目录扫描：进度应合并播报（不是每个数字都读），完成时只播报一次“扫描完成”。记录每处实际读出的原文（讲述人设置中可打开“讲述人历史记录”/字幕）。结束 Ctrl+Win+Enter 关闭。
7. **WFA-98 保持亮屏（补充确认）**：电源计划“关闭屏幕”时间临时不改；在“常驻工具”选“30 分钟”开始保持亮屏后，负责人不碰键鼠，等待超过当前息屏时间（本机交直流均为 30 分钟），屏幕应保持点亮；之后点“结束保持亮屏”，再等息屏时间，屏幕应按计划熄灭。若不愿等待 30 分钟，可记为受阻。
8. **WFA-88 一次 UAC**：仍受阻。本机 `EnableLUA=0`，所有进程都是完整管理员，不会出现 UAC；是否临时启用属于安全设置变更，由负责人决定，执行步骤沿用 [上一轮报告](windows-full-2026-10-23-retest-report.md) 的“需要负责人在场的用例”第 1 项。

## 清理与核对

- 进程：桌面（调试与发行）、模拟 AI 端点、CPU 负载进程、测试 ping 均已结束；结束时没有 CleanManager 进程。
- 回收站：本轮移入的 CMWFA 项目（`e1.bin`、`e2.bin` 及 Baz 残留 3 项）都经产品的永久删除计划（终端 `DELETE` + 摘要）删除；结束时回收站 0 项，与运行前一致，过程中没有非测试项目。
- 注册表：3 个 `CMWFA-*` 卸载登记已删除（Baz 由产品残留计划删除，Bar、Mismatch 由清理脚本删除）；VHD 卷的 BitBucket 设置键已删除。Run、`StartupApproved`、Uninstall、`HKCU\Software` 的 CMWFA 计数与总数、BitBucket 卷列表都与运行前基线一致；托盘从未开启，`CleanManager.Resident` 不存在，`NotifyIconSettings` 没有新增子键。
- 用户文件夹：`%LOCALAPPDATA%\CMWFA-BazApp` 已删除；启动文件夹、开始菜单、真实 Downloads 的 CMWFA 计数与运行前相同。
- VHD：已分离并删除，`W:` 不存在。
- ACL 与链接：拒绝 ACE 已用 `icacls /remove:d` 去除；4 个 junction 逐个 `rmdir`（不带 `/s`）删除，前后 System32 顶层条目数都是 4,900，sentinel 不变。
- 存储与夹具：审计库副本留在本地证据目录后，删除了整个 `%TEMP%\CMWFA-20261023b\`（夹具、4 个存储与独立 WebView2 配置）；各卷根没有 `.cleanmanager-quarantine`。
- 未改动：默认存储的文件清单、真实 WebView2 配置的 Local Storage、`EnableLUA`、电源计划超时、更新检查设置（关闭后已恢复为开启）、中文输入法转换模式（键盘旅程中切换过一次，已切回）。
- 构建：worktree 已删除；`target\acc3`、`target\acc3-desk`、`target\acc3-desk-rel` 构建缓存保留在被 Git 忽略的 `target\` 下，供 C 部分使用。本轮没有提交或推送。
