# CLI 与 Agent 协议

状态：首版完整契约草案（见“冻结边界”）。`cleanmanager` 已实现下文“命令面”中 MVP 的扫描（含等待、取消、后台宿主与宿主退出核对）、目录项分页、分析、计划、授权、执行、恢复与历史命令，`clean`、`purge`、`installer`、`protect`、`apps list`、`startup list`、`status`、`insights` 等命令组，以及 `completion`、`doctor`、`help` 与中英文人类可读输出；工具命令组的其余部分仍为草案（下表逐行标注）。各能力的验证状态以 `capabilities` 输出为准，未经实测的组合为 unverified。记录形状见 [JSON Schema](../schemas/v1/protocol.schema.json)；示例均为合成数据。人类可读输出不构成机器接口。

独立安装与运行方式见 [README 的 CLI 一节](../README.md#独立-cli安装与运行)；随附的 Claude/Codex Agent Skill 见 [skills/cleanmanager](../skills/cleanmanager/SKILL.md)。

## 命令面与副作用

| 命令 | 作用 | 响应记录 | 文件系统副作用 |
| --- | --- | --- | --- |
| `capabilities` | 平台、后端和受限能力说明 | `capabilities` | 不改用户目标文件 |
| `scan --root PATH [--timeout S]` | 新扫描，由本进程承载并返回 scan_id；`--timeout` 是等待预算 | JSON 为一个 `scan_result`；JSONL 为若干 `progress` 与一个 `scan_result`；失败为 `error` | 只写本工具索引，不清理目标 |
| `scan --root PATH --detach` | 另起一个 cleanmanager 进程承载扫描，记录建立后立即返回 | `scan_status` | 同上 |
| `scan show ID` | 查询扫描状态与覆盖率；宿主已退出的扫描先标为中断 | 运行中 `scan_status`，结束后 `scan_result` | 无目标写入 |
| `scan wait ID [--timeout S]` | 等待终态（默认 60 s，最长 86400 s） | `scan_result`；预算用尽为运行态 `scan_status`（退出 8） | 无目标写入 |
| `scan cancel ID` | 请求停止扫描（回执不是终态） | `scan_cancel_receipt` | 无目标写入 |
| `entries list --scan ID [--sort S] [--kind K] [--parent E] [--name T]` | 分页查询目录或大文件（`--limit` 默认 50、上限 500，`--cursor` 续页）；本页目录可带只读的 `cleanable_hint`（依据 `build_artifact`/`catalog_rule`/`cachedir_tag`、`rule_id`、`detail`、入口 `flow`，无依据或受保护时省略，见[磁盘视图线索](system-status.md#磁盘视图的可手动清理线索din-05)）；扫描之后执行的操作已移走的条目带 `removed`（`mode` 为 `trash`/`quarantine`/`permanent`、`operation_id`、`at`、`recoverable`、`inherited` 表示随上级文件夹一起移走），目录另带其下已移走的 `removed_count`、`removed_bytes`（已知部分，全部未知时为 null）、`removed_unknown_count`；都来自审计记录，不重新读取磁盘，较新的同根扫描不带这些字段（见[扫描之后的操作](#扫描之后的操作)，样例 [entries-page-removed](../examples/contracts/entries-page-removed.json)） | `entries_page` | 无目标写入 |
| `analyze --scan ID` | 规则分析、生成 candidate_set | `analysis_result` | 只写本工具结果 |
| `candidates list --set ID [--limit N] [--cursor C]` | 分页查询理由与影响：档位与理由、再生成与离线影响、年龄阈值口径、恢复限制（见“dry-run 即计划预览”） | `candidates_page` | 无目标写入 |
| `plan create --set ID --mode MODE --candidates ID,ID` | 生成固定计划；或以 `--all-default-selected` 取全部默认选中项；保护名单不可用时为 protection_unavailable（`purge`/`installer plan create` 同样）；trash 模式下目标所在卷没有可用回收站时为 unsupported（`trash_unavailable`，见“dry-run 即计划预览”） | `plan` | 只写本工具计划 |
| `plan show ID` | 展示计划与授权状态（清理计划与 recovery/v1 计划按存储的权威类型分派）；清理计划附 `authorized_by`（授权它的人工审阅或策略 ID/版本）与仍可被策略授权时的 `policy_matches` | `plan` 与 `authorization_status` | 无目标写入 |
| `plan review ID` | 受控终端中的人工审阅与批准；恢复/清空计划写独立的恢复授权 | `authorization_status` | 只写授权记录 |
| `plan revoke ID` | 撤销该计划已有的批准 | `authorization_status` | 只写授权记录 |
| `plan apply ID` | 核验授权并执行；按存储的计划类型调度清理执行器或恢复服务 | `operation_report`；缺授权为 `error`（清理计划附 `details.policy_matches`）；保护名单不可用为 `error`（protection_unavailable），不登记操作 | 仅覆盖计划内动作 |
| `operations show ID` | 查询逐项结果 | 运行中 `operation_status`，结束后 `operation_report`；宿主已退出、尚未核对时为临时 `operation_report`（状态 reconciliation_required，退出 8） | 无目标写入 |
| `operations cancel ID` | 停止调度后续动作（草案，尚未接入：首版执行在发起进程内完成） | `operation_cancel_receipt` | 不隐式撤销已完成动作 |
| `operations list [--limit N] [--cursor C]` | 历史分页（最近登记在前），含计划类型、授权来源、规则版本、计数与真实恢复入口 | `operation_list` | 无目标写入 |
| `operations reconcile ID` | 核对缺终态的动作（清理或恢复操作均按计划类型分派），从不重做动作；保留原授权来源，另记 `reconciled_at` | `operation_report` | 无目标写入 |
| `recovery list [--limit N] [--cursor C]` | 分页查看仍在隔离中的对象（最近隔离的在前，与 `operations list` 相同；桌面“恢复”页同序）、实时状态、提醒是否到期与恢复能力（别名 `quarantine list`） | `quarantine_list` | 无目标写入 |
| `recovery show ID` | 查看单个隔离项 | `quarantine_list` | 无目标写入 |
| `recovery plan create --restore ID,ID / --purge ID,ID` | 新建恢复或清空计划（recovery/v1，独立摘要与 15 分钟有效期） | `plan` | 只写本工具计划 |
| `completion print SHELL` | 生成 bash/zsh/fish/PowerShell 补全；`--format text` 输出可直接 eval/source 的脚本 | `completion_script` | 无 |
| `completion install SHELL [--profile PATH]` | 预览将追加到 shell 配置文件的内容；仅终端内输入 append 后写入 | `completion_script` | 仅在终端确认后追加配置文件 |
| `doctor [--export PATH] [--no-redact] [--repair-index]` | 诊断报告，默认脱敏；导出先预览，终端内输入 export 后写出；`--repair-index` 把不可用的扫描索引移到一旁并新建空索引 | `doctor_report`（`--repair-index` 时附 index_repair 字段） | 仅在终端确认后写出报告文件；`--repair-index` 只改名 `index.db` 并新建空索引 |

`help` 只输出人类可读的命令表（JSON 模式下为 invalid_argument）。`policy`、`rules update` 与 `mcp serve` 为后续命令，不属于首版已冻结结构（`policy list` 的 `policy_list` 记录不在 schemas/v1 中）；应用、启动项、维护、状态等工具命令组见下一节。授权入口先由桌面/受控人机交互提供；Agent 可使用预授权策略，不提供任意自批准端点。

## 工具命令组（变更 `add-mole-parity-suite`，草案）

每条命令的机器请求是 `cli_request` 记录：`command` 取下表的命令 ID，`params` 按命令严格校验（见 Schema 中 `params_*` 定义）。所有写入类命令只生成固定计划（`plan/v2`），缺授权返回 needs_approval；`--dry-run` 返回 `selection_preview`，与真实计划使用同一筛选逻辑且不消耗授权（见“dry-run 即计划预览”）。非交互调用只包含 default_selected 对象，扩大范围必须给出明确对象 ID（见“非交互与授权边界”）。

| 命令 ID | 终端写法 | 响应记录 | 目标副作用 |
| --- | --- | --- | --- |
| `clean.list` | `clean list`（已实现 Windows：类目、规则状态与实测登记覆盖、所属工具适配器只读预览；见 [清理目录](catalog.md)） | cleanup_catalog | 无（只运行适配器的只读预览命令） |
| `clean.preview` | `clean --scan ID --dry-run [--volume E:]`（已实现 Windows：与计划相同的分析与实时占用探测） | selection_preview | 无 |
| `clean.plan` | `clean --scan ID --mode MODE [--candidates ID,...]`（已实现 Windows；不带 ID 时只含未跳过的 default_selected；扫描后状态变化的项（目录：其中任一文件）列入 stale_items；trash 模式下卷没有可用回收站时整份拒绝） | plan（cleanup）或 error（policy_denied、plan_stale、protection_unavailable、unsupported（`trash_unavailable`）） | 只写计划 |
| `purge.roots.list` | `purge roots`（已实现） | purge_roots | 无 |
| `purge.roots.set` | `purge roots add\|remove --path PATH`（已实现；移除默认根记为排除） | purge_roots | 只写本工具设置（store 旁的 `purge-settings.json`） |
| `purge.preview` | `purge scan [--root PATH] [--recent-days N] [--max-depth N]`，等价 `purge --dry-run`（已实现） | selection_preview | 只写本工具索引与候选集 |
| `purge.plan` | `purge plan create (--candidates ID,ID \| --all-default-selected) [--set ID] [--mode trash\|quarantine\|delete]`（已实现，默认 trash） | plan（cleanup） | 只写计划 |
| `installer.preview` | `installer scan [--source LABEL] [--max-depth N]`，等价 `installer --dry-run`；`installer locations` 列出声明位置（已实现） | selection_preview；installer_locations | 只写本工具索引与候选集 |
| `installer.plan` | `installer plan create (--candidates ID,ID \| --source LABEL) [--set ID] [--mode ...]`（已实现；`--source` 仅交互终端） | plan（cleanup） | 只写计划 |
| `trash.plan` | `trash --scan ID --entry ID[,ID...]`（已实现 Windows 与 macOS（macOS 仅文件）；只接受已发布扫描的条目 ID，见 [手选移入回收站](manual-trash.md)） | plan（manual_selection）；受保护/覆盖不全为 error（policy_denied）；指向受保护系统位置（如 System32）的链接同为 policy_denied，message 写明链接解析到的受保护位置（只读解析，从不经链接写入）；卷没有可用回收站/废纸篓（含该卷已关闭回收站）为 error（unsupported，message 以 `trash_unavailable:` 开头并列出卷，`required_action` 给出下一步） | 只写计划 |
| `trash.purge_plan` | `trash purge --item ID[,ID...]`（已实现 Windows 与 macOS；ID 来自 `trash list`） | plan（manual_selection，仅回收站内对象） | 只写计划 |
| `trash.inspect` | `trash inspect --scan ID --entry ID`（已实现 Windows 与 macOS） | trash_inspection | 无（只读索引与占用进程查询，不打开、不下载） |
| `trash.list` | `trash list`（已实现 Windows 与 macOS，仅当前用户的回收站/废纸篓） | trash_list；macOS 的 `~/.Trash` 受隐私保护无法列出时为 error（permission_required），从不当作空 | 无 |
| `insights.list` | `insights`（已实现 Windows 已知大对象） | insight_list | 无 |
| `insights.overview` | `insights overview [--older-than-days N]`（已实现 Windows，见 [系统状态与洞察卡片](system-status.md)） | insight_overview | 无 |
| `insights.downloads` | `insights downloads [--older-than-days N] [--limit N]`（已实现 Windows；按大小排序、无预选） | old_downloads_review | 无 |
| `apps.list` | `apps list` | app_list | 无 |
| `apps.uninstall.plan` | `apps uninstall plan create --instance ID [--instance ID …] [--end-running]`（已实现 Windows，第一段：退出请求 + 经路径/哈希/签名核验的卸载适配；重复 `--instance` 把多个应用放进同一份计划（UNI-11，最多 20 个；`--instance` 是唯一可以重复的选项，同一实例重复为 invalid_argument）；`--end-running` 仅交互终端；见 [应用卸载](app-uninstall.md)） | plan（uninstall，附 `leftover_estimate` 估计，以及逐应用的 `uninstall_apps` 与被拒绝的 `uninstall_refused`）；部分应用被拒绝时仍返回计划，退出码 5；全部被拒绝时为 error：单个应用时与以前相同（不支持来源为 unsupported、保护类与签名不符为 policy_denied，`required_action` 指向系统设置），多个应用时逐个列出原因 | 只写计划 |
| `apps.uninstall.list` | `apps uninstall list`（已实现 Windows） | app_uninstall_list | 无 |
| `apps.uninstall.leftovers` | `apps uninstall leftovers --operation OP_ID`（已实现 Windows，第一段完成后） | uninstall_leftovers（另附逐应用的 `apps`；批量时顶层 `app` 为 null）；没有任何应用的卸载程序成功时为 invalid_argument | 无（只读） |
| `apps.uninstall.leftovers_plan` | `apps uninstall leftovers plan create --operation OP_ID [--items ID,ID]`（已实现 Windows，第二段，单独审阅；批量时包含所有卸载程序成功的应用的残留） | plan（uninstall）；kept/blocked 对象为 policy_denied | 只写计划 |
| `apps.uninstall.orphans` | `apps uninstall orphans`（已实现 Windows：失效启动项及其同名数据目录；macOS：`~/Library` 中按 bundle ID 无任何已安装应用或安装收据认领的条目） | uninstall_leftovers（review_only，从不预选） | 无 |
| `apps.uninstall.orphans_plan` | `apps uninstall orphans plan create --items ID,ID`（已实现 macOS，单独审阅；只含点名的孤立残留，移入废纸篓；Windows 为 unsupported，孤立残留只供审阅） | plan（uninstall，`fs_trash`）；kept/blocked 对象为 policy_denied/permission_required，未点名为 invalid_argument | 只写计划 |
| `apps.uninstall.restore_plan` | `apps uninstall restore --operation OP_ID`（已实现 Windows） | plan（recovery） | 只写计划 |
| `apps.updates.check` | `apps updates`；`apps updates show` 只读上次结果（已实现 Windows，见 [应用更新](app-updates.md)） | app_update_list | 仅访问更新通道；检查关闭时无任何请求 |
| `apps.updates.config` | `apps updates config [--checks on\|off]`（已实现） | app_update_settings | 只写本工具设置（`app-updates.json`） |
| `apps.updates.plan` | `apps update --app ID[,ID...]`（已实现 Windows） | app_update_plan（含 plan（update）与其余应用的去向） | 只写计划 |
| `startup.list` | `startup list` | startup_list | 无 |
| `startup.plan` | `startup disable\|enable\|remove-stale ITEM_ID`（已实现 Windows：Run 值与启动文件夹；见 [启动项](startup-items.md)） | plan（startup）；受保护为 policy_denied，系统级非管理员为 permission_required，无已验证机制为 unsupported | 只写计划 |
| `startup.restore_plan` | `startup restore --operation OP_ID`（已实现 Windows） | plan（recovery） | 只写计划 |
| `optimize.list` | `optimize list [--search TEXT]`（已实现 Windows：任务目录、只读适用性探测、用户保护与禁止项清单；见 [系统维护](system-maintenance.md)） | maintenance_catalog | 无（只读探测） |
| `optimize.plan` | `optimize plan create [--tasks ID,ID]`（已实现 Windows；不带 `--tasks` 只含未受保护且未判定不适用的任务；点名受保护任务为 policy_denied、点名禁止项为 policy_denied） | plan（maintenance）；`plan apply` 的结果为 maintenance_report | 只写计划 |
| `optimize.diagnose` | `optimize diagnose [--window SECONDS]`（已实现 Windows，1–30 s） | maintenance_diagnosis | 无（只读，不结束进程、不改设置） |
| `optimize.history` | `optimize history [--limit N]`（已实现） | maintenance_history | 无 |
| `status.snapshot` | `status --json`（可选 `network`：合计只计物理网卡，回环排除，隧道/虚拟网卡单独列出；无法分类时 `may_double_count: true`，见 [系统状态](system-status.md#网络合计去重sts-07)） | status_snapshot | 无 |
| `status.watch` | `status --watch [--interval N] [--count M]`（`--format jsonl` 等价；默认间隔 5 s，不限次数；未及时完成的采样以原时间重复并标 `stale`） | JSONL 的 status_snapshot；开启提醒时另有 status_alert | 无 |
| `status.alerts` | `status alerts [--state on\|off] [--cpu-threshold PCT] [--window-seconds N] [--cooldown-seconds N]`（已实现，默认关闭） | status_alert_settings | 只写本工具设置（`status-alerts.json`） |
| `process.list` | `process list [--by cpu\|memory\|disk\|network\|name] [--search TEXT] [--limit N]`；`process show PID`（已实现 Windows 与 macOS）。每行的 `started_at` 是标识进程实例的精确启动时间（Windows 为 FILETIME 刻度、macOS 为 Unix 纪元以来的微秒，十进制字符串，计划绑定它），`started_at_utc` 是同一时刻的 RFC 3339 UTC 时间（整秒，读不到为 null）；文本输出只显示后者 | process_list | 无 |
| `process.pin` | `process pin\|unpin NAME`（已实现） | process_pins | 只写本工具设置（`process-pins.json`） |
| `process.end_plan` | `process end PID`（已实现 Windows 与 macOS；关键进程、安全软件与本工具自身为 policy_denied，macOS 上其他用户的进程也是） | plan（process） | 只写计划 |
| `protect.list` | `protect list` | protection_list；名单不可用时为 error（protection_unavailable） | 无 |
| `protect.add` | `protect add --path P [--match descendants\|exact\|ancestors]\|--glob G\|--rule ID\|--task ID [--note TEXT]` | protection_list | 只写保护名单（指向硬拒绝位置的条目被拒绝） |
| `protect.remove` | `protect remove PROTECTION_ID` | protection_list；仅交互终端并输入条目 ID 确认 | 只写保护名单（硬拒绝与默认类目不可移除） |
| `protect.check` | `protect check --path P [--rule ID]` | protection_check | 无 |
| `doctor.report` | `doctor [--export PATH] [--no-redact] [--repair-index]`（已实现） | doctor_report | 无（导出文件需终端确认）；`--repair-index` 只移走不可用的扫描索引 |
| `completion.generate` | `completion print SHELL`；`completion install SHELL [--profile PATH]`（已实现） | completion_script | 仅在终端确认写入时修改 shell 配置 |
| `self.update.check` | `self update --check [--manifest URL\|PATH]`（已实现；见 [安装与更新](install.md)） | release_info；包管理器安装为 `use_package_manager` 并给出其命令 | 仅读取签名发布清单 |
| `self.update.apply` | `self update [--manifest URL\|PATH]`（已实现） | self_update_result | 校验签名、版本、存储兼容与逐文件校验和并试运行新程序后，原子替换本工具程序（失败回滚）；持有生命周期独占锁 |
| `self.install` | `self install --dir PATH [--add-to-path]`（已实现 Windows） | self_install_result；目录非空或已有安装为 conflict | 仅终端输入 install 后复制到新目录；`--add-to-path` 只追加用户 PATH 且 PATH 上有不同的 cleanmanager 时拒绝 |
| `self.remove.preview` | `self uninstall [--install-dir PATH] [--remove-user-data]`（已实现） | self_remove_preview；实际移除只在终端输入 uninstall 且计划摘要未变时进行 | 无（预览）；确认后只移除本工具自身足迹 |

### 实验功能（D15）

上表的工具命令组都属于尚未通过发布门禁的能力，**默认关闭**（设计 D15“单项未通过则该能力关闭”）。命令属于哪项能力、门禁是什么：

| 命令组 | 能力 | 门禁 |
| --- | --- | --- |
| `clean list`、`clean` | `clean.catalog` | G1 |
| `purge …` | `purge.artifacts` | G1 |
| `installer …` | `installer.cleanup` | G1 |
| `trash …` | `trash.manual` | G1 |
| `insights …` | `insights.overview` | G1 |
| `apps uninstall …` | `apps.uninstall` | G3 |
| `apps updates …`、`apps update` | `apps.updates` | G3 |
| `startup …` | `startup.toggle` | G3 |
| `optimize …` | `maintenance.run` | G3 |
| `status`、`status alerts` | `status.metrics` | G3 |
| `process …` | `process.end` | G3 |
| `self update` | `lifecycle.self_update` | G3 |
| `diag …` | `diag.ai` | AID |

- 关闭时，这些命令在做任何读写之前返回一条 `error`：`code: "unsupported"`（退出码 7），`message` 写明能力、门禁与开启方法，`required_action` 为 `cleanmanager experimental --state on`。`plan review`/`plan apply` 对已存储的 `plan/v2` 按其种类检查同样的开关（`manual_selection` → `trash.manual`，`uninstall` → `apps.uninstall`，`maintenance` → `maintenance.run`，`startup` → `startup.toggle`，`update` → `apps.updates`，`process` → `process.end`），拒绝时 `details.plan_id` 为该计划；`plan show`/`plan revoke` 不受影响。
- `cleanmanager experimental` 查看开关与每项能力（`experimental_features` 记录：`experimental_features`、每项的 `capability`/`gate`/`gate_passed`/`enabled`）；`cleanmanager experimental --state on|off` 只改本机存储中的 `features.json`，桌面端“设置 › 实验功能”读写同一个文件。没有环境变量或隐藏参数可以绕过。
- `capabilities` 随开关变化：关闭时对标能力报告为 `unsupported`，原因为 `off until gate Gx passes; experimental features are disabled`；开启后在 Windows 上仍为 `unverified`，从不报告为 `available`。
- 扫描、目录项、分析、`plan create`/v1 规则清理、隔离与恢复、`operations`、`protect`、`policy`、`apps list`、`capabilities`、`doctor`、`completion`、`self install|uninstall` 属于首版核心，不受开关影响。
- 参数先于开关检查（WFA-03）：未知选项、缺少的位置参数（如 `process show` 不带 PID）在开关关闭和开启时都是 `invalid_argument`（退出 2）。这一步只把命令词与选项和参数表比对，不读取或写入任何内容，开关仍不可绕过。

### 构建产物与安装包（已实现，Windows）

- `purge` 与 `installer` 的发现结果是普通 candidate_set（`candidates list --set`、`plan show/review/apply`、`operations` 与恢复均不变）。每个扫描根/声明位置各有一个扫描与候选集；预览项的 `notes` 写明所属候选集，跨根的选择需按根分别建计划（`--set ID`）。
- 构建产物候选只出现在含项目标记的目录的直接子目录（产物名目录从不被当作项目容器搜索）；依赖目录（node_modules、.venv、vendor…）恒为 review_only 并注明需联网重装；近 N 天（默认 7）有修改的项目不预选（`skip_reason: recent`）；含已跟踪文件、版本库元数据、疑似密钥文件的产物 blocked；跟踪状态无法读取为 `vcs_unknown`（不可计划）。执行前对每个产物再读一次 Git 索引，已被跟踪则该产物所有动作以 `protected`（`vcs_tracked: …`）跳过。
- 安装包按内容识别（安装引擎特征的 PE、Windows Installer 类 ID 的复合文件、含 AppxManifest 或安装载荷的 ZIP、ISO/VHD(X) 签名）；仅含文档、加密或无法列出目录的压缩包与改名的普通文件不是候选。候选恒为 review_only，带 `source_label`、签名者与“对应应用已安装”的提示；已挂载镜像与被占用文件不可计划（`skip_reason: mounted / in_use`，附推出指引）。另一进程以不共享读的方式打开的文件读不到内容：名称像安装包的（`.exe`、`.msi`、`.zip`、`.iso`、`.vhdx` 等）仍列出并计入摘要，`skip_reason: in_use`，`notes` 写明“not recognized by content”与占用进程（Restart Manager 能给出时，否则说明无法确定），没有可用模式、不可计划；松开后重新扫描即按内容识别。
- 计划项 `kind`：缺省为文件；构建产物计划（trash/delete 模式）额外包含 `kind: "directory"` 的目录项（纳入摘要）。目录项在其内全部计划项成功后、自底向上执行，仅当身份与计划一致、不是链接且此刻为空时删除，从不递归；出现计划外内容为 `precondition_new_descendant`，内部有计划项未完成为 `precondition_descendant_kept`。trash 模式下若目录现场子树与计划完全一致，可整体移入回收站（一次 Shell 操作，事后再核对回收站内子树）；否则退回逐文件 + 空目录删除。quarantine 模式不含目录项（恢复需要原父目录）。

### 系统维护（已实现，Windows）

- 维护运行是 `maintenance` 种类的 plan/v2 计划：`optimize plan create` → `plan review`（终端审阅）→ `plan apply`。`rule_set_digest` 是本构建任务目录（ID、版本、是否需管理员）的摘要；已存计划中与本构建目录不一致的任务记为 skipped（`task_version_changed`），不会运行。
- `plan apply` 先核对用户保护（PRT-06，`protect add --task ID`，记为 skipped `protected`），再在任何任务运行前把需管理员的任务集中起来**只请求一次**提权：已是管理员则进程内执行；否则通过 UAC 启动一个短生命周期工作进程，只接受本计划摘要内的这些任务 ID（见 [提权边界](privilege-boundary.md)）。拒绝 UAC → 这些任务 skipped（`elevation_declined`），其余任务照常执行。
- 每项任务先做只读适用性探测，再执行固定动作；结果为 applied、unchanged、skipped、unavailable、attention、failed 之一并附 `code` 与原因。`all_clear` 仅在没有 failed/attention 时为真；退出码：全部 applied/unchanged 为 0，否则 5（全部 failed 为 8）。
- 每次运行写入 `maintenance_report`（任务版本、入口、执行者、提权情况、逐项结果与时间），`optimize history` 与 `operations list`（`action: maintenance`，恢复 `unavailable`）可查；维护动作没有通用撤销。
- macOS（14.3/14.5）：没有提权路径，`maintenance_catalog.elevation` 为 `unavailable`，需管理员的任务恒为 skipped（`elevation_unavailable`）。设置任务在计划中是 `settings_change` 动作（`from_value` 为审阅时的值）；成功写入后操作的恢复为 `in_app_restore`，`optimize restore --operation OP_ID` 生成 `recovery` 计划把原值改回。卷任务的 `maintenance_task` 动作带 `volume`（计划时 `--volume` 点名的挂载点），受摘要覆盖。`maintenance_task_info.reversible` 只有设置任务为 true。

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
| process | process_end（恰好一个；绑定 PID、创建时间、可执行路径与 SHA-256；创建时间 `started_at` 为平台精确值，另附供人阅读的 `started_at_utc`） | 否 |

`requires_admin` 必须与动作一致（需管理员的维护任务、HKLM 键、系统级启动项）。tool_adapter 只记录固定可执行文件、其 SHA-256 与 argv 数组，不接受 shell 命令字符串。

已实现（`cm_core::plan::v2`、`cm_app::plans`）：`plan_kind` 在 body 内，受 CM-C14N-v1 摘要覆盖；每种类的允许动作、`policy_matchable`（仅 cleanup 可为 true）、`requires_admin` 一致性、manual_selection 的位置约束（且一份计划不混合移入与永久删除）、恢复类 `startup_toggle restore` 须带 `backup_id`、`registry_restore` 须 `on_existing: skip`，在建计划时与执行前各校验一次，违规的已存 body 在任何动作前以 policy_denied 拒绝。plan/v2 的授权是绑定 `plan_kind` 与摘要的独立回执类型，只来自终端审阅；执行器从不为 plan/v2 查询策略，所以即使有已启用的清理策略，manual_selection/startup/recovery 计划仍为 needs_approval。`plan show/review/revoke/apply` 与 `operations reconcile` 按存储的契约与种类调度（plan/v1 → 清理执行器，recovery/v1 → 隔离恢复，plan/v2 → 按种类）；本版本执行 manual_selection、startup、recovery、maintenance（见上文“系统维护”）、process（结束单个进程）与 update（经签名验证的包管理器更新），其余种类可校验与展示但 apply 返回 unsupported。可选、受摘要覆盖的字段：`plan_target.kind`（directory）、文件动作的 `subtree`（整目录移动绑定的成员摘要）与 `trash_entry_id`（回收项的有序步骤）、`startup_toggle` 的 `definition`/`observed_sha256`/`backup_id`、`registry_restore` 的 `value_name`/`backup_sha256`、`process_end` 的 `started_at_utc`（只供阅读，进程仍按 `started_at` 识别；较早的计划没有它，摘要不变）。旧的 plan/v1 与 recovery/v1 计划、摘要与回执不变。

## 通用参数与输出

- 格式：`--format json|jsonl|text`；`--json` 是 `--format json` 的别名，`human` 是 `text` 的别名；同一命令重复或冲突的格式参数报 invalid_argument。stdout 不是终端时默认 JSON；从不自动启动 TUI 或等待输入。
- `--non-interactive`：任何命令都接受，进程内所有提示（计划审阅、策略启用、保护移除、补全安装、Doctor 导出）都直接按“无人确认”处理；stdin 或 stdout 不是终端时同样视为非交互。非交互从不意味着授权。
- `--lang zh-CN|en`（或环境变量 `CLEANMANAGER_LANG`，否则跟随系统界面语言）只影响人类可读文本；`--no-color` 被接受，JSON 从不含 ANSI 控制序列。
- `--debug`（CLI-14）：任何命令都接受，把诊断轨迹逐行写到 stderr，格式为 `[debug +<自启用起的毫秒>ms] <阶段>: <内容>`，包括阶段起止与耗时（`scan.walk: done in 12ms`）、所选扫描/执行后端及其理由、跳过与降级原因（候选的 skip_reason、过期项、每个动作的状态与代码）、外部进程的退出码与耗时（维护工具、卸载程序、适配器、提权 worker、AI CLI；退出码写成数字，没有退出码时为 `none`，如 `exit=0`）以及 HTTP 状态码。stdout 的机器记录与不带 `--debug` 时逐字节相同（request_id、时间戳等本来就每次不同的字段除外）；退出码、选择、授权与执行都不变。轨迹不含 AI 提供方密钥（只写 `key=present|absent`）、请求头、提示词或回执内容；路径按 Doctor 报告的规则显示（主目录为 `~`，用户名与计算机名为 `<user>`/`<host>`）。轨迹是给人排查用的文本，不是协议，调用方不得解析它；不带 `--debug` 时 stderr 不写任何轨迹。
- 只有 `--name` 形式的长选项；`-y`、`-f` 之类短选项一律是参数错误，不猜测含义。未知选项、缺值与重复选项都是 invalid_argument（退出 2），不忽略。
- `--limit` 默认 50、上限 500，超出报错；游标是不透明字符串，与 scan revision、排序、过滤和页大小绑定，换任何一项即 cursor_stale（退出 6）。

stdout 只写 UTF-8、无 BOM 的结构化数据；诊断写 stderr，日志不得泄漏凭据。所有机器记录包含 `schema_version: "1.0"`、`kind` 与 request_id。字节数为十进制字符串（未知为 null，不写 0），时间为 UTC RFC3339；路径同时给出平台原始编码（`raw_base64`）与显示值，执行不重新解析显示值。

JSON 模式只输出一个终态记录。JSONL 模式输出零个或多个 progress 记录及恰好一个终态记录；每条一行，字符串中的换行被 JSON 转义。进度未知时 total_units 为 null，不伪造百分比。进程被强制终止导致缺少终态时，调用者必须按不完整处理，再凭任务 ID 查询。

请求 ID 用于日志关联，不是授权凭据。写操作需额外持久化去重键，并绑定计划摘要；重复请求返回同一 Operation 的当前状态，不重新执行已完成的动作。

## 扫描生命周期与任务宿主

首版没有后台服务：扫描由发起它的进程承载（`scan`），或由 `scan --detach` 另起的一个 `cleanmanager` 进程承载；宿主进程退出，扫描就结束。其他进程通过存储查询和请求取消：

- `scan --format jsonl` 在扫描记录建立后立即输出第一条 `progress`（`task_id` 即 scan_id、`sequence: 0`），之后每秒至多四条，最后恰好一条 `scan_result`。调用方可以先拿到 ID，再从别的进程查询或取消。
- `scan show ID` 与 `scan wait ID`：先用宿主身份（PID + 进程创建时间，避免 PID 复用误判）核对宿主是否仍在；宿主已退出而扫描未到终态时，已提交的结果按 `interrupted`、`partial_scope` 发布，issues 含 `host_exited`，退出 5。宿主存活状态无法判定时从不结束任务。
- `scan cancel ID` 只记录请求并返回 `scan_cancel_receipt`（`accepted`、`state_at_receipt`）。它不是终态：扫描在下一个安全边界停止，期间显示 `stopping`，最终为 `cancelled`（退出 130），已确认的行保留且可分页查询。安全边界是每个目录之间以及每写入一批行之后（默认 4,096 行），所以只有一个巨大目录的扫描也会在列举途中停下，而不是列完才看取消。`accepted: true` 的回执一定以 `cancelled` 结束：即使后端恰好在收到请求后、再次检查前完成，发布时也按 `cancelled`、`partial_scope` 处理，从不发布为 `completed`。对已结束的扫描回执为 `already_terminal`、`accepted: false`；判定与发布在同一写事务中，两者不会交错。
- `scan wait ID --timeout S` 的超时只是等待预算：返回当时的 `scan_status`，退出 8（“稍后凭 ID 再查”），不取消、不回滚，扫描继续由宿主运行。
- `scan --root P --timeout S` 的超时是宿主自己的预算：宿主像其他进程一样为自己的扫描记录取消请求，在安全边界停止，结果为 `cancelled` 并带 `timeout` issue（说明“没有回滚”），退出 8。
- 终态 `scan_result` 的退出码对 `scan`、`scan show`、`scan wait` 一致：completed 0，partial/interrupted 5，cancelled 130，超时 8，其他按 issue 映射（permission_required 4、unsupported 7、否则 8）。
- 终态 `scan_result` 的 `logical_bytes` 是已确认行中已知部分之和；`failed` 的结果没有可用的小计，`logical_bytes` 与 `allocated_bytes` 一律为 null（未知），即使失败前已写入部分行，也从不写 "0"。其他终态只在已知小计确实为零时写 "0"（例如取消时只确认了一个空目录）。
- `scan_status`/`scan_result` 的计数与大小不随扫描规模变慢（NEW-01）：`entries_seen`、`skipped_ranges` 在扫描中逐批记下、发布时重新核定，根的去重汇总（已知/未知大小、跳过范围）在发布的同一事务中写入索引，查询只按键读取，不加载整棵树（150 万行的 `scan show` 由约 16 秒降到 0.1 秒内）。字段与取值不变；旧版本程序发布的扫描没有这些记录，首次查询时沿父索引遍历一次（结果相同）并补记根汇总。桌面端概览、空间分布图与选择汇总使用同一组记录。

扫描 issue 的 `code` 在两个平台含义一致（常量见 `cm_core::scan::issue`）。根被拒绝时扫描在写入任何行（包括根条目）之前以 `failed` 结束，`entries_seen` 为 0：

| code | 含义 | 结果与退出码 |
| --- | --- | --- |
| `not_a_directory` | 根存在但不是目录（普通文件、设备等）；从不把它记录为目录 | failed，退出 8 |
| `root_is_link` | 根本身是链接：符号链接（指向目录或文件、包括悬空链接）、junction、卷挂载点，或扫描器不进入的其他 reparse point。链接从不被跟随，message 提示直接扫描它指向的文件夹 | failed，退出 8 |
| `root_unavailable` | 根不存在、所在卷离线或无法打开 | failed，退出 8 |
| `permission_required` | 以当前权限读不了根（failed，退出 4）或某个子树（该子树记为跳过范围，结果 partial，退出 5）；从不自动提权 | 见左 |
| `own_store_skipped` | CleanManager 自己的数据目录：记录为 `own_store` 边界、不进入、不计大小 | 不影响终态 |
| `volume_changed` / `identity_changed` | 扫描中卷被替换（停止，partial）/ 目录在列出后被替换（该目录跳过） | partial，退出 5 |
| `host_exited` | 宿主进程在终态前退出（包括已写完所有行和自身报告、尚未发布时被结束：保留宿主的报告并补上此 issue） | interrupted，退出 5 |
| `timeout` | `scan --timeout` 的预算用尽，宿主为自己请求取消 | cancelled，退出 8 |

根判断基于已打开的根句柄（Windows 以 `FILE_FLAG_OPEN_REPARSE_POINT` 打开、macOS 以 `O_NOFOLLOW`），不是先查路径再打开，因此检查与扫描之间替换路径也不会被跟随。Windows 上云同步根目录本身带云 reparse 标记，是唯一按目录接受的 reparse point：只按元数据列出，其中的占位子目录仍记为 `cloud_provider` 边界。桌面端在提供和开始扫描前用同样的 code（`not_a_directory`、`root_is_link`）预先拒绝。

### 扫描之后的操作

扫描是一份快照：之后执行的计划移走的条目仍在扫描结果里。`entries list`（以及桌面端的磁盘表格、空间分布图与搜索，同一实现 `cm_app::removed`）给这些条目加上可选的 `removed` 事实，不重新读取磁盘：

- 来源只是审计库：扫描结束（`finished_at`；中断的扫描用最后一次心跳）之后**逐项成功**的动作——`plan/v1` 的每一项（规则清理、构建产物、安装包），以及 `plan/v2` 的文件动作 `fs_trash` / `fs_quarantine` / `fs_delete`（手动移到回收站、旧下载、卸载残留、清空回收站项）。`mode` 取动作结果的 `recovery`（`trash`、`quarantine`、`unavailable` → `permanent`），没有时按计划的处理方式；随内容一起移走的空文件夹按计划的处理方式。
- 隔离项被恢复计划放回（`restored`）后不再标记；被永久清空（`purged`）后为 `permanent`、`recoverable: false`。回收站中的项 CleanManager 看不到后来的状态，始终按 `recoverable: true` 报告；在系统回收站里还原后重新扫描即可。
- 路径按扫描根之下的各级名称落到这份快照的条目（Windows 不区分大小写），不在快照里的路径忽略；所以同一根的较新扫描（在这些操作之后结束）不带任何 `removed`。被移走的文件夹之下的条目带 `inherited: true`。
- 目录的 `removed_count` / `removed_bytes` / `removed_unknown_count` 只计其下最上层的已移走条目（文件夹里随它一起移走的不重复计），大小用快照里存下的大小：大小未知或部分未知的条目计入 `removed_unknown_count`，`removed_bytes` 是已知部分，全部未知时为 null，从不把未知写成 0。
- 计算只读审计库与少量目录的子项名称，按“扫描 + 审计修订”缓存（桌面端）；创建计划仍在执行前逐项核对对象是否还在（`precondition_missing`）。

### 扫描索引不可用与修复

扫描索引 `index.db` 是可重建的缓存，操作历史、隔离清单、授权与恢复都只在 `audit.db` 中。存储打开时只打开审计库；索引在命令第一次需要它时才打开，所以索引损坏或结构版本不可用时：

- 不需要索引的命令照常工作：`operations list|show|reconcile`、`recovery list|show`（`quarantine list`）、`recovery plan create` 与恢复/清空计划的 `plan apply`、`plan show|review|revoke`、`policy`、`protect` 等。
- 需要索引的命令（`scan`、`scan show|wait|cancel`、`entries list`、`analyze`、`candidates list`、`clean`、`trash`、v1 `plan create` 等）返回 `error`：`code: "index_unavailable"`（退出 8，不可重试），`message` 说明原因，`required_action` 为 `cleanmanager doctor --repair-index`（样例：[index-unavailable](../examples/contracts/index-unavailable.json)）。运行中 SQLite 报告损坏（`SQLITE_CORRUPT`/`SQLITE_NOTADB`）同样是这个错误，而不是 internal_error。
- `doctor` 总是对索引做完整的 `quick_check`，不可用时 `store.index` 为 `failed` 并给出修复命令；审计库与隔离计数照常报告。
- `doctor --repair-index` 先做同样的完整检查：索引正常或不存在时什么都不改（`index_repair.status: "not_needed"`）；不可用时把 `index.db` 及其 `-wal`/`-shm` 改名为 `index.db.damaged-<UTC 时间>`（`-wal`、`-shm`）留在存储目录（从不删除），新建空索引，并在 `migrations.log` 记一行 `rebuilt`。结果在 `doctor_report.index_repair`（`status: "repaired"`、`previous_state: corrupt|incompatible`、`kept_files`、`message`）。旧的扫描、候选集及其 ID 随之不可用，需重新扫描；引用旧扫描的计划在执行前核验时按原有规则拒绝。Windows 上另一进程（运行中的扫描宿主或桌面端）仍打开着索引时文件无法改名：返回 `conflict`（退出 6），已改名的文件全部改回，什么都不变；macOS 上请先关闭桌面端并等扫描结束。桌面端的 IPC 命令 `repair_index`（参数 `confirm: true`，由界面在用户确认后传入；缺少确认返回 `invalid_argument`/`confirmation_required`）调用同一实现（`cm_app::index_repair`），返回同样的 `index_repair` 对象；它先关闭本进程的全部索引连接，本进程正在扫描或执行操作时拒绝（`conflict`：`scan_running`/`operation_running`），其他进程占用索引时返回 `conflict`/`index_busy` 且什么都不变；成功或索引本就正常后，启动时记下的索引问题随之清除。
- 完整性检查不在每次打开时运行（PERF-WFA-01）：仅在从未检查过、上次通过已超过 7 天、存在“非正常结束”标记（扫描宿主未发布终态即退出，或运行中 SQLite 报告损坏）、即将迁移结构，以及 `doctor` 时运行；结果记在索引的 `meta` 表中。审计库保持每次打开都检查（它很小，而且损坏必须在写入之前阻止写入）。

端到端测试 `crates/cm-cli/tests/scan_lifecycle.rs` 在子进程中启动真实扫描（调试构建可用 `CM_TEST_SCAN_SLOW_MS` 放慢批次，发布构建忽略该变量），从其他进程查询、取消、等待，并强杀宿主后验证中断状态。

## 稳定退出码

| 退出码 | 分类 | 例子 |
| --- | --- | --- |
| 0 | 完整成功 | success：完成扫描或全部计划动作成功 |
| 2 | 参数或协议错误 | invalid_argument、schema_mismatch |
| 3 | 需用户授权 | needs_approval、policy_denied |
| 4 | 系统权限不足 | permission_required |
| 5 | 部分成功 | partial：部分目录不可读、扫描中断、部分动作失败或跳过 |
| 6 | 前置条件或状态已过期 | plan_stale、cursor_stale、conflict |
| 7 | 能力不支持 | unsupported |
| 8 | 可诊断的系统或内部失败，或需要核对 | io_error、internal_error、protection_unavailable、index_unavailable；operation_report 的 reconciliation_required；等待预算用尽 |
| 130 | 用户取消 | cancelled |

机器可读的同一张表见 [exit-codes.json](../schemas/v1/exit-codes.json)，Rust 测试与文档验证器分别与它比对。结构化 code 是调用方分支依据，message 只供人阅读。error 记录携带 retryable、details 和 required_action；自动重试不适用于未知副作用结果。部分扫描返回 scan_result，status 为 partial，退出 5，而不是只输出一个丢失扫描 ID 的 error。

## dry-run 即计划预览

`clean --dry-run`、`purge scan`（= `purge --dry-run`）与 `installer scan`（= `installer --dry-run`）返回 `selection_preview`：

- 与建计划使用同一分析、同一档位计算与同一实时占用探测；对同一扫描，非交互计划的对象集合等于预览中 `selected_by_default` 的对象集合（`crates/cm-cli/tests/dry_run_parity.rs`、`clean_flow.rs`）。
- 预览不写计划、授权票据或操作记录，不消耗任何授权；`convertible_to_plan` 为真时，预览的 `object_id` 可原样作为 `--candidates` 建计划。
- 建计划时再对每个选中对象做一次只读现场核对（只取元数据，不跟随对象本身）：已不存在（`missing`），或文件的大小、修改时间（整秒）已变（`changed`）；目录对象则逐个核对扫描时在其下观察到的文件（不进入链接与挂载点），其中任一文件消失或变化，整个目录记为 `changed`，`detail` 写明是哪个文件。这些对象不进入计划，而是列在 `plan` 记录的 `stale_items`（签名的 body 之外，不影响摘要），其余对象照常进入计划；被排除的对象随计划保存，之后的 `plan show` 与桌面计划审阅页都会列出它们及原因（R3-07）。现场核对先于实时占用探测：点名（`--candidates`）的对象已消失时列为 `missing`，不再因其探测结果为 unknown 而使整个请求以 policy_denied 失败；仍存在的点名对象若被占用，照旧以 policy_denied 拒绝。全部选中对象都已变化时返回 plan_stale（退出 6），先重新扫描。现场核对之前先做可计划判定：此刻不能计划的点名对象（例如已挂载的磁盘映像、被占用的安装包）以 policy_denied 拒绝并写明原因，不会因挂载中的映像一直在变而报告为 plan_stale（WFA-58）。计划从不包含零个动作：索引中没有任何文件的文件夹（例如被先前的清理清空）不成为候选，选中对象全都没有可处理的内容时以 policy_denied（`empty selection: …`）拒绝，不保存计划（WFA-113）。桌面的构建产物、安装包计划使用同一套核对（`cm_app::selection`）。执行器在每个动作前仍逐项核验身份与前置条件。
- trash 模式的计划（`clean`、`plan create`、`purge plan create`、`installer plan create` 与手选 `trash`）在保存前按卷检查回收站/废纸篓是否可用：卷上关闭了回收站（该卷设为“不将文件移到回收站”，或组策略“不将删除的文件移到回收站”）、网络卷或系统没有可用回收站时，整份计划以 unsupported（退出 7）拒绝，不保存任何计划；message 以 `trash_unavailable:` 开头并逐个列出受影响的卷与原因，`required_action` 给出下一步（改用 `--mode quarantine`，或为该卷打开回收站；手选 `trash` 只有后者）。从不改为永久删除，也不自动换成隔离模式。`trash inspect` 对这样的对象给出 `trashable: false`、`refusal.reason: trash_unavailable`。测试可用 `CM_TEST_BITBUCKET_ROOT`（`HKCU\Software\CleanManagerTest\` 下的键）额外提供每卷设置；它只会关闭回收站，不能打开。
- `candidates list` 与 `clean` 预览给出同一套解释（ANA-02，字段可选，旧记录没有）：`tier` 与 `tier_reason`（规则上限及再生成代价，随后原样列出每个降档或阻止代码）、`rebuild_class`、`regeneration`、`offline_effect`、`age_threshold`（天数与口径：`modified_time` 是修改时间，不是最后使用时间）、`recovery_limits`（每个可用模式的恢复途径与限制）；`candidates list` 另有 `category`、`evidence_refs`，其 `reason` 以规则自身的匹配依据开头（含年龄阈值与口径），不再只是 “matched rule”。规则包中已没有该规则版本时，规则派生字段省略。

## 非交互与授权边界

- 任何命令组都没有 `--yes`、`--force`、`--approve` 之类的批准参数；它们与任何未知选项一样被拒绝（退出 2）。批准只来自终端内的 `plan review`（输入摘要前 8 位）或用户事先在终端启用的有限策略。
- 非交互调用不扩大选择：不带 ID 的计划只含未跳过的 default_selected 对象；review_only、近期项目、依赖目录、个人数据与会话只能通过明确对象 ID 进入计划，并且仍需授权。按来源整批选择（`installer plan create --source`）等“扩大选择”的选项在参数表中登记，由统一检查在任何命令代码运行前拒绝非交互调用者。
- 参数表 `crates/cm-cli/src/args.rs` 的 `COMMANDS` 是全部命令组的唯一声明；单元测试比对 `commands.rs` 的分派与该表，检查没有任何自批准名称的选项、扩大选择的选项都被统一拒绝，并把本文、README 与 Skill 中的全部 `cleanmanager …` 示例按该表解析（注释含“草案”/draft 的示例除外）。新增命令须在该表登记。

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

长扫描与另一进程的查询：

```text
cleanmanager scan --root "D:\Data" --detach --json            # scan_status：后台宿主已开始
cleanmanager scan wait s_demo --timeout 60 --json             # scan_result；仍在运行则 scan_status + 退出 8
cleanmanager scan cancel s_demo --json                        # scan_cancel_receipt，不是终态
cleanmanager entries list --scan s_demo --kind file --limit 20 --json
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

`operation_report` 分开报告 `processed_logical_bytes`（处理量）、`quarantined_logical_bytes`（移入同卷隔离的量）、`estimated_freed_bytes`（仅永久删除/清空计入的逻辑估计；隔离、回收站与恢复为 0）以及卷空闲前后观测和有符号差值；差值按实测原样报告（可能为负），不归因于本工具。历史中的恢复入口如实给出：隔离为 `in_app_restore`，回收站为 `system_recycle_bin`（使用系统回收站恢复），永久删除与清空为 `unavailable`（无撤销），结果未确认为 `unknown`（先核对）。plan/v2 各种类按自身给出 `action` 与恢复入口（WFA-40）：手选移入回收站 `trash`/`system_recycle_bin`、回收站内永久删除 `delete`/`unavailable`；启动项 `startup`（记录的原状态未还原前为 `in_app_restore`，`startup restore`）；维护 `maintenance`（设置改动有备份时 `in_app_restore`，否则 `unavailable`）；结束进程 `process`/`unavailable`（结束的进程无法复原）；应用更新 `update`/`unavailable`（由包管理器安装，本工具不回滚）；卸载第一段 `uninstall`：厂商卸载程序或包管理器为 `unavailable`（只能重新安装，提示用 `apps uninstall leftovers` 审阅残留），macOS 应用包移到废纸篓为 `system_recycle_bin`，没有卸载程序完成为 `not_applicable`；卸载残留 `uninstall_leftovers`：注册表键与启动值的备份未还原前为 `in_app_restore`（`apps uninstall restore`，说明中一并给出回收站中文件的放回方法），只剩回收站中的文件时为 `system_recycle_bin`，已全部还原为 `not_applicable`；还原计划按放回的内容命名为 `startup_restore`、`uninstall_restore` 或 `maintenance_restore`（`not_applicable`），`restore`/`purge` 只用于隔离区恢复（recovery/v1）。宿主在报告前退出、尚未核对（`reconciliation_required`）的操作，`operations list` 仍给出按日志得到的 `authorization_source`（审阅或策略回执）与 `counts`，恢复入口为 `unknown` 并提示先运行 `operations reconcile`。端到端测试 `crates/cm-cli/tests/recovery_flow.rs` 在真实 NTFS 上覆盖以上正常与拒绝路径；macOS 恢复动作尚未验证，当前只返回 unsupported。

`add-mole-parity-suite`：`clean` 已在 Windows 实现（`clean list` 返回 [cleanup-catalog](../examples/contracts/cleanup-catalog.json) 形状）；`apps uninstall` 已在 Windows 实现为两段计划（见 [应用卸载](app-uninstall.md)）：

```text
cleanmanager clean list --json                                       # cleanup_catalog：类目状态、规则登记覆盖、适配器预览
cleanmanager clean --scan s_demo --dry-run --non-interactive --json  # selection_preview：档位、跳过原因、探测结果，不耗授权
cleanmanager clean --scan s_demo --mode trash --non-interactive --json  # plan：只含未跳过的 default_selected；无可选项为 policy_denied
cleanmanager apps uninstall plan create --instance app_demo --non-interactive --json  # plan（uninstall）：app_quit + tool_adapter，需审阅
cleanmanager apps uninstall leftovers --operation op_demo --json      # uninstall_leftovers：分组残留，仅缓存/日志默认选中
```

这两条命令的机器请求与响应分别对应样例 [request-clean-preview](../examples/contracts/request-clean-preview.json)、[preview-clean](../examples/contracts/preview-clean.json)、[request-apps-uninstall](../examples/contracts/request-apps-uninstall.json) 与 [needs-approval-uninstall](../examples/contracts/needs-approval-uninstall.json)；从预览建计划且有变化项时见 [plan-from-preview-stale](../examples/contracts/plan-from-preview-stale.json)。

批量卸载（UNI-11）：`apps uninstall plan create --instance A --instance B` 生成**一份** `uninstall` 计划，每个应用各有自己的 `app_quit` 与 `tool_adapter`（第一个应用的动作 ID 与单独卸载相同，为 `q0000000`/`u0000001`；第 n 个为 `u{n:03}0000`/`u{n:03}0001`，动作仍按 `action_id` 排序）。计划记录附加 `uninstall_apps`（每个应用的 `action_ids` 与估计）和 `uninstall_refused`（与单独卸载时一样被拒绝的应用，不进入计划）；`plan/v2` 的正文、摘要与种类规则不变，单应用计划仍完全有效。执行时某个应用的卸载程序只等待它自己的退出请求；一个应用失败、被跳过或登记已变化不阻止其他应用，`operation_report` 附加逐应用的 `uninstall_apps`（`status`：completed/partial/failed/skipped/not_started/unknown，`leftovers_reviewable`），`operations show` 同样给出。之后 `apps uninstall leftovers` 的 `apps` 按应用分组；卸载程序没有成功的应用不列出任何对象，残留计划也从不包含它们。样例：[request-apps-uninstall-batch](../examples/contracts/request-apps-uninstall-batch.json)、[plan-v2-uninstall-batch](../examples/contracts/plan-v2-uninstall-batch.json)、[operation-uninstall-batch](../examples/contracts/operation-uninstall-batch.json)、[uninstall-leftovers-batch](../examples/contracts/uninstall-leftovers-batch.json)。

两个平台使用同一参数风格。路径由 shell 按本平台正确传入，内部立即转成原始路径结构；不把模型拼出的字符串当 shell 脚本执行。输出中的条目 ID 是后续选择的主要接口，避免再次拼接路径。

`plan apply` 无授权时返回 needs_approval，并保持目标文件未改动。已预授权的策略覆盖该计划时可以直接执行，不要求无意义的重复确认。管理员权限与授权状态分别返回；模型不得自行提高风险上限来通过拒绝。

授权、保护与核对的可查询事实（WFA-34/40/42/43、PRT-02）：

- **策略为何适用或不适用**：清理（plan/v1）计划的 `plan` 记录附可选的 `authorized_by`（授权或曾授权这份计划的回执：`source` 为 `interactive_review` 或 `policy`，策略时带 `policy_id` 与 `policy_version`；计划用过并过期后仍给出，撤销的不算）；授权状态为 `required`/`revoked` 时另附 `policy_matches`：每条已存策略的 `policy_id`、`policy_version`、`state`、`covers`（此刻执行时会授权它；配额只在执行时原子预留）、`reason`（不适用的条件：`not_enabled`、`expired`、`other_principal`、`scope_differs`、`mode_differs`、`rule_not_covered`、`tier_not_default_selected`、`size_unknown`、`single_limit`、`quota_exceeded`）与写明双方取值的 `message`。清理计划的 needs_approval 在 `message` 中说明是否已审阅及每条策略不适用的原因，并在 `details.policy_matches` 中给出同样结构；配额不足的 policy_denied 写明 24 小时窗口的已用量、上限与本计划所需量。plan/v2 计划的 needs_approval 写明策略从不适用于该种类。样例：[plan-policy-matches](../examples/contracts/plan-policy-matches.json)、[plan-policy-authorized](../examples/contracts/plan-policy-authorized.json)、[needs-approval-policy-mismatch](../examples/contracts/needs-approval-policy-mismatch.json)。
- **保护名单不可用时什么都不开始**（PRT-02）：`plan apply` 在核验计划之后、使用任何授权（策略配额）和登记操作之前检查 `protection.json`；不可用时返回 protection_unavailable（退出 8，`required_action` 指向该文件），不登记操作、不消耗计划或回执，修复后同一份已批准计划在有效期内照常执行。`plan create`、`clean`、`trash`、`purge plan create` 与 `installer plan create` 同样拒绝建计划；桌面执行入口与执行器使用同一检查。执行中某对象已不在原位置（例如被另一份计划处理过）时报告 `precondition_missing`，而不是 `protected`。
- **中断的操作**：宿主进程在报告前退出的操作，`operations show` 返回按日志生成的临时 `operation_report`：`status` 为 `reconciliation_required`（退出 8），有意图无结果的动作为 `unknown`/`outcome_unconfirmed`（消息提示运行 `operations reconcile`），没有意图的为 `not_started`/`never_started`，`finished_at` 为日志最后一次记录的时间；它只读，不判定也不写入任何结果。
- **核对保留授权来源**：`operations reconcile` 只判定缺终态的动作，`authorization_source`（`interactive_review` 或 `policy:ID@版本`）保持原值；核对作为独立事实记入 `operation_report` 与 `operations list` 的可选字段 `reconciled_at`。`operation_report`（`operations show`、`plan apply`、`operations reconcile` 的输出）总是带 `authorization_source`，与 `operations list` 同一操作的值相同；宿主中断、尚未核对的操作由日志生成的临时报告（`reconciliation_required`）也带记录中的来源。没有待判定动作时返回原报告不作改动。样例：[operation-reconciled](../examples/contracts/operation-reconciled.json)。

计划有效期建议首版默认 15 分钟，这是设计默认值，需要实测与使用反馈；执行前仍逐项核验，时间未过期并不证明对象未变化。授权票据与可导出的计划分开保存，JSON 中的授权状态只是查询结果，不能当外部可写凭证。

## 产品工具：补全、Doctor 与语言

- **Shell 补全**（LIF-03）：`completion print bash|zsh|fish|powershell` 由参数表生成（命令词逐级补全、各命令的选项、`--format`/`--mode`/`--sort` 等的取值、路径选项补全文件；内部选项不出现）。`--format text` 输出裸脚本，例如 `eval "$(cleanmanager completion print bash --format text)"`。`completion install SHELL [--profile PATH]` 先显示将追加的带标记的加载行和目标文件；JSON、非交互或无终端时只返回 `action: preview`、`written: false`，文件不被创建或修改；只有终端内输入 `append` 才追加（`action: installed`），已安装时为 `already_installed`。PowerShell 默认配置文件是 `Documents\PowerShell\Microsoft.PowerShell_profile.ps1`，文档目录被重定向时传 `--profile $PROFILE`。
- **Doctor**（LIF-04）：`doctor` 只读，报告版本、平台、能力矩阵、权限上下文（是否启用了绕过 ACL 的特权）、配置有效性（规则包与实测登记、保护名单、构建产物设置）、完整错误码与退出码表、操作历史中最近的非成功动作代码及存储自检（审计库与索引各自的 SQLite quick_check 与结构版本——`store.open` 与 `store.index` 分开报告、隔离与待核对操作计数）；不读取文件内容、环境变量或凭据，没有存储目录时不创建。唯一的写入是显式的 `--repair-index`（见“扫描索引不可用与修复”）。默认脱敏：用户主目录替换为 `~`，用户名与计算机名（含 Windows 8.3 短名形式）替换为 `<user>`/`<host>`；`--no-redact` 关闭。`--export PATH` 在 JSON/非交互时只返回带 `export.written: false` 的预览；终端内显示完整报告并输入 `export` 后才以新文件写出（已存在则拒绝覆盖）。
- **自更新**（LIF-01）：`self update --check` 与 `self update` 只在人运行时访问更新通道（构建时嵌入的 `CM_UPDATE_MANIFEST_URL`，或 `--manifest` 给出的 HTTPS 地址/本地文件）。信封的 Ed25519 签名必须由构建时嵌入的发布公钥验证；随后核对产品、目标平台、版本（只升不降）与存储结构兼容，逐文件比对大小与 SHA-256（内存与落盘各一次），试运行新程序（`capabilities` 报告的版本须等于清单版本），最后逐个顶层条目改名替换并保留旧版本于 `.cleanmanager-update-backup\<版本>`；任何一步失败都不改动当前版本（`verification_failed`/`incompatible`），替换中失败则全部改回（`rolled_back`；改回本身失败为 `rollback_incomplete` 并给出备份位置）。winget、Scoop、npm、cargo、Homebrew 安装只返回 `use_package_manager` 与 `manager_command`，从不访问网络或改动文件；没有发布清单的开发构建为 `cannot_check`。退出码：成功/最新/可更新/包管理器 0，校验或兼容失败 3，被锁 6，无通道/无密钥/非发行布局 7，下载失败与回滚 8。
- **单实例锁**：`<store>\lifecycle.lock` 是操作系统文件锁（进程退出即释放）。`self update`/`self install`/确认后的 `self uninstall` 独占持有；其他所有写命令（参数表中 `writes` 的命令）在整个运行期间共享持有，因此更新不会在写操作进行中替换程序或迁移存储，更新期间新的写命令立即以 `conflict` 失败。
- **安装与自卸载**（MVP 14.1、LIF-02）：见 [安装与更新](install.md)。`self install` 与 `self uninstall` 在 JSON、非交互或无终端时只返回预览；`self uninstall` 默认保留配置（含 `features.json`、`resident.json`）、保护名单、操作历史（含注册表/启动项备份与 `diagnoses\` 中保存的 AI 诊断）与隔离区，存储中本工具自己写入的每个文件与文件夹都按类别识别（索引、上次发现记录、被修复挪开的损坏索引与中断写入留下的 `.tmp` 为缓存），只有不认识的条目才注明保留，隔离区非空时给出数量、大小与恢复/清空命令，`--remove-user-data` 只在隔离区为空且无待核对操作时生效。
- **语言**（LIF-05）：人类可读文本（`--format text` 的标题与说明、提示、`help`）提供简体中文与英文；kind、code、status/state、字段名与所有取值不本地化，机器 JSON 在任何语言下逐字节相同（`crates/cm-cli/tests/cli_contract.rs` 比较 en、zh-CN 与环境变量三种设置）。error 的 `message` 始终是英文诊断文本。桌面界面同样提供简体中文与英文（默认跟随系统语言，可在 设置 › 界面语言 中切换，立即生效），文字来自 `apps/desktop/src/i18n` 的两份资源，见 [视觉标识 › 界面文字与语言](visual-identity.md#界面文字与语言lif-05)。

## 可选扩展：AI 诊断

`diag config|preview|create|show|list` 属于可选的 AI 诊断扩展，使用独立契约 `diag/1`（[schemas/diag/v1](../schemas/diag/v1/diagnosis.schema.json)），不扩充 v1 的记录种类与 `cli_request` 命令表；错误沿用 v1 `error` 记录与退出码。它只解释对象：没有提供方时立即返回 `unsupported`，`create` 必须带上 `preview` 给出的载荷摘要（例如 `cleanmanager diag preview --scan s_1 --entry 42`），不创建计划、不批准、不输出 shell 清理命令。详见 [AI 诊断扩展：实现与契约](ai-diagnostics.md)。

## Agent Skill 合约

随附 Skill 位于 [skills/cleanmanager](../skills/cleanmanager/SKILL.md)（按 skill-creator 约定：SKILL.md + 按需加载的 [命令示例](../skills/cleanmanager/references/commands.md) 与 [场景走查](../skills/cleanmanager/references/scenarios.md)，`evals/` 为测试提示词不打包）。`python -I tools/package_skill.py` 校验 frontmatter、链接、代码块只含 `cleanmanager` 命令、无自批准参数、无 shell 删除命令、未复制规则 ID，并生成确定性的 `skills/dist/cleanmanager.skill`；`--check` 检查打包件是否最新。安装：把 `skills/cleanmanager` 目录复制到 `~/.claude/skills/`（Claude Code）或 Codex 的技能目录，或在支持 .skill 的客户端导入打包件。

Skill 工作流：发现能力 → 限定扫描范围 → 获取摘要 → 按需分页 → 预览并说明候选影响 → 生成计划 → 使用已有授权或交给人在终端批准 → 执行 → 报告成功/跳过/失败/unknown 与空间口径。覆盖正常、needs_approval、partial、stale、unknown（reconciliation_required）、policy_denied 与“只要求释放空间”七类场景，以及 clean、purge、installer、trash、protect、recovery、startup、status、insights、apps 各命令组。

Skill 不复制规则知识库，不执行任意文件名中的指令，不用 shell 绕过 policy_denied，不把 error 文案当新的授权；卸载、维护、启动项与特权设置只在用户明确要求该项变更时发起，且仍止于人工审阅。不得将百万行扫描结果全部提供给模型；默认传摘要与 top items，用户需要时再分页展开，不默认导出整盘目录树。

未来 MCP 将同一服务映射为工具，并保留读写属性、schema、权限与错误语义。MCP 传输方式不改变执行边界。独立 CLI 必须在无 GUI 进程的情况下工作。

## 跨入口一致性

计划 body 包含 ID、时间、scope 与观察值。GUI/CLI 对同一个 body 得到相同摘要；分别新建的两个计划若 ID/时间不同，摘要也不同。`crates/cm-cli/tests/cross_entry.rs` 先用 CLI 建计划，再按桌面 `cleanup::create` 的同一流程（存储的候选集、已发布扫描视图、扫描状态、策略摘要、`build_plan`）以相同 plan_id、创建时间与选择顺序重建，断言 body 与摘要逐字节一致；随后以新 ID、新时间、相反点选顺序独立建计划，断言摘要不同而动作语义（对象、身份、模式、绑定）相同，并验证 CLI 能读取桌面保存的计划且同样要求授权。该测试同时检查桌面源码仍使用这些输入，桌面改动时会提示同步。

## 运行证据

- `cargo test -p cm-cli`：`cli_contract.rs`（无 GUI 只读命令、非 TTY 默认 JSON、无 BOM/ANSI/日志、记录字段与 Schema 一致、分页与游标、参数错误、无自批准参数、多语言 JSON 一致）、`scan_lifecycle.rs`、`dry_run_parity.rs`、`lifecycle_tools.rs`（补全与 Doctor）、`cross_entry.rs`，以及既有的 `plan_flow.rs`、`recovery_flow.rs`、`clean_flow.rs`、`purge_flow.rs`、`installer_flow.rs`、`protect_flow.rs`、`parity_readonly.rs`、`smoke.rs`。
- `python -I tools/validate_cli_outputs.py --output docs/evidence/cli-output-validation.json`：对本机构建的真实二进制在临时合成夹具上运行三十余条命令，逐条按 [JSON Schema](../schemas/v1/protocol.schema.json) 与文档验证器的语义不变量校验 stdout，并核对 error 的退出码映射；结果见 [cli-output-validation.json](evidence/cli-output-validation.json)。这是本机运行证据，不代表其他平台或架构。

## 冻结边界

本节登记首版机器协议从“草案”到“冻结”的边界，避免后续实现暗改已承诺的结构：

- **草案（当前）：** `schemas/v1` 中所有记录与 [contracts 样例](../examples/contracts/README.md)、[CM-C14N-v1 黄金向量](../examples/c14n/golden.json) 与 [CM-OBS-v1 样例](../examples/c14n/observation-v1.json)。草案期间可以在 `schema_version: "1.0"` 内补齐字段与枚举值，但每次修改必须同步生成器、样例、负例与 Rust 测试；验证器会拒绝缺 fixture 的记录、文档与 Schema 不一致的命令表及退出码表。
- **冻结条件：** G1 核心/CLI 闭环通过后由契约评审冻结：表中命令与响应记录、错误码与退出映射、`plan/v1`、`plan/v2`、`recovery/v1` 的 body 与摘要规则、CM-OBS-v1 字段集。
- **冻结之后：** 严格对象 Schema 继续拒绝未知字段；任何新增或改名字段、新枚举值都需要新的 `schema_version` 与协商，旧版本记录仍可读取。计划摘要规则（CM-C14N-v1）不得在同一版本内改变。
- **不在冻结范围：** 人类可读文本、`message` 文案、`display` 路径显示、草案中标注为 `add-mole-parity-suite` 的工具命令组（随其 G1–G4 门禁分别冻结）。

## 版本与验证

目前 Schema 覆盖 capabilities、scan_status、scan_result、scan_cancel_receipt、entries_page、analysis_result、candidate、candidates_page、plan（plan/v1、plan/v2、recovery/v1，可选 stale_items）、authorization_status、operation_status、operation_report、operation_cancel_receipt、quarantine_list、error、progress，以及工具命令组的 cli_request、selection_preview、app_list、startup_list、maintenance_catalog、maintenance_report、status_snapshot、protection_list、insight_list、doctor_report、purge_roots、installer_locations、release_info、self_update_result、self_remove_preview、completion_script。分页包装、candidate_set 结果（`analysis_result`）、运行态查询（`scan_status`/`operation_status`）、扫描与操作取消回执、授权查询和隔离清单已补入草案。首版不提供后台任务服务：宿主进程退出后扫描为 `interrupted`，未确认动作进入 `reconciliation_required`。`operations cancel` 不涵盖 scan。需要待核对的操作不得自动重试；以上结构在“冻结边界”一节的条件满足前不对外承诺。

跨入口行为比较应固定 ID、时间与选择顺序，或比较选择和动作的语义，不能要求不同 body 得到相同 hash（见“跨入口一致性”）。

v1 草案采用严格对象 schema，拒绝未定义字段。未来增加字段需要协商新的 schema_version；不要一边严格拒绝未知字段，一边宣称所有新增字段完全兼容。二进制版本、数据库版本、规则版本与协议版本分别维护。

Schema 校验只能证明形状，不能证明 OS 对象存在、授权真实性或删除安全。仓库验证器额外检查摘要、字节范围、时间顺序、raw path 编码和汇总一致性；真正执行语义仍由平台测试验证。运行方法见 [文档索引](README.md)。
