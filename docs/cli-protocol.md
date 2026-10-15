# CLI 与 Agent 协议

状态：首版完整契约草案（见“冻结边界”）。示例全部是合成数据；`cleanmanager` 目前只实现了 `capabilities` 冒烟命令，其余命令尚未实现，返回的能力状态均为 unverified。已定义的记录形状见 [JSON Schema](../schemas/v1/protocol.schema.json)。人类可读输出不构成机器接口。

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
| `plan show ID` | 展示计划与授权状态 | `plan` 与 `authorization_status` | 无目标写入 |
| `plan review ID` | 受控终端中的人工审阅与批准 | `authorization_status` | 只写授权记录 |
| `plan apply ID` | 核验授权并执行 | `operation_report`；缺授权为 `error` | 仅覆盖计划内动作 |
| `operations show ID` | 查询逐项结果 | 运行中 `operation_status`，结束后 `operation_report` | 无目标写入 |
| `operations cancel ID` | 停止调度后续动作 | `operation_cancel_receipt` | 不隐式撤销已完成动作 |
| `recovery list` | 分页查看隔离项与恢复状态 | `quarantine_list` | 无目标写入 |
| `recovery show ID` | 查看单个隔离项 | `quarantine_list` | 无目标写入 |
| `recovery plan create --restore ID... / --purge ID...` | 新建恢复或清空计划（recovery/v1） | `plan` | 只写本工具计划 |

`policy`、`rules update` 与 `mcp serve` 为后续命令，不属于首版已冻结结构；应用、启动项、维护、状态等工具命令组见下一节。授权入口先由桌面/受控人机交互提供；Agent 可使用预授权策略，不提供任意自批准端点。

## 工具命令组（变更 `add-mole-parity-suite`，草案）

每条命令的机器请求是 `cli_request` 记录：`command` 取下表的命令 ID，`params` 按命令严格校验（见 Schema 中 `params_*` 定义）。所有写入类命令只生成固定计划（`plan/v2`），缺授权返回 needs_approval；`--dry-run` 返回 `selection_preview`，与真实计划使用同一筛选逻辑且不消耗授权。非交互调用只包含 default_selected 对象，扩大范围必须给出明确对象 ID。

| 命令 ID | 终端写法 | 响应记录 | 目标副作用 |
| --- | --- | --- | --- |
| `clean.list` | `clean list --scan ID` | selection_preview | 无 |
| `clean.preview` | `clean --scan ID --dry-run` | selection_preview | 无 |
| `clean.plan` | `clean --scan ID` | plan（cleanup）或 error | 只写计划 |
| `purge.roots.list` | `purge roots` | purge_roots | 无 |
| `purge.roots.set` | `purge roots --set PATH...` | purge_roots | 只写本工具设置 |
| `purge.preview` | `purge --dry-run [--recent-days N]` | selection_preview | 无 |
| `purge.plan` | `purge [--recent-days N]` | plan（cleanup） | 只写计划 |
| `installer.preview` | `installer --dry-run` | selection_preview | 无 |
| `installer.plan` | `installer` | plan（cleanup） | 只写计划 |
| `trash.plan` | `trash --scan ID --entry ID...` | plan（manual_selection） | 只写计划 |
| `trash.purge_plan` | `trash purge --item ID...` | plan（manual_selection，仅回收站内对象） | 只写计划 |
| `insights.list` | `insights [--scan ID]` | insight_list | 无 |
| `apps.list` | `apps list` | app_list | 无 |
| `apps.uninstall.plan` | `apps uninstall APP_ID [--include GROUP...]` | plan（uninstall） | 只写计划 |
| `apps.updates.check` | `apps updates [APP_ID...]` | app_list（update_status） | 仅访问更新通道 |
| `apps.updates.plan` | `apps update APP_ID...` | plan（update） | 只写计划 |
| `startup.list` | `startup list` | startup_list | 无 |
| `startup.plan` | `startup disable\|enable\|remove\|restore ITEM_ID` | plan（startup） | 只写计划 |
| `optimize.list` | `optimize list` | maintenance_catalog | 无 |
| `optimize.plan` | `optimize TASK_ID...` | plan（maintenance）；执行结果为 maintenance_report | 只写计划 |
| `status.snapshot` | `status --json` | status_snapshot | 无 |
| `status.watch` | `status --watch --interval N` | JSONL 的 status_snapshot | 无 |
| `protect.list` | `protect list` | protection_list | 无 |
| `protect.add` | `protect add PATH\|--glob G\|--rule ID\|--task ID` | protection_list | 只写保护名单 |
| `protect.remove` | `protect remove PROTECTION_ID` | protection_list | 只写保护名单（硬拒绝与默认类目不可移除） |
| `doctor.report` | `doctor [--no-redact]` | doctor_report | 无（导出文件需另行确认） |
| `completion.generate` | `completion SHELL [--write-profile]` | completion_script | 仅在确认写入时修改 shell 配置 |
| `self.update.check` | `self update --check` | release_info | 仅访问发布清单 |
| `self.update.apply` | `self update` | self_update_result | 校验签名与校验和后替换本工具程序 |
| `self.remove.preview` | `self remove` | self_remove_preview；实际移除只在终端交互确认后进行 | 无 |

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

草案（`add-mole-parity-suite`，尚未实现）：

```text
cleanmanager clean --scan s_demo --dry-run --non-interactive --json
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

目前 Schema 覆盖 capabilities、scan_status、scan_result、scan_cancel_receipt、entries_page、analysis_result、candidate、candidates_page、plan（plan/v1、plan/v2、recovery/v1）、authorization_status、operation_status、operation_report、operation_cancel_receipt、quarantine_list、error、progress，以及工具命令组的 cli_request、selection_preview、app_list、startup_list、maintenance_catalog、maintenance_report、status_snapshot、protection_list、insight_list、doctor_report、purge_roots、release_info、self_update_result、self_remove_preview、completion_script。分页包装、candidate_set 结果（`analysis_result`）、运行态查询（`scan_status`/`operation_status`）、扫描与操作取消回执、授权查询和隔离清单已补入草案。首版不提供后台任务宿主：宿主进程退出后扫描为 `interrupted`，未确认动作进入 `reconciliation_required`。`operations cancel` 不涵盖 scan。需要待核对的操作不得自动重试；以上结构在“冻结边界”一节的条件满足前不对外承诺。

计划 body 包含 ID、时间、scope 与观察值。GUI/CLI 对同一个 body 应得到相同摘要；分别新建的两个计划若 ID/时间不同，摘要也应不同。跨入口行为比较应固定这些输入或比较选择和动作的语义，不能要求不同 body 得到相同 hash。

v1 草案采用严格对象 schema，拒绝未定义字段。未来增加字段需要协商新的 schema_version；不要一边严格拒绝未知字段，一边宣称所有新增字段完全兼容。二进制版本、数据库版本、规则版本与协议版本分别维护。

Schema 校验只能证明形状，不能证明 OS 对象存在、授权真实性或删除安全。仓库验证器额外检查摘要、字节范围、时间顺序、raw path 编码和汇总一致性；真正执行语义仍由平台测试验证。运行方法见 [文档索引](README.md)。
