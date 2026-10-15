# CLI 与 Agent 协议

状态：首版完整契约草案（见“冻结边界”）。示例全部是合成数据；`cleanmanager` 已实现下文“命令面”中 MVP 的扫描、分析、计划、授权、执行、恢复与历史命令，以及 `protect`、`apps list`、`startup list`、`status`、`insights` 只读/保护命令；工具命令组其余部分仍为草案。各能力的验证状态以 `capabilities` 输出为准，未经实测的组合为 unverified。已定义的记录形状见 [JSON Schema](../schemas/v1/protocol.schema.json)。人类可读输出不构成机器接口。

## 命令面与副作用

| 命令 | 作用 | 响应记录 | 文件系统副作用 |
| --- | --- | --- | --- |
| `capabilities` | 平台、后端和受限能力说明 | `capabilities` | 不改用户目标文件 |
| `scan --root PATH` | 新扫描，返回 scan_id | JSONL `progress` 与一个 `scan_result`；失败为 `error` | 可写本工具索引与日志，不清理目标 |
| `scan show ID` | 查询扫描状态与覆盖率 | 运行中 `scan_status`，结束后 `scan_result` | 无目标写入 |
| `scan cancel ID` | 请求停止扫描 | `scan_cancel_receipt` | 无目标写入 |
| `entries list --scan ID` | 分页查询目录或大文件 | `entries_page` | 无目标写入 |
| `analyze --scan ID --profile NAME` | 规则分析、生成 candidate_set | `analysis_result` | 只写本工具结果 |
| `candidates list --set ID` | 分页查询理由与影响 | `candidates_page` | 无目标写入 |
| `plan create --candidate-set ID --mode MODE` | 生成固定计划 | `plan` | 只写本工具计划 |
| `plan show ID` | 展示计划与授权状态（清理计划与 recovery/v1 计划按存储的权威类型分派） | `plan` 与 `authorization_status` | 无目标写入 |
| `plan review ID` | 受控终端中的人工审阅与批准；恢复/清空计划写独立的恢复授权 | `authorization_status` | 只写授权记录 |
| `plan apply ID` | 核验授权并执行；按存储的计划类型调度清理执行器或恢复服务 | `operation_report`；缺授权为 `error` | 仅覆盖计划内动作 |
| `operations show ID` | 查询逐项结果 | 运行中 `operation_status`，结束后 `operation_report` | 无目标写入 |
| `operations cancel ID` | 停止调度后续动作 | `operation_cancel_receipt` | 不隐式撤销已完成动作 |
| `operations list [--limit N] [--cursor C]` | 历史分页（最近登记在前），含计划类型、授权来源、规则版本、计数与真实恢复入口 | `operation_list` | 无目标写入 |
| `operations reconcile ID` | 核对缺终态的动作（清理或恢复操作均按计划类型分派），从不重做动作 | `operation_report` | 无目标写入 |
| `recovery list [--limit N] [--cursor C]` | 分页查看仍在隔离中的对象、实时状态、提醒是否到期与恢复能力（别名 `quarantine list`） | `quarantine_list` | 无目标写入 |
| `recovery show ID` | 查看单个隔离项 | `quarantine_list` | 无目标写入 |
| `recovery plan create --restore ID,ID / --purge ID,ID` | 新建恢复或清空计划（recovery/v1，独立摘要与 15 分钟有效期） | `plan` | 只写本工具计划 |

`policy`、`rules update` 与 `mcp serve` 为后续命令，不属于首版已冻结结构；应用、启动项、维护、状态等工具命令组见下一节。授权入口先由桌面/受控人机交互提供；Agent 可使用预授权策略，不提供任意自批准端点。

## 工具命令组（变更 `add-mole-parity-suite`，草案）

每条命令的机器请求是 `cli_request` 记录：`command` 取下表的命令 ID，`params` 按命令严格校验（见 Schema 中 `params_*` 定义）。所有写入类命令只生成固定计划（`plan/v2`），缺授权返回 needs_approval；`--dry-run` 返回 `selection_preview`，与真实计划使用同一筛选逻辑且不消耗授权。非交互调用只包含 default_selected 对象，扩大范围必须给出明确对象 ID。

| 命令 ID | 终端写法 | 响应记录 | 目标副作用 |
| --- | --- | --- | --- |
| `clean.list` | `clean list`（已实现 Windows：类目、规则状态与实测登记覆盖、所属工具适配器只读预览；见 [清理目录](catalog.md)） | cleanup_catalog | 无（只运行适配器的只读预览命令） |
| `clean.preview` | `clean --scan ID --dry-run [--volume E:]`（已实现 Windows：与计划相同的分析与实时占用探测） | selection_preview | 无 |
| `clean.plan` | `clean --scan ID --mode MODE [--candidates ID,...]`（已实现 Windows；不带 ID 时只含未跳过的 default_selected） | plan（cleanup）或 error（policy_denied、protection_unavailable） | 只写计划 |
| `purge.roots.list` | `purge roots`（已实现） | purge_roots | 无 |
| `purge.roots.set` | `purge roots add\|remove --path PATH`（已实现；移除默认根记为排除） | purge_roots | 只写本工具设置（store 旁的 `purge-settings.json`） |
| `purge.preview` | `purge scan [--root PATH] [--recent-days N] [--max-depth N]`，等价 `purge --dry-run`（已实现） | selection_preview | 只写本工具索引与候选集 |
| `purge.plan` | `purge plan create (--candidates ID,ID \| --all-default-selected) [--set ID] [--mode trash\|quarantine\|delete]`（已实现，默认 trash） | plan（cleanup） | 只写计划 |
| `installer.preview` | `installer scan [--source LABEL] [--max-depth N]`，等价 `installer --dry-run`；`installer locations` 列出声明位置（已实现） | selection_preview；installer_locations | 只写本工具索引与候选集 |
| `installer.plan` | `installer plan create (--candidates ID,ID \| --source LABEL) [--set ID] [--mode ...]`（已实现；`--source` 仅交互终端） | plan（cleanup） | 只写计划 |
| `trash.plan` | `trash --scan ID --entry ID...` | plan（manual_selection） | 只写计划 |
| `trash.purge_plan` | `trash purge --item ID...` | plan（manual_selection，仅回收站内对象） | 只写计划 |
| `insights.list` | `insights [--scan ID]`（已实现 Windows 已知大对象；`--scan` 的概览卡片属 7.3，暂拒绝） | insight_list | 无 |
| `apps.list` | `apps list` | app_list | 无 |
| `apps.uninstall.plan` | `apps uninstall APP_ID [--include GROUP...]` | plan（uninstall） | 只写计划 |
| `apps.updates.check` | `apps updates [APP_ID...]` | app_list（update_status） | 仅访问更新通道 |
| `apps.updates.plan` | `apps update APP_ID...` | plan（update） | 只写计划 |
| `startup.list` | `startup list` | startup_list | 无 |
| `startup.plan` | `startup disable\|enable\|remove\|restore ITEM_ID` | plan（startup） | 只写计划 |
| `optimize.list` | `optimize list` | maintenance_catalog | 无 |
| `optimize.plan` | `optimize TASK_ID...` | plan（maintenance）；执行结果为 maintenance_report | 只写计划 |
| `status.snapshot` | `status --json` | status_snapshot | 无 |
| `status.watch` | `status --watch [--interval N] [--count M]`（`--format jsonl` 等价；默认间隔 5 s，不限次数） | JSONL 的 status_snapshot | 无 |
| `protect.list` | `protect list` | protection_list；名单不可用时为 error（protection_unavailable） | 无 |
| `protect.add` | `protect add --path P [--match descendants\|exact\|ancestors]\|--glob G\|--rule ID\|--task ID [--note TEXT]` | protection_list | 只写保护名单（指向硬拒绝位置的条目被拒绝） |
| `protect.remove` | `protect remove PROTECTION_ID` | protection_list；仅交互终端并输入条目 ID 确认 | 只写保护名单（硬拒绝与默认类目不可移除） |
| `protect.check` | `protect check --path P [--rule ID]` | protection_check | 无 |
| `doctor.report` | `doctor [--no-redact]` | doctor_report | 无（导出文件需另行确认） |
| `completion.generate` | `completion SHELL [--write-profile]` | completion_script | 仅在确认写入时修改 shell 配置 |
| `self.update.check` | `self update --check` | release_info | 仅访问发布清单 |
| `self.update.apply` | `self update` | self_update_result | 校验签名与校验和后替换本工具程序 |
| `self.remove.preview` | `self remove` | self_remove_preview；实际移除只在终端交互确认后进行 | 无 |

### 构建产物与安装包（已实现，Windows）

- `purge` 与 `installer` 的发现结果是普通 candidate_set（`candidates list --set`、`plan show/review/apply`、`operations` 与恢复均不变）。每个扫描根/声明位置各有一个扫描与候选集；预览项的 `notes` 写明所属候选集，跨根的选择需按根分别建计划（`--set ID`）。
- 构建产物候选只出现在含项目标记的目录的直接子目录（产物名目录从不被当作项目容器搜索）；依赖目录（node_modules、.venv、vendor…）恒为 review_only 并注明需联网重装；近 N 天（默认 7）有修改的项目不预选（`skip_reason: recent`）；含已跟踪文件、版本库元数据、疑似密钥文件的产物 blocked；跟踪状态无法读取为 `vcs_unknown`（不可计划）。执行前对每个产物再读一次 Git 索引，已被跟踪则该产物所有动作以 `protected`（`vcs_tracked: …`）跳过。
- 安装包按内容识别（安装引擎特征的 PE、Windows Installer 类 ID 的复合文件、含 AppxManifest 或安装载荷的 ZIP、ISO/VHD(X) 签名）；仅含文档、加密或无法列出目录的压缩包与改名的普通文件不是候选。候选恒为 review_only，带 `source_label`、签名者与“对应应用已安装”的提示；已挂载镜像与被占用文件不可计划（`skip_reason: mounted / in_use`，附推出指引）。
- 计划项 `kind`：缺省为文件；构建产物计划（trash/delete 模式）额外包含 `kind: "directory"` 的目录项（纳入摘要）。目录项在其内全部计划项成功后、自底向上执行，仅当身份与计划一致、不是链接且此刻为空时删除，从不递归；出现计划外内容为 `precondition_new_descendant`，内部有计划项未完成为 `precondition_descendant_kept`。trash 模式下若目录现场子树与计划完全一致，可整体移入回收站（一次 Shell 操作，事后再核对回收站内子树）；否则退回逐文件 + 空目录删除。quarantine 模式不含目录项（恢复需要原父目录）。

### plan/v2 计划种类与动作

| 计划种类 | 允许动作 | 可被有限策略匹配 |
| --- | --- | --- |
| cleanup | fs_trash、fs_quarantine、fs_delete、tool_adapter | 是（tool_adapter 需策略显式列出适配器） |
| manual_selection | 原位 fs_trash；仅对回收站内对象 fs_delete | 否 |
| uninstall | app_quit（强制结束只能 ask_user）、tool_adapter、fs_trash、fs_quarantine、registry_backup_delete（必须先备份）、startup_toggle | 否 |
| maintenance | maintenance_task、settings_change | 否 |
| startup | startup_toggle | 否 |
| update | app_quit、tool_adapter（包管理器/商店/签名校验通过的更新源） | 否 |
| recovery | registry_restore（目标已存在则跳过）、startup_toggle、settings_change | 否 |

`requires_admin` 必须与动作一致（需管理员的维护任务、HKLM 键、系统级启动项）。tool_adapter 只记录固定可执行文件、其 SHA-256 与 argv 数组，不接受 shell 命令字符串。

## 通用参数与输出

`--format json|jsonl|text`，`--json` 是 `--format json` 的别名；同命令重复或冲突格式参数报错。`--non-interactive` 禁止输入等待；当 stdout 非 TTY 时，默认结构化 JSON，不自动启动 TUI。TUI 需显式选择并要求终端可用。

`--timeout` 限制调用等待时间，不意味着能立即撤销操作；已创建操作返回 operation_id 供恢复查询。`--limit` 默认 50、上限 500，超出报错；游标是不透明字符串，与 scan revision/过滤/排序绑定。`--no-color` 仅作用于人类输出，JSON 从不含 ANSI 控制序列。

stdout 只写 UTF-8、无 BOM 的结构化数据；stderr 写诊断日志，日志不得泄漏凭据。所有机器记录包含 `schema_version: "1.0"`、`kind` 与 request_id。字节数为十进制字符串，时间为 UTC RFC3339；显示时再按用户时区转换。

JSON 模式只输出一个终态记录。JSONL 模式输出零个或多个 progress 记录及恰好一个终态记录；每条一行，字符串中的换行必须 JSON 转义。进度未知时 total 为 null，不伪造百分比。进程被强制终止导致缺少终态时，调用者必须按不完整处理，再凭任务 ID 查询。

请求 ID 用于日志关联，不是授权凭据。写操作需额外持久化去重键，并绑定计划摘要；重复请求返回同一 Operation 的当前状态，不重新执行已完成的动作。

## 稳定退出码

| 退出码 | 分类 | 例子 |
| --- | --- | --- |
| 0 | 完整成功 | success：完成扫描或全部计划动作成功 |
| 2 | 参数或协议错误 | invalid_argument、schema_mismatch |
| 3 | 需用户授权 | needs_approval、policy_denied |
| 4 | 系统权限不足 | permission_required |
| 5 | 部分成功 | partial：部分目录不可读、部分动作失败或跳过 |
| 6 | 前置条件或状态已过期 | plan_stale、cursor_stale、conflict |
| 7 | 能力不支持 | unsupported |
| 8 | 可诊断的系统或内部失败，或需要核对 | io_error、internal_error、protection_unavailable；operation_report 的 reconciliation_required |
| 130 | 用户取消 | cancelled |

机器可读的同一张表见 [exit-codes.json](../schemas/v1/exit-codes.json)，Rust 测试与文档验证器分别与它比对。结构化 code 是调用方分支依据，message 只供人阅读，可以本地化。error 记录携带 retryable、details 和 required_action；自动重试不适用于未知副作用结果。部分扫描返回 scan_result，status 为 partial，退出 5，而不是只输出一个丢失扫描 ID 的 error。

## 计划与执行交互

已实现（MVP）的两条正常路径，均可在 Windows 上照抄运行（`--store` 可省略，默认使用用户数据目录）：

```text
# 共同前半段：只读扫描 → 分析 → 计划
cleanmanager scan --root "%LOCALAPPDATA%\Temp" --json
cleanmanager analyze --scan s_demo --json
cleanmanager candidates list --set cs_demo --limit 50 --json
cleanmanager plan create --set cs_demo --mode quarantine --candidates c_demo --json

# 路径一：人工审阅（需真实终端，输入摘要前 8 位）
cleanmanager plan apply p_demo --json        # 无授权：退出 3，needs_approval，文件未动
cleanmanager plan review p_demo              # 终端内审阅并批准
cleanmanager plan apply p_demo --json        # operation_report，退出 0/5/8
cleanmanager operations show op_demo --json

# 路径二：有限预授权策略（创建即暂停，启用需终端）
cleanmanager policy create --scope scope_demo --rules microsoft.edge.code-cache@1.0.0 --mode quarantine --json
cleanmanager policy enable pol_demo          # 终端内输入策略 ID
cleanmanager plan create --set cs_demo --mode quarantine --all-default-selected --json
cleanmanager plan apply p_demo --json        # 策略覆盖时直接执行，不再逐次确认
```

端到端测试 `crates/cm-cli/tests/plan_flow.rs` 覆盖缺授权退出 3、机器调用者不能审阅或启用策略、暂停策略不授权，以及已审阅计划在真实 NTFS 上隔离且重复 apply 只回放原报告；预授权路径（配额预留、超额拒绝、unknown 占用配额）由 `cm-app` 执行测试覆盖。

### 隔离、恢复与历史（已实现，Windows）

恢复与清空是独立的 `recovery/v1` 计划，有自己的摘要、有效期和授权：清理计划的批准、旧的恢复批准或恢复批准都不覆盖新的恢复/清空计划，有限策略也从不匹配恢复计划，只有终端内的人工审阅能批准。`plan review`/`plan show`/`plan revoke`/`plan apply` 按存储的计划类型调度。下面接在上文“路径一”之后，可照抄运行：

```text
cleanmanager recovery list --json                                  # quarantine_list：state、reminder_due、restore_capability
cleanmanager recovery plan create --restore q_demo --json          # plan（body.engine_contract = recovery/v1）
cleanmanager plan apply rp_demo --json                             # 无恢复授权：退出 3，needs_approval
cleanmanager plan review rp_demo                                   # 终端内审阅恢复计划，输入摘要前 8 位
cleanmanager plan apply rp_demo --json                             # operation_report；原名已被占用的条目 skipped/destination_exists，退出 5
cleanmanager recovery plan create --purge q_demo --json            # 永久清空另建计划、另行审阅
cleanmanager operations list --limit 20 --json                     # operation_list：recovery.availability 与 guidance
```

恢复逐项核验：隔离位置与原父目录所在卷必须是记录的卷（否则 `volume_offline`/`volume_changed`，清单保留，不恢复到同盘符的新卷）；隔离对象的卷/文件 ID、链接与类型必须未变（`missing`、`identity_changed`、`link_or_reparse`）；原父目录必须仍是记录的身份（`parent_missing`、`parent_changed`）；原名已有任何对象时跳过为 `destination_exists`，两者都保留，不改名、不覆盖，另选位置需重新计划。清空只删除通过同样核验且逻辑大小未变的隔离对象（否则 `metadata_changed`），不会因保留期到期自动执行。结果与清理执行共用意图/结果日志、单计划单操作、身份锁和 `operations reconcile`。

`operation_report` 分开报告 `processed_logical_bytes`（处理量）、`quarantined_logical_bytes`（移入同卷隔离的量）、`estimated_freed_bytes`（仅永久删除/清空计入的逻辑估计；隔离、回收站与恢复为 0）以及卷空闲前后观测和有符号差值；差值按实测原样报告（可能为负），不归因于本工具。历史中的恢复入口如实给出：隔离为 `in_app_restore`，回收站为 `system_recycle_bin`（使用系统回收站恢复），永久删除与清空为 `unavailable`（无撤销），结果未确认为 `unknown`（先核对）。端到端测试 `crates/cm-cli/tests/recovery_flow.rs` 在真实 NTFS 上覆盖以上正常与拒绝路径；macOS 恢复动作尚未验证，当前只返回 unsupported。

`add-mole-parity-suite`：`clean` 已在 Windows 实现（`clean list` 返回 [cleanup-catalog](../examples/contracts/cleanup-catalog.json) 形状）；`apps uninstall` 仍为草案：

```text
cleanmanager clean list --json                                       # cleanup_catalog：类目状态、规则登记覆盖、适配器预览
cleanmanager clean --scan s_demo --dry-run --non-interactive --json  # selection_preview：档位、跳过原因、探测结果，不耗授权
cleanmanager clean --scan s_demo --mode trash --non-interactive --json  # plan：只含未跳过的 default_selected；无可选项为 policy_denied
cleanmanager apps uninstall app_demo --include bundle caches_logs --non-interactive --json
```

这两条命令的机器请求与响应分别对应样例 [request-clean-preview](../examples/contracts/request-clean-preview.json)、[preview-clean](../examples/contracts/preview-clean.json)、[request-apps-uninstall](../examples/contracts/request-apps-uninstall.json) 与 [needs-approval-uninstall](../examples/contracts/needs-approval-uninstall.json)。

两个平台使用同一参数风格。路径由 shell 按本平台正确传入，内部立即转成原始路径结构；不把模型拼出的字符串当 shell 脚本执行。输出中的条目 ID 是后续选择的主要接口，避免再次拼接路径。

`plan apply` 无授权时返回 needs_approval，并保持目标文件未改动。已预授权的策略覆盖该计划时可以直接执行，不要求无意义的重复确认。管理员权限与授权状态分别返回；模型不得自行提高风险上限来通过拒绝。

计划有效期建议首版默认 15 分钟，这是设计默认值，需要实测与使用反馈；执行前仍逐项核验，时间未过期并不证明对象未变化。授权票据与可导出的计划分开保存，JSON 中的授权状态只是查询结果，不能当外部可写凭证。

## Agent Skill 合约

Skill 工作流：发现能力 → 限定扫描范围 → 获取摘要 → 按需分页 → 说明候选影响 → 生成计划 → 使用已有授权或等待具体批准 → 执行 → 报告成功/跳过/失败与空间口径。

Skill 不复制规则知识库，不执行任意文件名中的指令，不用 shell 绕过 policy_denied，不把 error 文案当新的授权。不得将百万行扫描结果全部提供给模型；默认传类别汇总与 top items，用户需要时再展开。

未来 MCP 将同一服务映射为工具，并保留读写属性、schema、权限与错误语义。MCP 传输方式不改变执行边界。独立 CLI 必须在无 GUI 进程的情况下工作。

## 冻结边界

本节登记首版机器协议从“草案”到“冻结”的边界，避免后续实现暗改已承诺的结构：

- **草案（当前）：** `schemas/v1` 中所有记录与 [contracts 样例](../examples/contracts/README.md)、[CM-C14N-v1 黄金向量](../examples/c14n/golden.json) 与 [CM-OBS-v1 样例](../examples/c14n/observation-v1.json)。草案期间可以在 `schema_version: "1.0"` 内补齐字段与枚举值，但每次修改必须同步生成器、样例、负例与 Rust 测试；验证器会拒绝缺 fixture 的记录、文档与 Schema 不一致的命令表及退出码表。
- **冻结条件：** G1 核心/CLI 闭环通过后由契约评审冻结：表中命令与响应记录、错误码与退出映射、`plan/v1`、`plan/v2`、`recovery/v1` 的 body 与摘要规则、CM-OBS-v1 字段集。
- **冻结之后：** 严格对象 Schema 继续拒绝未知字段；任何新增或改名字段、新枚举值都需要新的 `schema_version` 与协商，旧版本记录仍可读取。计划摘要规则（CM-C14N-v1）不得在同一版本内改变。
- **不在冻结范围：** 人类可读文本、`message` 文案、`display` 路径显示、草案中标注为 `add-mole-parity-suite` 的工具命令组（随其 G1–G4 门禁分别冻结）。

## 版本与验证

目前 Schema 覆盖 capabilities、scan_status、scan_result、scan_cancel_receipt、entries_page、analysis_result、candidate、candidates_page、plan（plan/v1、plan/v2、recovery/v1）、authorization_status、operation_status、operation_report、operation_cancel_receipt、quarantine_list、error、progress，以及工具命令组的 cli_request、selection_preview、app_list、startup_list、maintenance_catalog、maintenance_report、status_snapshot、protection_list、insight_list、doctor_report、purge_roots、installer_locations、release_info、self_update_result、self_remove_preview、completion_script。分页包装、candidate_set 结果（`analysis_result`）、运行态查询（`scan_status`/`operation_status`）、扫描与操作取消回执、授权查询和隔离清单已补入草案。首版不提供后台任务宿主：宿主进程退出后扫描为 `interrupted`，未确认动作进入 `reconciliation_required`。`operations cancel` 不涵盖 scan。需要待核对的操作不得自动重试；以上结构在“冻结边界”一节的条件满足前不对外承诺。

计划 body 包含 ID、时间、scope 与观察值。GUI/CLI 对同一个 body 应得到相同摘要；分别新建的两个计划若 ID/时间不同，摘要也应不同。跨入口行为比较应固定这些输入或比较选择和动作的语义，不能要求不同 body 得到相同 hash。

v1 草案采用严格对象 schema，拒绝未定义字段。未来增加字段需要协商新的 schema_version；不要一边严格拒绝未知字段，一边宣称所有新增字段完全兼容。二进制版本、数据库版本、规则版本与协议版本分别维护。

Schema 校验只能证明形状，不能证明 OS 对象存在、授权真实性或删除安全。仓库验证器额外检查摘要、字节范围、时间顺序、raw path 编码和汇总一致性；真正执行语义仍由平台测试验证。运行方法见 [文档索引](README.md)。
