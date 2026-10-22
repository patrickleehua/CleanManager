# Windows 本机验收报告（2026-10-22）

> 后续进展：BUG-WIN-01 已由 subagent 修复，并通过独立 CLI/原生窗口复验，见[修复复验报告](windows-2026-10-22-retest-report.md)。以下保留初次验收的原始结论和失败证据，不将历史失败改写为通过。
>
> 发布整理补记（2026-10-22）：BUG-WIN-01 的修复正由另一个 agent 提交入库；在该提交落地前，复验所测版本仍是 `2d654d9` 加未提交补丁。运行结束后的代码审查另发现两项问题，本轮没有据此改判：一是以 junction（目录链接）作为扫描根时会误报完成（false complete），本轮未覆盖该输入；二是在单个大目录内取消扫描不能及时生效，即下文“执行障碍与限制”中 30,000 文件宽目录取消后仍为 completed 的情况。两项的修复后预期已写入可重复验收工具 [tools/win_verify.py](../../tools/win_verify.py)。
>
> 为公开发布，原始证据（含用户名、已安装软件与实时进程清单、截图）只保留在本地 `.research/acceptance-raw/2026-10-22/`（不入库）；文中路径按 doctor 脱敏规则改写（用户主目录 → `~`，用户名 → `<user>`，主机名 → `<host>`），证据链接改为 [docs/evidence/runs/](../evidence/runs/) 中逐项脱敏的运行记录。结论、数字与失败描述未改动。

**25 条文字用例：21 通过，1 失败，1 受阻，2 未执行。当前版本不应判为整体验收通过。**

[用例与独立预期](windows-2026-10-22-cases.md) · [逐项运行记录](../evidence/runs/)（`run_win_acc_2026-10-22_win-*.json`，已脱敏）· [轮次完整性与判据汇总](../evidence/runs/run_win_acc_2026-10-22_win-22.json) · 原始证据目录仅本地保留（`.research/acceptance-raw/2026-10-22/windows-2026-10-22-evidence/`）

## 版本、环境与执行范围

- 版本：`main@2d654d9199d4959a7492f4e519158a8255dc0f29`，Windows 11 x64，OS build `10.0.26200`，Tauri 原生 Windows 窗口与真实 CLI。时间以新加坡时间（UTC+8）表述；原始输出保留 UTC。
- 桌面 SHA-256：`c7024489970867fdfc43f3ad59a1284ac3f44d2b72386dae3be11b0a75cfa08c`；CLI SHA-256：`0f104e35a0e96527a7121c56d68322c33d509f54cecaa17aeffc1340153bdb15`。本轮重新构建了桌面及 CLI；没有改动业务代码。新增文件为用例、报告和证据。
- 输入为本次生成的专用临时目录：6 个文件、3 个子目录，含中文空格名、空文件、空目录与假 `.ssh` 内容；逻辑大小 **27,904 字节**。独立记录文件名、大小与 SHA-256；范围外另有 sentinel。
- 使用独立 `CM_STORE_DIR`；测试期间只在该存储开启实验功能。结束后已关闭实验功能、恢复“跟随系统”主题，保留应用窗口、夹具和待批准计划，方便复查。没有批准任何写操作，最终执行历史仍为空。
- 开始阶段的工具沙箱限制了部分硬件计数器、注册表和回收站探测。相关项通过真实主机 CLI 重跑；随后关闭原桌面进程，使用同一二进制和独立存储启动新进程，再实测了原生资源与进程页面。真实主机窗口显示 CPU 约 13%、16 个逻辑处理器与 373 个进程。维护页面报告管理员上下文；未独立采集 token elevation，因此**不能由本轮推导普通用户/UAC 的完整验收**。
- 不使用网页 mock、单元测试或历史 CI 结果替代本轮桌面操作。57 个本轮机器 stdout 记录均通过 v1 JSON Schema 校验；这只证明结构符合协议，不能消除下面的行为缺陷。

环境与二进制信息：每条运行记录的 `os`、`binary_sha256` 与 `config`；输入清单：[WIN-04 运行记录](../evidence/runs/run_win_acc_2026-10-22_win-04.json)的 `measurements.fixture_manifest`；结构检查：[WIN-22 运行记录](../evidence/runs/run_win_acc_2026-10-22_win-22.json)的 `measurements.protocol_schema_validation`。

## 失败：普通文件被当成扫描目录

**WIN-09 / BUG-WIN-01，P2，未修复。** `scan --root` 收到一个已存在的普通文件后，没有明确拒绝该输入，而是返回 `partial`，并把文件写成目录条目。

复现前置：本轮 `input\a.txt` 是普通文件，内容长度 **1,024 字节**。

```powershell
$env:CM_STORE_DIR = 'D:\workplace\cleanmanager\.research\windows-acceptance-2026-10-22\store'
$fixture = '~\AppData\Local\Temp\CleanManagerAcceptance-20261022-b4ef3aa9\input'
& .\target\debug\cleanmanager.exe scan --root "$fixture\a.txt" --json --non-interactive
# 用返回的 scan_id 查询：
& .\target\debug\cleanmanager.exe entries list --scan <scan_id> --json --non-interactive
```

预期：扫描目录入口拒绝普通文件，并给出可理解的输入/范围错误；不能将普通文件记为目录。依据为 SCN-01 的指定目录扫描、SCN-03 的范围与终态语义，以及查询记录必须描述实际对象的契约。

实际：

- `scan_result.status="partial"`，退出 **5**；`entries_seen=1`、`logical_bytes="0"`、`unknown_size_count=1`。
- issue 为 `io_error`，消息为“参数错误。(os error 87)”。
- `entries list` 返回该 `a.txt`，但 `kind="directory"`、`logical_bytes=null`。
- 重启桌面后，概览把这次记录呈现为“部分完成 / 已知空间 0 B / 1 个缺口”。文件本身仍完整，没有被移动或删除。

该问题在沙箱与真实 Windows 主机环境各复现一次。影响为错误的对象类型、扫描终态和用户/Agent 解释；没有观察到越界写入。

证据：[WIN-09 运行记录](../evidence/runs/run_win_acc_2026-10-22_win-09.json)（扫描输出与错误的目录条目；桌面截图仅本地保留）。定位线索：[Windows 目录扫描实现](../../crates/cm-platform/src/windows/directory.rs) 中根对象记录将 `attributes` 固定为 `ATTR_DIRECTORY`，进入枚举前没有在该路径验证根确为目录。本轮只记录缺陷，没有修复或降低预期。

## 逐项结果

下表的“通过”仅覆盖对应文字用例列出的本轮检查点。证据文件均来自实际输入、操作与观察。

| 用例 | 结果 | 实际观察与证据 |
| --- | --- | --- |
| WIN-01 | 通过 | 原生窗口响应，五个主工具和六个扩展入口可到达；Agent、AI 诊断、设置有明确页面。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-01.json) |
| WIN-02 | 通过 | 状态页显示默认关闭和 G3 原因；CLI `status` 退出 7 / `unsupported`。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-02.json) |
| WIN-03 | 通过 | 深色主题实际显示，重载保留；随后恢复跟随系统，真实进程重启后仍保留。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-03.json) |
| WIN-04 | 通过 | 原生文件夹选择器选择专用夹具，桌面扫描完成，10 个条目、27,904 字节；与独立清单及 CLI 一致。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-04.json) |
| WIN-05 | 通过 | 表格按逻辑大小降序；切换名称排序后标签和行顺序变化；进入 nested 后显示 second.log 8,192 字节与 zero.txt 0 字节。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-05.json) |
| WIN-06 | 通过 | 中文过滤命中 1 项；不存在的词明确为 0 项；清空后恢复 6 个当前层条目。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-06.json) |
| WIN-07 | 通过 | 详情给出真实大小，“在资源管理器中显示”；选择栏为“移到回收站…”，说明先检查再计划，未点击最终写入。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-07.json) |
| WIN-08 | 通过 | 为 big.bin 生成 `manual_selection`，查询仍需批准；非交互 apply 返回 `needs_approval` / 3；最终执行历史为空。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-08.json) |
| WIN-09 | **失败** | 不存在目录有明确 failed；普通文件被错误索引为目录且终态为 partial。见 BUG-WIN-01。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-09.json) |
| WIN-10 | 通过 | 2 项一页，共 5 页覆盖全部 10 项，无重无漏；字节为十进制字符串或 null；limit=0/501 均 invalid_argument / 2。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-10.json) |
| WIN-11 | 通过 | 假 `.ssh/id_ed25519` 被识别为 credentials，操作仅查看/复制路径；真实主机建计划返回 policy_denied / 3。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-11.json) |
| WIN-12 | 通过 | 固定 1 个 fs_trash 对象，前后 body/摘要一致，有效期恰为 900 秒，不可匹配策略；桌面显示相同摘要前 8 位 a222d35d。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-12.json) |
| WIN-13 | 通过 | 测试进程以 share_mode=0 只读独占文件；inspect 报告 python.exe PID 26612，与实际测试 PID 一致，随后关闭测试句柄。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-13.json) |
| WIN-14 | 通过 | 软件只读清单列 161 项，明确部分注册表来源受限；筛选 Chrome 后显示发布者、安装登记身份与“不在本次扫描内”。没有发起卸载/更新。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-14.json) |
| WIN-15 | 通过 | 5 项维护任务，无预勾选，适用/不适用与权限说明存在；草稿按钮禁用，没有运行记录，进入页面只读探测。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-15.json) |
| WIN-16 | 通过 | 原生真实主机窗口采到 CPU 13%、16 逻辑处理器、内存与网络；进程页列 373 项。不可用温度等有原因/空值，不伪装成 0；System PID 4 的结束计划被拒绝。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-16.json) |
| WIN-17 | **受阻** | 已验证系统对象 CLI 返回 5 项，WinSxS/受限快照大小保持未知并有说明；完整洞察页还会枚举真实 Downloads，Windows 使用系统 KnownFolder，当前没有独立测试账号/可重定向入口，因此本轮没有执行完整页面和旧下载测量。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-17.json) |
| WIN-18 | 通过 | CLI 计划可在桌面审阅；扫描历史区分完成、失败、取消；执行历史为空；恢复页明确隔离区为空、系统回收站不导入。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-18.json) |
| WIN-19 | 通过 | 对 400 目录/40,000 文件扫描发取消，回执后终态为 cancelled / 130，保留 6,401 个确认条目，可继续分页查询。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-19.json) |
| WIN-20 | 通过 | 中文 help 在 text 模式可读，doctor 明确 `redacted=true`，SQLite 与配置检查有结果；没有使用导出、发送或上传入口。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-20.json) |
| WIN-21 | 通过 | CLI 开启后桌面可使用受门禁工具；真实关闭/重启进程后开关仍生效；能力未被提升为 available；最后关闭并在桌面确认默认说明页。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-21.json) |
| WIN-22 | 通过 | 所有测试完成后输入名称/目录/大小/SHA-256 和 sentinel 均相同；执行历史为空。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-22.json) |
| WIN-23 | 未执行 | 没有进行最终批准、回收站/隔离移动、永久清空或恢复。本轮通过的是预览、共享计划与拒绝路径，不能推导实际执行/恢复通过。 |
| WIN-24 | 未执行 | 未准备可还原 VM、专用测试安装器/启动项、签名包及相应设备；未卸载真实应用、修改启动项、执行系统维护或安装升级。 |
| WIN-25 | 通过 | 原生清理页对本轮扫描执行规则分析，返回 0 候选，明确仅限已启用规则在此范围没有匹配，且没有预选项；没有虚构可清理空间。 [运行记录](../evidence/runs/run_win_acc_2026-10-22_win-25.json) |

## 执行障碍与限制

- 初次准备使用了每次调用都会更换的沙箱临时目录；该目录无法跨 CLI/原生窗口共享。改为固定的专用临时夹具后重新执行，未把预检失败算作产品缺陷。一次性执行脚本的控制台编码与 help 输出模式问题也仅修正了执行方式，未修改产品。
- 沙箱内回收站探测返回拒绝访问，记录在 `cli-19-protected-plan.json` / `cli-20-manual-plan.json`；真实主机对应 `*-host.json` 正常完成保护拒绝与待批准计划。因此这两条沙箱输出属于环境限制，不能当成回收站实现故障。
- 第一轮 30,000 文件的宽目录取消得到 running/accepted 回执，但最终 completed，不能仅凭回执认定取消成功。正式取消用例使用多目录夹具，实测并核对 cancelled 终态。该宽目录时间边界保留在 cli-27..30 记录中，没有用成功结果覆盖它。
- 原生控件的过期索引、窗口外坐标和对话框输入焦点问题通过重新观察、滚动及原生路径字段处理；它们不是产品缺陷，也没有用模拟结果替代界面操作。
- WIN-23 的最终桌面删除动作受所用 computer-use 技能的动作时确认规则约束：**“Always Confirm at Action-Time”**，其中删除包含通过应用界面操作的本地数据。本轮没有执行该动作，也没有自行写入批准回执。
- 本报告没有修改原有发布门禁、设备矩阵或旧验证记录。未覆盖完整 Narrator/键盘旅程、云物化、竞态故障注入、百万文件性能、exFAT/SMB、签名安装升级及其他设备；不由本机结果推导这些组合通过。

## 后续验收入口

先修复 BUG-WIN-01，并重跑 WIN-09 及普通目录扫描/查询。完整洞察用例需要独立 Windows 测试账号或可配置的测试 KnownFolder；实际写入与恢复需要单独完成动作时确认和对应回归。原始计划默认 15 分钟有效，过期后必须生成新计划，不能修改旧计划继续执行。

临时运行器与夹具配置保留在 `.research/windows-acceptance-2026-10-22/`；本报告引用的原始 stdout/stderr、检查结果与原生 UI 文本/截图保留在本地 `.research/acceptance-raw/2026-10-22/`（不入库，含个人数据），便于独立核对；入库的是 `docs/evidence/runs/` 中逐项脱敏的运行记录。
