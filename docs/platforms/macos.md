# macOS 平台设计

日期：2026-10-14，状态更新 2026-10-17。状态：**部分实现，尚无真实 Mac 验证结果**。已实现并在 GitHub 托管 macOS arm64 runner 上通过测试：`mac.posix`/`mac.bulk` 扫描（含 `shared_blocks`、卷/根替换检测）、文件动作（隔离、删除、空目录、废纸篓，带描述符校验的父链与可信命名空间门槛）、隔离恢复与清除、清理目录解析与第 1 波规则（全部 experimental、仅审阅）、应用清单、手选废纸篓、空间洞察、构建产物与安装包清理。状态、启动项、维护、更新、卸载与常驻工具尚未实现；特权 helper、风扇/电池、Touch ID 与公证需要签名与真机，本构建不提供。所有 macOS 能力都报告为 `unverified` 或 `unsupported`，从不为 `available`；托管 runner 不是参考设备，真机验证见 [macOS 真机验证指南](macos-verification.md)。下文是设计说明，实现与之不一致处以代码和测试为准。

本文细化 [架构分析](../architecture-analysis.zh-CN.md)，与 [Windows 设计](windows.md) 共享领域模型，测试与阶段 0 基准统一见 [验证计划](../verification-plan.md)。竞品研究沿用现有架构文档；本文不重新研究 Mole/WizTree 的内部实现。

**官方行为**表示所链 Apple、Apple 开源项目、Homebrew 或 Tauri 一手资料所描述的行为；**拟议设计**表示本项目的实现选择；**待验证**表示需要指定版本、设备和签名身份的实测。Apple 归档指南用于解释机制，不能替代目标 SDK/系统验证；开源 XNU `main` 用于定位接口语义，落地时必须记录实际 SDK 和对应源码版本。现有 [REA bundle](../evidence/rea-wiztree-4.33.bundle.json) 只包含 Windows WizTree portable 清单/资源等静态证据，[native provider 不可用记录](../evidence/rea-native-provider-unavailable.json) 也已保存；不能外推为 macOS 调用路径、运行或性能证据。

## 0. 与核心和 v1 协议的映射

对外以 [核心架构](../core-architecture.md)、[安全规则](../safety-and-rules.md)、[CLI 协议](../cli-protocol.md) 和 [v1 Schema](../../schemas/v1/protocol.schema.json) 为准；本文新增的原生属性是 Observation/内部诊断设计，不是对严格 Schema 的隐式扩展。

- **计划默认 TTL 为 15 分钟（900 秒）**，到期返回 `plan_stale`、退出 6，重试或重启不能续期。未到期也必须核验当前身份、规则和授权；授权回执与计划内容分开。
- FSEvents 的 `current / reconciling / resync_required` 是内部索引状态；scan 终态只用 `completed / partial / cancelled / failed`，完整性只用 `verified_scope / partial_scope`。事件丢失、权限或云边界未覆盖时为 partial、退出 5；取消过程用 `progress.phase=stopping`，终态退出 130。
- macOS 原始路径使用 `unix_bytes_base64`，byte count 使用十进制字符串。内部未知值不伪造为 0；v1 scan 的 `logical_bytes` 是已知部分合计，与 `unknown_size_count`、`skipped_ranges`、`partial_scope` 一起解释；未扫描子树数量不能冒充未知文件数量。逻辑量未知的对象暂不成为要求大小字段的可执行候选。
- `reclaimable_estimate` 对应 candidate 的 `reclaimable_estimate_bytes`；共享块未知对应 `estimate_basis=shared_blocks_unknown`。空闲差值映射 `observed_free_delta_bytes`，允许负数字符串。fork、allocation source、dirty 集合等保存在内部，不直接添加顶层字段。
- `race_safety_unavailable` 等细分原因可用于 `issues` / action code 和内部日志；若整个请求失败，顶层仅返回 `unsupported`、退出 7，用 message 解释。helper 版本不兼容映射 `schema_mismatch`、退出 2；不在严格 `error.details` 中添加新键。
- `(volume_uuid, file_id)` 等映射 `identity` 的现有字段，generation 不可得时为 null，不编造代次。父身份和元数据前置条件保存在 Scope/Observation 并绑定元数据指纹；指纹不要求读取文件内容。执行遵循 `CM-C14N-v1` 计划摘要及 durable intent/result 协议，不能把辅助 fd/锁当作对外安全承诺。
- helper、原生索引与后续 MaintenanceOperation 可采用独立内部协议。`apps uninstall`、恢复、登录项修改尚不是已冻结的 v1 命令，相关测试为后续能力门槛。

## 1. 平台范围与后端选择

**拟议设计：**桌面首版采用站外 Developer ID 分发；不把 Mac App Store 作为首版分发目标。优先 Apple Silicon 真机，Intel 单独列入验证矩阵，未验证前不宣称支持。平台代码以 macOS 13 引入的 `SMAppService` 为相关能力下限，但**实际产品最低版本尚未冻结**，需要结合届时仍受支持的系统、Tauri/Rust 依赖及可用测试机器决定；SDK API 的最低版本不是产品支持承诺。

| 条件 | 拟议 backend ID | 实现 | 完整性与回退 |
| --- | --- | --- | --- |
| 本地 APFS 已授权目录 | `mac.bulk` | 目录 fd 上 `getattrlistbulk` 分批取元数据 | 请求字段未返回时单项未知或安全补查；不读原始 APFS |
| 本地 HFS+、外接文件系统 | `mac.bulk` 能力探测后启用，否则 `mac.posix` | `readdir` + `fstatat(..., AT_SYMLINK_NOFOLLOW)` | 字段、ID、大小口径不强行视为 APFS |
| 网络目录、特殊 provider | `mac.posix.remote` 或跳过 | 低并发、预算、断线/取消状态 | 不承诺 FSEvents 覆盖服务器全部变化 |
| 有有效索引的本地卷 | `mac.fsevents_index` | 事件标 dirty，批量枚举重建 dirty 子树 | 事件不是新大小数据，失效按第 4 节处理 |
| 未验证云同步根、dataless 目录 | `mac.cloud_skipped` | 默认不进入，输出已知边界 | 未覆盖计入部分结果，不显示为空目录 |
| 外部导入 | `import.*` | 保留来源、采样时间、列口径 | 仅用于分析；不能形成已核验身份 |

**官方行为：**`getattrlistbulk` 对打开的目录批量返回属性，符号链接返回链接本身；要求名称与 returned-attributes 字段，支持属性不保证每项都实际返回。[Apple XNU 手册源码](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/getattrlistbulk.2)

**实现边界（拟议）：**Rust 核心调用小而封闭的 libc/CoreServices/Foundation 适配层；Objective-C/Swift shim 仅承担需要的系统服务，不包含第二套规则引擎。扫描、事件接收与 UI 分离；结果使用有界批次送入 SQLite，前端分页取树，不搬运所有完整路径。

初始每卷 2 个目录 worker，最大 8 个补充属性查询、8,192 条待写记录和 64 MiB 队列内存；每次属性缓冲从 64 KiB 起步，上限和 fallback 由解析器控制。参数是原型起点，须比较 1/2/4/8 并发及耗时/能耗；APFS SSD 不代表无限并行会更快。

## 2. 批量扫描、身份和路径

**拟议扫描流程：**

1. 由用户选择的根建立 scope，记录 root fd、卷 UUID、挂载设备信息、文件系统类型、大小写敏感性及权限上下文。默认不跨挂载点；显式增加另一个根才扫描另一卷。系统卷与 Data 卷的多个入口、firmlink 别名按卷/实体去重，避免 `/Users` 与 `/System/Volumes/Data/Users` 重复计算。
2. 在专门扫描线程启用第 5 节的防物化策略，然后逐层相对目录 fd 打开下一层。不要先以字符串路径查询任意深层对象来“检测云文件”。
3. `getattrlistbulk` 请求 `ATTR_CMN_NAME`、`ATTR_CMN_RETURNED_ATTRS`、`ATTR_CMN_ERROR`，以及能力支持的类型、文件 ID、父 ID、修改时间、flags、逻辑长度、分配长度和链接数。按返回位图而非请求位图解析字段。
4. 检查每条记录长度、偏移、属性位图、对齐、缓冲边界及溢出；`ATTR_CMN_ERROR` 保留单条错误；整批不支持时回退，不将不支持属性填 0。`attrreference_t` 的相对偏移按结构定义解析，不以 C 字符串猜测记录边界。
5. 仅普通目录进入队列；符号链接、挂载点和特殊文件形成边界条目。FIFO、设备与 socket 不打开内容；`.app`/package 可折叠展示，但物理扫描是否深入由明确策略决定，不以 UI 折叠影响大小统计。
6. 对缺失的必要属性通过 fd 相对接口补查；持续变化、权限不足或触发防物化失败时返回部分结果。错误携带原始 errno 和稳定业务分类，不能只靠 `EPERM` 断言缺少 Full Disk Access。

**身份与名字（拟议）：**实体键 `(volume_uuid, file_id)`，目录项保存 `(parent_id, raw_name_bytes)`，临时 `st_dev` 与持有 fd 用于当前会话核验；`st_dev` 不能作为跨重启卷身份。保存 `st_nlink`、类型、创建/修改信息和当前范围。inode/file ID 可复用，时间戳不是唯一身份。保留名字原始字节，显示另做可打印编码；不要把 Unicode 归一化后的显示名用于执行，也不要在大小写敏感 APFS 上合并 `A` 与 `a`。

`open/openat` 的 no-follow 约束和相对 fd 有助于缩小路径解析风险，但不自动提供“核对 inode 后原子删除该 inode”的接口。[Apple open 手册源码](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/open.2)、[unlink 手册源码](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/unlink.2)。执行限制见第 7 节。

## 3. APFS 大小口径

**官方行为：**APFS clone 共享未修改块，支持稀疏文件、快照与容器内卷共享空闲空间；不能把同一容器多个卷报告的空闲量直接相加。`stat.st_blocks` 的单位是 512 字节。[About Apple File System](https://developer.apple.com/documentation/foundation/about-apple-file-system)、[Apple stat 手册源码](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/stat.2)

**拟议字段与映射：**

| 字段 | 来源与含义 | UI/汇总约束 |
| --- | --- | --- |
| `logical_bytes` | 普通文件 `st_size` 或默认数据 fork 的逻辑长度，记录来源 | 是内容长度，不能称物理占用 |
| `allocated_bytes` | `ATTR_FILE_ALLOCSIZE` 等文件系统报告分配量；fallback `st_blocks * 512` | 是系统报告值，不能称独占量；两接口差异需要记录而非静默覆盖 |
| `fork_scope` | `data_only / total_reported / unknown`；区分 data/resource fork 与其他元数据 | 不能混用 `DATALENGTH`、`TOTALSIZE`、分配量；仅比较同口径值 |
| `allocation_source` | `getattrlistbulk / stat_blocks / foundation` 与 SDK/FS 信息 | 字段缺失返回 null，不能假定所有 FS 行为一致 |
| `shared_extent_status` | `unknown / known_shared_by_fixture` 等证据状态 | clone 的 inode 不同，不能用硬链接去重算法识别共享块 |
| `reclaimable_estimate` | 默认 null；按动作、硬链接/快照条件及可证明范围再估计 | 不从 allocated 直接生成“删除必释放 N 字节” |
| `volume_free_bytes` | 指定挂载卷的可用量及观测时间 | 与目录求和分开；容器共享池只展示一次 |
| `important_usage_capacity` | 系统的容量建议指标，单列保存 | 不是 CleanManager 有权删除的 purgeable 垃圾 |

针对压缩文件、资源叉和扩展属性，阶段 0 同时保留原始接口字段，验证是否已包含相关存储；未知覆盖标 `size_coverage=partial`。不随意将所有 xattr 值读入内存来“补足占用”，更不把压缩文件的逻辑长度做簇对齐后当作物理量。目录自身元数据与快照保留空间可能不在常规文件求和内。

硬链接按实体去重，但保留各目录项；clone 保留为不同实体，并标记“共享块影响释放估算”。公开元数据不足以建立任意 APFS 文件的全局独占块账本，首版不实现在线 APFS 容器解析器，也不承诺精确 clone 回收估算。Apple 的 fast directory sizing 特性不作为任意目录 O(1) 精确计量的承诺。

Time Machine 本地快照由系统管理，删除活体文件后块也可能仍被快照引用；首版仅解释差额，不删除或自动瘦身快照。系统“重要用途可用容量”与普通 free 是不同口径。[Apple 本地快照说明](https://support.apple.com/en-us/102154)、[重要用途容量 API](https://developer.apple.com/documentation/foundation/urlresourcevalues/volumeavailablecapacityforimportantusage)

同卷 Trash/隔离通常只是改名移动，报告 `quarantined_bytes` 与 `observed_free_delta` 两个不同指标。后者受其他进程与系统回收影响，可为负，不可直接归因为本工具释放量。对应 `TC-MAC-003` 至 `TC-MAC-006`、`TC-SAFE-009`。

## 4. FSEvents 基线、检查点与重扫

**官方行为：**FSEvents 通知可能合并；`MustScanSubDirs` 需要递归重扫相关路径；`UserDropped`/`KernelDropped` 要重扫被监测范围；`RootChanged`、事件 ID 回绕及设备历史变化必须处理。Apple 建议在建立目录快照前启动监听，并把事件作为提示而非完整变更数据库。[Apple FSEvents 指南](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html)

**拟议持久字段：**`volume_uuid`、`fsevents_uuid`、stream 类型与 flags、root 身份/原始路径、scope 摘要、`received_event_id`、`applied_event_id`、durable dirty 集合、index generation、schema 与权限上下文。FSEvents 日志 UUID 和卷 UUID 是不同概念；均需存储核验，不能把设备号当作它们。

**拟议状态机：**`uninitialized → baselining → reconciling → current`；已知 coverage gap 为 `partial`；连续性/身份失效进入 `resync_required`。`current` 的含义是到记录水位完成已知范围对账，不是原子快照。

1. 在扫描前为获准的本地卷/根建立 stream，持久索引优先 per-device。启用 `WatchRoot`；`FileEvents` 可帮助缩小 dirty 范围，但不能省掉重扫。配置变化必须使原检查点失效。
2. 确认监听已启动后建基线，同时把事件涉及目录加入有界、可落盘的 dirty 集合。父路径 dirty 覆盖其子路径；队列溢出升级范围重扫，绝不丢事件后照常提交完整。
3. 基线结束对扫描期间发生变化的子树重新枚举，与已有目录项集合对账。rename 不要求配对；记录旧/新父范围，身份不明则扩大到最近可信根。删除事件只是失效提示，目录重扫才确认最终状态。
4. 利用事件 flush/交付水位组织追赶，但 flush 不等于文件系统静止。重扫期间的新事件继续进入下一批；持续 churn 超出预算后保持 `partial`/dirty，后续继续对账。
5. 同一 SQLite 事务保存子树替换、dirty 待办与检查点；只有没有未处理变更的水位可以提升 `applied_event_id`。若先持久 received 游标，必须同时持久其对应 dirty 工作并确保重启先处理，不能让收到即等于已应用。
6. 重启核对 root、卷 UUID、日志 UUID、历史有效性和索引版本。恢复被拒绝、无法证明历史连续、ID 回退/回绕或设备被重格式化时重建获准范围；禁止直接从 `SinceNow` 恢复并宣称历史完整。

| 信号/情况 | 拟议处理 | 可观察状态 |
| --- | --- | --- |
| 普通目录/文件变化 | 合并相关父目录 dirty，安全枚举并对账 | `dirty_roots` 增加后减少 |
| `MustScanSubDirs` | 在权限和挂载约束内递归重扫被标记子树 | `rescan_reason=coalesced` |
| `UserDropped` / `KernelDropped` | 该 stream 全部获准 roots 重建 | `resync_required`；不能只补收到的那一个路径 |
| `RootChanged` | 停止沿旧路径操作；核验持有 root fd 和身份 | `root_changed`；新同名目录不自动继承授权 |
| `EventIdsWrapped`、日志 UUID 改变、游标回退 | 废弃该流检查点，重新建基线 | `rescan_reason=history_invalid` |
| 卸载/重接卷 | 结束旧 stream，重新探测身份与权限 | `volume_offline / identity_changed` |
| 网络卷不支持可靠事件历史 | 按需和定期重扫批准根 | `incremental_support=unavailable` |

定期核对不能被事件机制取消。初版可在索引超过 24 小时后下次打开时触发有界核对；间隔是设计值，需要阶段 0 校准。FSEvents 的文件级事件不是精确操作日志；不根据事件计数推算字节变化，不关闭系统日志或清除其他消费者所需历史。对应 `TC-MAC-007` 至 `TC-MAC-010`。

## 5. iCloud / File Provider 与防物化

**官方行为：**dataless 文件只有本地元数据；读取、目录枚举乃至存在性检查都可能使文件或中间目录 materialize。`SF_DATALESS` 可用于识别，但查询深层路径本身可能物化祖先。Apple TN3150 介绍按线程/进程关闭 dataless 物化策略，并处理 `EDEADLK`。[TN3150](https://developer.apple.com/documentation/technotes/tn3150-getting-ready-for-data-less-files)

**拟议实现：**

- 默认 `cloud_policy=avoid_materialization`。未知 File Provider/iCloud 根先作为边界展示；不能为了识别 provider 就递归遍历所有 `~/Library/CloudStorage` 内容。
- 所有扫描 I/O 在固定 worker 线程上进行；先读取旧策略，再尝试 `setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_THREAD, IOPOL_MATERIALIZE_DATALESS_FILES_OFF)`，用 RAII 在退出/错误时恢复。异步任务若迁移线程会失去约束，因此 I/O 不任意迁移，补查和事件重扫也走同一受控路径。
- 从已知本地父目录逐层读元数据，检查 flags；遇到 dataless 目录不进入。需要安全补查时使用相对 fd 并维持 no-follow。不能用普通 `stat(full_path)` 作为访问前的无风险探针。
- 策略接口不存在、设置失败、provider 行为未验证或收到 `EDEADLK` 时输出 `cloud_materialization_avoided` 和未知范围；不临时打开物化再重试，不当作普通死锁无限退避。
- 不调用下载 API、不读取文件内容哈希/缩略图、不解压或预览包、不驱逐云数据。纯 metadata-only 字样不作为行为保证。禁用 APFS dataless 物化也不代表禁用所有 NFS/SMB 网络 I/O。
- provider 状态可能在检查后变化；只在指定 OS/provider 实测通过后启用更精细枚举。无法验证则默认跳过。原始逻辑大小可展示，但本地驻留量未知时不能把它计入可释放量。

**待验证：**iCloud 与至少一个实际采用的 File Provider 需在真实 Mac 上观测 API 轨迹、provider 日志、dataless flags、驻留量及网络变化。总网卡流量不能区分背景同步与本工具触发的下载；缺乏归因证据时只能写“未确认”，不能通过零下载用例。对应 `TC-MAC-011`、`TC-MAC-012`。

## 6. POSIX、TCC、Sandbox、SIP 与 helper

**官方行为：**传统权限/ACL、App Sandbox、强制访问控制（含用户隐私保护）有不同作用；root 不意味着可绕过全部隐私或系统保护。Apple DTS 的权限说明是官方工程指导，具体授权归属仍需目标系统实测。[On File System Permissions](https://developer.apple.com/forums/thread/678819)

**拟议权限模型：**

| 边界 | 本项目处理 | 不能作出的假设 |
| --- | --- | --- |
| POSIX owner/mode、ACL | 按实际访问结果报告缺口；不自动 chmod/chown | 普通文件可读不代表目录可枚举 |
| TCC / Full Disk Access | 按实际执行程序与责任链提示用户在设置中授权 | GUI 获准不代表 CLI/helper/终端启动的进程全部获准 |
| App Sandbox | 若将来做沙盒版本，采用用户选择范围和 security-scoped 访问并单独收窄能力 | 站外签名、公证或 root 不是 sandbox extension |
| SIP/只读系统卷 | 默认保护，不提供关闭保护或删除系统卷入口 | FDA 不等于系统目录可改写 |
| helper/root | 只承接确需较高 POSIX 权限的固定操作 | 不能解决所有 TCC 拒绝，更不提供通用提权终端 |

CLI 从 Terminal、GUI 子进程、launchd/XPC helper 启动时分别测试权限归属。`permission_required` 必须说明对象、可证实的阻塞层和可执行的下一步；仅凭 `EACCES/EPERM` 无法判别时写 `permission_cause=undetermined`。首版不批量请求 FDA，不访问私人目录来试探授权，也不修改 TCC 数据库。

**隐私弹窗期间的读取（2026-10-18 真机实测）：**负责的应用对某个受保护位置（下载、桌面、文稿、其他应用的数据）还没有做过选择时，第一次 `open(2)` 会卡在内核里等对话框的回答，没有超时，也不返回 `EPERM`；在 launchd Background 会话里也一样。`lstat` 不等待；只卡住发起调用的线程，进程仍能正常退出。复现方法与日志见 [macOS 真机验证指南](macos-verification.md#已知的真机现象隐私弹窗会让读取停住已处理)。因此对受保护位置的第一次访问都经由 `cm-platform` 的 `macos::privacy::bounded`：放在单独的线程上执行，最多等 `PROMPT_WAIT`（5 秒）。超时后该位置报告为 `permission_required (permission_cause=privacy_prompt_pending)`，说明位置与下一步（回答对话框后重新测量），大小与数量未知，不按 0 或空计算；调用方继续执行，等待中的线程在对话框回答后自行结束，结果丢弃。同一路径还在等待时，再次请求会立刻报告等待中，不会再开新线程。`--debug` 轨迹在等待超过 250 毫秒和超时时各写一行（`privacy:`）；桌面端据此显示“正在等待 macOS 授权”。这不是试探授权：只包住本来就要做的那次读取，不重试，不改任何隐私设置。目前接入的是空间洞察（下载文件夹、iPhone 备份、其他应用容器内的磁盘映像）；扫描与清理目录解析尚未接入。

**helper 设计：**默认不安装常驻服务；需要时选择本应用 bundle 内签名 helper，优先基于 `SMAppService` 注册并由系统/用户批准，launchd 按需激活。注册生命周期与一次操作授权分开。`SMAppService` 管理的是本应用的 helper，而非第三方任意进程。[官方 Service Management](https://developer.apple.com/documentation/servicemanagement/smappservice)

XPC 协议采用固定消息类型、最大条目数与数据量、nonce、deadline、caller、计划摘要、scope 与一次性批准凭据。高权限服务核验真实连接的 audit token/代码签名要求（Team ID、designated requirement、允许的客户端身份），不能信任消息里的 UID/PID/path。具体可用的 XPC peer 校验 API 随最低系统确定；无法建立可信 caller 绑定时不发布写 helper。普通用户可写的 SQLite 记录不能单独证明特权授权。

高权限 helper 不调用用户提供的 shell、Homebrew、卸载脚本或任意可执行路径；不将客户端字符串直接传给递归删除。只对管理员控制、不可由低权限主体换名的根开放已适配动作。用户可写目录中的动作原则上以用户身份执行；即使以用户身份执行，也受第 7 节的竞态能力限制。注册失败/批准被拒绝不循环弹窗，返回 `permission_required`；协议版本不同返回 `helper_version_mismatch`。

签名与鉴权只能界定进程和权限，不能自动证明“用户希望删这批文件”。已有授权仍需绑定具体计划。用户账户已被完全控制、同 UID 任意进程以及 root 攻击者不是本工具能隔离的威胁模型；需要更强保障必须依赖系统隔离。

## 7. 链接、挂载与名称竞态的实际限制

**拟议策略：**持有 root fd，逐级 `openat`，普通目录使用 `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`，使用 `fstatat(..., AT_SYMLINK_NOFOLLOW)` 核验名称对应的实体。默认不跟随 symlink，不跨 `st_dev`/卷边界；符号链接本身只有在单独明确计划中处理。对新出现文件、未知 descendants、共享容器和受保护子项没有继承批准。

**必须写清的限制：**`fstatat(parentfd, name)` 后再 `unlinkat(parentfd, name)` 仍是两次调用，另一个进程可在其间替换最后一个名字。`renameat`/移入 Trash 也没有自动附带“仅当 inode 仍等于 X”条件。目录 fd 能稳定父目录对象，但祖先移动会改变它在命名空间中的位置。`flock` 是协作锁，不能阻止不合作程序改名；重新检查两遍也不构成原子保证。[Apple open](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/open.2)、[Apple unlink](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/unlink.2)

据此采用以下**拟议能力门槛**：

1. 计划优先细化为叶子操作；核验实体、父项、类型、规则元数据和受保护范围，发生不符立即 `plan_stale`。
2. 需要跨权限安全保证时，只接受受信任、无法被较低权限主体改名的命名空间。root helper 对普通用户可写的任意目录执行删除/rename 的能力默认关闭。仅“owner 是用户”不能排除同 UID 并发替换。
3. 对要求严格身份绑定、但当前后端无法原子约束名称变化的计划，返回 `race_safety_unavailable`，不给出已安全执行的假象。自动/Agent 模式不通过免责声明绕过此门槛。
4. “应用已退出”和短时间无事件只是静默线索，不能等同锁定目录。若后续设计协作维护协议，需定义应用如何独占命名空间、哪些主体仍可修改，并独立审计。
5. 私有隔离目录中的后续处理可减少其他 UID 的竞争，但将候选项从原目录移入隔离区的第一步仍可能竞争。事后核对发现错移时保留内容、报告冲突，不自动覆盖恢复或永久删除。私有隔离不被当作解决首步竞态的证明。
6. Trash 由系统接口处理可恢复动作；必须记录真实返回位置。失败不能改用 `unlink`。对不能满足身份要求的 Trash 操作也拒绝。

这是首版清理能力的真实约束，不承诺 POSIX 路径 API 能实现任意目录的零竞态清理。阶段 0 只读不受执行门槛影响；写功能在 `TC-SAFE-002`、`TC-SAFE-004`、`TC-SAFE-006` 通过对应限定范围后才开放。

## 8. 应用清单、卸载和登录/后台项

**拟议 inventory：**仅扫描批准的 `/Applications`、用户 Applications 和包管理器已知位置；以 bundle URL、bundle ID、版本、Team ID/签名和安装来源组合识别。同 bundle ID 多副本保留为不同安装实例；不能仅用 bundle ID 推断所有同名配置都可删。pkg receipt 是安装证据，不等于完整卸载脚本；LaunchServices/Spotlight 只作补充，不能当作完整磁盘清单。

| 类型 | 拟议后续动作 | 边界与恢复语义 |
| --- | --- | --- |
| 普通拖拽安装 `.app` | 退出状态核验后移入 Trash | 仅应用包；不承诺移除登录项、扩展、共享容器或用户数据 |
| 有官方卸载器/管理流程 | 优先引导应用自身卸载器；适配后固定参数调用 | 外部维护操作，不是预先枚举的逐文件删除事务 |
| Homebrew formula/cask | 用户已安装 brew 时查询 receipt，按固定 token/type 调用正常 uninstall | 不用 shell 拼接；不 sudo brew；版本、依赖及退出结果必须记录 |
| Homebrew cleanup | 单独展示工具版本与 `--dry-run` 结果，再单独授权维护动作 | dry-run 到 apply 会变化；不能保证全量可恢复 |
| Cask `zap` | 首版关闭自动入口 | 官方说明可能删共享资源；不能等同安全清残留 |
| pkg/系统扩展/驱动/VPN | 首版识别与说明来源，指向供应商流程 | 不根据 receipt 删除 `/Library` 文件；不绕过系统扩展卸载约束 |
| 应用容器/App Group | 只读显示已证实归属与共享关系 | 孤立目录名不是已卸载残留的证明；共享容器默认保护 |

**实现状态（卸载，2026-10-17）：**普通 `.app` 退出后移入废纸篓、Homebrew cask/formula 的固定参数 `brew uninstall`（不用 zap）与 `~/Library` 残留的第二段计划已实现，只在托管 runner 上用夹具验证；App Store 与当前用户不能修改的应用包没有提权路径，返回 `permission_required`；`Containers` 只报告、`Group Containers` 共享保留。npm 全局 AI 命令行工具经受信任的 Node.js 执行 `npm uninstall --global`，会话与设置从不预选；孤立残留只按 bundle ID 证据列出（目录名本身不是证明：候选一律不预选、须闲置 30 天、逐个点名后才进入单独审阅的废纸篓计划）。细节见 [应用卸载](../app-uninstall.md) 的“macOS”。

**官方行为：**Apple 建议优先使用应用自身卸载方式；Homebrew 的 uninstall、cleanup 和 cask zap 有不同作用范围，zap 可能删除共享资源。[Apple 卸载指引](https://support.apple.com/en-us/102610)、[Homebrew 手册](https://docs.brew.sh/Manpage)、[Cask Cookbook](https://docs.brew.sh/Cask-Cookbook)

**启动/后台项（拟议）：**

- CleanManager 自身登录项使用 `SMAppService.mainApp`，读取真实 status，尊重用户/系统批准；默认关闭。
- 可读取明确允许位置的 LaunchAgents/LaunchDaemons plist，并展示 Label、ProgramArguments、签名归属和作用域；文件存在不等于已加载或已启用，无法查询就标 unknown。
- 第三方现代后台项不提供“全部一键开关”。`SMAppService` 不是全局管理 API；提供系统登录项设置入口和供应商流程。MDM 管理项保持只读，不能绕过策略。
- 不修改系统 Background Task Management 私有数据库，不靠删除 plist 假装已停止服务，不把 `sfltool` 等诊断输出当稳定产品协议。后续 `launchctl` 适配必须绑定具体用户域/标签、保存原状态、限定自身或已验证的应用；首版不执行此类修改。

上述范围来自 `SMAppService` 官方对本应用 helper 的描述，表中的阶段与保守策略是本项目选择。[官方 API](https://developer.apple.com/documentation/servicemanagement/smappservice)。对应 `TC-MAC-015`、`TC-MAC-016`。

## 9. 分发、签名、公证与更新

**官方行为：**站外分发需要相应 Developer ID 签名与公证流程；公证要求分发的可执行代码具有有效签名、时间戳和所需 Hardened Runtime，公证不是 App Review。Tauri 提供 macOS 签名/公证接入，updater 另有自己的签名验证。[Apple 公证指南](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)、[公证错误处理](https://developer.apple.com/documentation/security/resolving-common-notarization-issues)、[Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/)、[Tauri updater](https://v2.tauri.app/plugin/updater/)

**拟议发布流程：**

1. 先提供 `aarch64-apple-darwin`；只有 Intel 真机验收通过才提供 x86_64 或 universal 包。分别记录 deployment target、SDK、Rust/Tauri 版本。依赖 Rosetta 运行不能算 Intel 原生测试。
2. `.app` 内所有嵌套 helper、CLI、framework 先分别签名，再签外层；不靠 `codesign --deep` 修补未知嵌套签名。最小化 entitlements；不为方便保留调试权限或不必要的 JIT/库验证例外。
3. 使用 Developer ID Application 签代码；若后续提供需要安装步骤的 pkg，使用匹配的 Installer 身份。通过 `notarytool` 提交正确容器，检查结果并对支持的分发容器 staple 票据。独立 CLI 必须随可验证签名/公证路线分发，不能只公证 GUI。
4. 干净 Mac 上保留正常下载 quarantine 属性，验证 Gatekeeper、首次启动和离线启动；不通过清除 quarantine、关闭 SIP/Gatekeeper 来“修好安装”。签名身份变更可能影响隐私授权归属，必须作为升级测试变量。
5. CLI 与 GUI 共享核心，但可独立运行。提供用户选择的 CLI 安装路径；不擅自写 `/usr/local/bin` 或覆盖已有命令。可选用户级链接必须检测冲突，移除时只删除自身仍指向正确目标的链接。
6. helper 注册、用户批准、主 app 路径和升级一起测试；用户移动/删除 app 后能报告服务不可用，不留下接受任意旧客户端的 helper。更新前停接写任务并保留恢复日志，数据库迁移失败返回旧版本可读状态。
7. 同时验证 updater 签名、平台签名、架构与版本。helper 与客户端协议不匹配时拒绝动作；卸载软件不顺带永久清空 Trash、隔离数据或用户审计记录。

当前没有签名身份、发布包或 Mac 真机验证证据。`TC-MAC-017`、`TC-MAC-018` 未执行，不能以 Windows 上构建文档/交叉编译结果代替。

## 10. 阶段 0 与未决项

阶段 0 交付为只读 `mac.posix` 与 `mac.bulk`、可复现语料 manifest、APFS 字段原始观测和 FSEvents 故障注入接口。不得为测试创建/删除生产快照、修改用户隐私数据库、遍历整台 Mac 或执行清理器。测试夹具和命令必须绑定明确批准的测试范围，当前没有实际运行结果。

| 未决项 | 所需证据 | 验证入口 |
| --- | --- | --- |
| 批量枚举相对 POSIX 基线的收益 | 真机、同语料/字段/缓存/权限，含补查和数据库时间 | `TC-P0-004`、`TC-MAC-001` |
| APFS 压缩、fork/xattr 的分配字段覆盖 | 同对象多个原生接口与夹具 manifest 对照 | `TC-MAC-003`、`TC-MAC-004` |
| clone/快照的空间变化 | 专用测试卷，保留共享与并发条件；只验证语义而非普遍固定字节值 | `TC-MAC-005`、`TC-MAC-006` |
| provider 是否服从防物化策略 | 指定 OS/provider 的真实访问轨迹和可归因驻留变化 | `TC-MAC-011`、`TC-MAC-012` |
| CLI/GUI/helper 的 TCC 责任链 | 多启动方式、多个签名版本的授权/撤销测试 | `TC-MAC-013`、`TC-MAC-014` |
| 可安全发布的写操作范围 | adversarial rename、共享目录和权限边界测试 | `TC-SAFE-002`、`TC-SAFE-004` |
| 最低系统、Intel 支持、签名/升级流程 | 实际支持版本和物理设备安装验收 | `TC-MAC-017`、`TC-MAC-018` |

在上述证据到位前，产品能力必须继续标注未验证/不可用；不得从官方机制说明推导出 CleanManager 已达到秒扫、零下载、完整访问或任意操作可回滚。

## 11. 测试重定向与真机门槛

macOS 后端的自动化测试与 Windows 使用同名的环境变量重定向，只让测试读写自己创建的临时目录，不触碰真实用户数据。以下变量只供测试与验收脚本使用，不是产品配置；产品代码遇到它们以外的值时按真实位置解析。

| 变量 | 作用（macOS） | 对应 Windows |
| --- | --- | --- |
| `CM_STORE_DIR` | 索引、审计、隔离区与设置所在目录 | 同名 |
| `CM_RULES_DIR` | 规则包目录（测试用夹具规则） | 同名 |
| `CM_RESOLVE_HOME` / `CM_RESOLVE_TEMP` / `CM_RESOLVE_DOWNLOADS` | 规则根解析使用的家目录（`~/Library/Caches`、`~/Library/Logs`、`~/.Trash` 等都相对它）、临时目录与下载目录；设置 `CM_RESOLVE_HOME` 时 `trash list`/`trash purge` 只列出 `$CM_RESOLVE_HOME/.Trash`，不列 `/Volumes` 下的废纸篓 | `CM_RESOLVE_USERPROFILE` / `CM_RESOLVE_TEMP` / `CM_RESOLVE_DOWNLOADS` |
| `CM_RESOLVE_SYSTEM` | 代替 `/` 的夹具根：需管理员的系统日志规则（`/Library/Logs`、`/private/var/log`）在其下解析；设置 `CM_RESOLVE_HOME` 而未设置它时，系统位置不解析 | `CM_RESOLVE_PROGRAMDATA` |
| `CM_UNINSTALL_TEST_ROOT` + `CM_UNINSTALL_TEST_HOME` | 卸载清单与残留扫描的假根：`ROOT` 下放 `Applications/`、`opt/homebrew/Caskroom/`（及 `Cellar/`）、`Library/`，`HOME` 下放用户 `Library/`；路径一律以该前缀解析，不读真实 `/Applications` 或 Homebrew | 同名（注册表根与家目录） |
| `CM_STARTUP_TEST_ROOT` / `CM_STARTUP_TEST_FOLDER` | 启动项清单与启停的假根：launchd plist 目录（`Library/LaunchAgents`、`Library/LaunchDaemons`）在其下解析；计划中的位置记录该根，换根的计划不能作用于真实域 | 同名（测试注册表键与启动文件夹） |
| `CM_MAINTENANCE_TEST_SCRIPT` / `CM_MAINTENANCE_TEST_ELEVATION` | 维护任务以脚本结果代替真实执行；`ELEVATION` 只模拟“拒绝/直接”两种提权结果（macOS 没有提权路径，`ELEVATION` 不起作用） | 同名 |
| `CM_MAINTENANCE_TEST_PREFS_DIR` | 维护设置变更（`macos.network-dsstore-off`）读写 `<目录>/com.apple.desktopservices` 这个绝对路径偏好域，而不是当前用户的偏好域；报告类维护任务读的家目录由 `CM_RESOLVE_HOME` 重定向 | 无 |
| `CM_ALLOW_DMG_TESTS=1` | 允许挂载一次性磁盘映像的测试（已有） | `CM_ALLOW_VHD_TESTS=1` |
| `CM_ALLOW_MAC_DEVICE_TESTS=1` | 允许触碰本机真实状态的测试：用户真实废纸篓、LaunchServices 注册、电源断言（保持亮屏）等。未设置且不在 `GITHUB_ACTIONS=true` 的托管 runner 上时，这些测试打印 `skipped` 并通过 | 无（Windows 对应测试只在 CI 或显式变量下运行） |

`CM_ALLOW_MAC_DEVICE_TESTS` 的测试仍只操作自己创建的文件或断言（例如移入废纸篓的是测试刚建的临时文件，断言在测试结束时释放），不会清空废纸篓、注销他人应用或改变系统设置。

**托管 CI 能证明什么：**`macos-14` arm64 runner 上的编译、clippy、单元与契约测试、基于临时目录与重定向的夹具流程（扫描、规则解析、计划生成、卸载/启动项假根），以及在 runner 自身临时用户下的废纸篓移动与电源断言调用能成功返回。**不能证明：**真实用户机器上的 TCC/完全磁盘访问授权、SIP/沙盒边界、Gatekeeper 与签名/公证、`SMAppService` 用户批准流程、真实应用与 Homebrew 版本、iCloud/File Provider 行为、Intel 机型或其他系统版本。托管 runner 不是参考设备，结果不写入 `docs/evidence/device-matrix.json` 或任何验收记录；门禁（D15）仍要求真机证据。

**本构建在 macOS 上没有提权路径：**不调用 `osascript` 的 `with administrator privileges`，不使用已废弃的 `AuthorizationExecuteWithPrivileges`，也未安装特权 helper（D12 的 `SMAppService` 守护进程尚未实现）。需要管理员的维护任务在计划中照常列出，执行时逐项记为 `skipped`、原因 `elevation_unavailable`；系统域的 launchd 作业与其他系统级变更同样不执行，只说明原因。
