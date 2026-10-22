# Rust 核心与数据模型

状态：实现前设计基线。本文规定模块边界和数据语义，不代表已有产品实现。入口与阅读顺序见 [文档索引](README.md)。

## 模块与依赖

`cm-core` 包含领域对象、扫描编排、规则、计划和执行状态机；`cm-store` 负责 SQLite 持久化（可重建的扫描索引与独立的审计库），使核心不携带 C 依赖；它不依赖 Tauri、终端颜色库或特定 Agent。`cm-platform` 实现系统访问接口，`cm-cli` 和桌面端分别负责输入输出转换。后续可在性能或发布边界明确后将平台模块拆成 Windows、macOS 两个 crate。

| 模块 | 输入与输出 | 不承担的职责 |
| --- | --- | --- |
| ScanCoordinator | Scope 与 ScanOptions → ScanSession、批量条目、覆盖率 | 清理判断与写入用户文件 |
| ScannerBackend | 根对象与权限上下文 → 元数据流、范围错误 | UI 排版、把访问失败换成 0 B |
| IndexStore | 批量观察值 → 一致的索引 revision 与游标 | 声称扫描期间文件系统静止 |
| Analyzer | 指定 revision、规则集 → CandidateSet | 授权和删除 |
| Planner | 候选选择、模式 → 不可变 PlanBody | 接受任意 shell 或临时扩大范围 |
| Authorizer | 计划摘要、用户选择或策略 → AuthorizationReceipt | 以 root 或管理员身份代替批准 |
| Executor | Plan、授权、实时核验 → Operation 与逐项日志 | 把扫描快照当作当前状态 |
| ApplicationAdapter | 已识别的软件身份 → 清单或受限维护操作 | 根据软件名称猜测可删除目录 |
| CapabilityRegistry | 平台、文件系统、权限、版本 → 能力与原因 | 用统一布尔值掩盖所有限制 |

接口建议使用 Rust trait，但不要为每个动作建立动态插件。初期静态注册后端与应用适配器，通过枚举区分操作。异步任务调度与阻塞文件系统 I/O 分开；线程数由后端、存储介质和实测调整，不能为每个文件产生一个无界任务。

## 核心实体

| 实体 | 最小字段 | 不变量 |
| --- | --- | --- |
| Scope | id、平台、主体、根目录原始表示、卷身份、跨卷策略 | 路径字符串不等于授权范围 |
| ScanSession | id、scope、backend、revision、状态、覆盖率、起止时间 | revision 发布后供查询使用，不原地改写快照语义 |
| FileObject | 卷身份、文件 ID、代次信息、类型、大小、属性 | 文件实体与目录项分离，文件 ID 可能重用 |
| DirectoryEntry | entry_id、parent_entry_id、name_raw、object_id | 同一对象可以有多个硬链接目录项 |
| Observation | 时间、来源、字段有效性、错误 | 未知、无权限、未扫描不等于零 |
| Candidate | scan revision、entry、rule id/version、理由、影响、风险 | 是建议，不是删除授权 |
| CandidateSet | 规则集摘要、scan revision、候选 ID 集 | 父子包含和重叠消除要跨规则进行 |
| PlanBody | scope、固定对象列表、操作模式、规则/策略摘要、有效期 | 内容不可变；修改内容产生新计划和摘要 |
| AuthorizationReceipt | plan digest、主体、授权来源、有效期、撤销状态 | 与 PlanBody 分离；摘要自身不证明授权 |
| Operation | id、plan digest、请求去重键、状态、时间、汇总 | 同一计划不可并发或隐式重复执行 |
| ActionJournal | action id、意图、前置状态、执行结果、恢复信息 | 逐项持久化；整批文件操作不是数据库事务 |

跨进程使用数据库事务与 OS 锁协调写操作。锁键涵盖卷及路径重叠，不能只锁 plan_id，因为不同计划可能引用同一文件。进程消失后不得只因锁超时就重跑有副作用动作，先进入恢复核对。

## 字节数与路径表示

协议中的字节数使用无符号十进制字符串，上限为 u64；前端按 BigInt 或精确十进制处理。JSON number 会在 JavaScript 中丢失大整数精度，因此不用于磁盘字节计数。观测到的空闲变化可以是带负号的十进制字符串；不允许 NaN 或 Infinity。

路径同时保存原始编码与显示文本：Windows 为 UTF-16LE 的 Base64，macOS 为原始路径字节的 Base64。执行只使用经验证的原始值与对象身份，不能把 UI 显示文本重新解析成执行路径。拒绝 NUL；Windows 输入必须是合法长度的 UTF-16 code units 序列，是否包含不配对代理项由原生 API 路径语义处理，不强制用 Unicode 字符串重写。

大小写比较依照实际卷与目录语义；不能把全平台路径统一转小写。不要对磁盘上的名字做 Unicode 归一化再执行操作。对外脱敏显示不能更改内部目标身份。

`logical_bytes`、`allocated_bytes` 和 `reclaimable_estimate` 各自记录来源。共享块未知时预计回收量允许 null。只有完整已知的同口径值才能汇总为确切数字，否则返回已知部分和 unknown_count。目录树可以显示每个链接位置，但全卷物理统计必须避免重复计数，并解释采用的归属规则。

## 持久化设计

建议 SQLite 表：`scopes`、`scan_sessions`、`file_objects`、`directory_entries`、`observations`、`scan_errors`、`candidate_sets`、`candidates`、`plans`、`authorizations`、`operations`、`action_journal`、`incremental_checkpoints`、`rule_sets`。索引键至少覆盖 `(scan_id,parent_entry_id)`、`(volume_id,file_id)`、`(operation_id,action_id)` 与 plan digest。

扫描批量提交，先写 staging revision，结束或取消后发布对应完整性。实时进度可独立发送，但查询结果必须携带 revision，避免分页时混入不同版本。增量索引与日志检查点同一事务提交；检查点失效时重建而非静默跳过。

目录排序在核心执行，分页游标绑定 scope、revision、排序、过滤条件和最后排序键。revision 过期返回 `cursor_stale`。不对百万条目使用全路径字符串排序来实现每次分页；保存层级与紧凑名称池，按访问需求构造路径。

缓存数据库损坏时可以丢弃可再生索引并重扫，不能顺带丢弃未完成操作的审计日志。

已实现的存储边界（`cm-store`）：

| 数据 | 文件 | 损坏或版本不兼容时 |
| --- | --- | --- |
| 扫描、revision、实体、目录项、跳过范围、子树汇总 | `index.db` | 首次需要时才打开（`Stores::open` 只打开审计库，新存储才立即建空索引），因此索引不可用时历史、隔离与恢复照常可用；需要索引的调用报告 `IndexCorrupt` / `IndexIncompatible`（协议码 `index_unavailable`，`required_action` 为 `cleanmanager doctor --repair-index`），运行中 SQLite 报告的损坏同样归为 `IndexCorrupt`；不自动重置。`repair_index`/`rebuild_index` 把 `index.db` 及其 WAL/SHM 改名为 `index.db.damaged-<时间>` 保留（从不删除）并新建空索引，然后需重新扫描。完整性检查按需运行（从未检查、超过 7 天、非正常结束标记、迁移前、`doctor`），见 `cm-store/src/integrity.rs` |
| 操作、去重键、逐项意图/结果日志（后续扩展授权与隔离清单） | `audit.db` | 随存储立即打开，每次打开都做 SQLite quick_check；报告 `AuditUnavailable` 并阻止写操作，**从不**删除或重建；保留原文件供修复 |

宿主进程以 PID + 进程创建时间标识。宿主确认退出后：运行中的扫描以 `interrupted` 发布已提交批次（`partial_scope`），未结束的操作进入 `reconciliation_required`；无法判断存活（如权限不足）时保持原状态，不推断结束。存储目录不得位于任何扫描根内。操作日志与恢复记录需要独立的可靠保存策略。存储路径按系统用户数据目录解析，数据库限制为当前用户访问；日志默认不包含文件内容或凭据。

## 生命周期与取消

扫描：`created → running → completed | partial | cancelled | failed`。`partial` 是有限范围内的部分成功，不承诺全盘完整；错误数不是“未扫描文件数”的估计，因为被拒绝目录的子项数量也未知。

操作：`created → validating → running → completed | partial | cancelled | failed | reconciliation_required`。中断发生于 OS 操作与结果持久化之间时，必须标记待核对。移动目标或删除标记可能已经生效，不能凭没有成功日志认定没执行。

取消只在安全边界停止调度后续动作，不承诺回滚已完成的动作。阻塞的系统调用可以有不可立即取消的区间，UI 显示“正在停止”。预期停止延迟在阶段 0 实测，不能在无数据时保证固定毫秒数。

## 应用服务接口

建议首批入口：`capabilities`、`start_scan`、`query_scan`、`list_entries`、`analyze`、`list_candidates`、`create_plan`、`get_plan`、`apply_plan`、`get_operation`。接口返回类型化错误，CLI/GUI 根据同一错误码本地化文本。

`AppInventory`、`MaintenanceOperation` 与 `HardwareCapability` 作为独立服务扩展。软件卸载或包管理器维护可能改变调用时未完全枚举的文件，因此使用单独的 operation 类型与预览语义，不能混进“精确文件清单删除”的能力承诺。

原生 API 事实与限制见 [Windows 设计](platforms/windows.md) 和 [macOS 设计](platforms/macos.md)；机器格式见 [CLI 协议](cli-protocol.md)。
