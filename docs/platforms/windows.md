# Windows 平台设计

日期：2026-10-14。状态：**拟议设计，尚无 CleanManager 产品实现或性能实测**。

本文细化 [架构分析](../architecture-analysis.zh-CN.md)，验收入口为 [验证计划](../verification-plan.md)。竞品定位、许可证和既有合成探针结果沿用架构研究，本文不重复竞品研究，也不把竞品宣传速度作为本项目指标。

证据约定：**官方行为**指下文链接对应的 Microsoft/Tauri 文档；**拟议设计**指 CleanManager 将实现的选择与约束；**待验证**指必须通过原型或指定环境确认的事项。官方接口存在，不代表本产品已经实现、通过验证或适用于所有文件系统。当前研究已有 WizTree 4.33 官方 portable 的包清单与 PE 资源静态证据，见 [REA bundle](../evidence/rea-wiztree-4.33.bundle.json)；[native provider 不可用记录](../evidence/rea-native-provider-unavailable.json) 限制了 native 调用路径分析。程序未运行，资源证据不能证明 MFT 大小算法或性能；Evidence ID 与边界集中在 [验证计划](../verification-plan.md)。

## 0. 与核心和 v1 协议的映射

以 [核心架构](../core-architecture.md)、[安全规则](../safety-and-rules.md)、[CLI 协议](../cli-protocol.md) 和 [v1 Schema](../../schemas/v1/protocol.schema.json) 为对外契约。本文的 backend ID 是拟议实现标识，Schema 的 backend 为字符串；与合成示例的 `portable_walk` 不构成已有产品版本差异。

- **计划默认 TTL 为 15 分钟（900 秒）**，`expires_at = created_at + 900s`；到期拒绝继续执行，返回 `plan_stale`，退出 6。期限内仍逐项实时核验，授权到期/撤销另行判断。不能以重启或重试延长旧计划。
- 文中 `current`、`resync_required`、`cancel_pending` 等为索引/worker 内部状态，不添加到 v1 `scan_result.status`。发布终态只用 `completed / partial / cancelled / failed`；缺口或未对账范围映射 `partial_scope`，部分结果退出 5，用户取消退出 130；停止过程用 `progress.phase=stopping`。
- `size_source`、streams、dirty/cursor、helper 原因等详细字段先保存在核心 Observation/内部诊断，**不向严格 v1 对象擅自加字段**。对外使用已有 `issues`、capability `reason`、action `code/message`；顶层错误只用 Schema 枚举。`race_safety_unavailable` / `unsupported_safe_action` 映射 `code=unsupported`（退出 7），具体原因放 message；`helper_version_mismatch` 映射 `schema_mismatch`（退出 2），不能往严格 `error.details` 添加 reason。
- 内部 `reclaimable_estimate` 对应 candidate 的 `reclaimable_estimate_bytes`；空闲量和差值对应 `volume_free_before_bytes / volume_free_after_bytes / observed_free_delta_bytes`。字节数为十进制字符串，未知分配量/估计为 null。v1 scan 的 `logical_bytes` 表示已知部分之和，配合 `unknown_size_count`、`skipped_ranges` 和 `partial_scope`；未知目录子项数不猜测。逻辑量未知的对象可留在内部索引，暂不输出要求逻辑大小的可执行候选。
- Windows 原始路径对外编码为 `windows_utf16le_base64`。内部卷/ID/代次映射到 `identity.volume_id/file_id/generation/quality/observed_at`；祖先和其他前置元数据留在受验证的 Scope/Observation，并纳入 `observed_fingerprint_sha256` 的实现约定，不向 plan item 添加私有字段。指纹是元数据摘要，不要求读取云文件内容。弱身份不能升级成可靠特权写能力。
- 计划采用 `CM-C14N-v1` 摘要，批准凭据与 PlanBody 分离；逐项执行遵循 durable intent → 实时核验 → OS 动作 → durable result。外部卸载与后台管理属于后续 MaintenanceOperation，不伪装为 v1 精确文件计划。

## 1. 支持范围与后端选择

**拟议设计：**首个发布候选以 Windows 11 x64 为目标；具体最低 build、SDK、Rust/Tauri 版本在阶段 0 后冻结。ARM64 是单独发布门槛，交叉编译或 x64 仿真成功不等于原生支持。Windows Server、ReFS、网络文件系统不继承本地 NTFS 的能力声明。

`probe(root, requested_scope, permission_context)` 返回文件系统、卷身份、挂载方式、可用接口、权限、云盘边界和选择理由。只探测用户指定根及其所属卷；“扫描一个目录”不能偷偷变成读取整卷命名空间。卷级快速扫描必须具有明确的卷级读取范围。

| 条件 | 拟议 backend ID | 枚举、大小与增量 | 降级行为 |
| --- | --- | --- | --- |
| 任意本地目录；普通用户 | `win.directory` | Win32 批量目录信息；按需补充句柄属性；首次发布的可靠基线 | 拒绝访问形成 coverage gap；不自动提权 |
| 明确选择本地 NTFS 整卷且允许元数据读取 | `win.ntfs_enum` | `FSCTL_ENUM_USN_DATA` 构建身份骨架，另查大小、硬链接名字；USN 辅助增量 | 卷接口不可用或不支持版本则退回目录遍历；说明范围和完整性变化 |
| NTFS 小范围目录，已有获准的卷索引 | `win.ntfs_index` | 从索引取该子树，再重新核验范围内元数据 | 过期索引不能生成可直接执行的计划 |
| exFAT/FAT、尚未适配的 ReFS | `win.directory` | 能取得的属性；无保证的 ID/分配量置未知 | 不调用 NTFS 枚举接口；不声称这些文件系统一概没有 USN |
| SMB/UNC、可移动介质 | `win.directory.remote` | 限制并发、请求数和超时；事件仅作失效提示 | 断线为部分结果；禁止切换到同盘符的新设备 |
| 外部 CSV | `import.wiztree_csv` | 保留导入来源、列口径、时间；无活体身份保证 | 仅分析，执行前必须进入本地重新扫描/核验流程 |
| 原始 MFT 解析 | `win.ntfs_raw_mft`，默认禁用 | 后续研究分支，须完整解析与差分验证 | 不是首版依赖，也不作为默认提权理由 |

**官方行为：**`FSCTL_ENUM_USN_DATA` 在 NTFS 卷上按文件引用号继续枚举；与读取变更日志的 `FSCTL_READ_USN_JOURNAL` 不同。目录批量结构 `FILE_ID_BOTH_DIR_INFO` 可携带名称、ID、EOF 和分配量。[枚举接口](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data)、[目录信息结构](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_both_dir_info)

**实现接口（拟议）：**平台模块向核心暴露 `probe`、`scan_batches`、`refresh_entities`、`read_changes`、`revalidate_entry`、`apply_fixed_action`。扫描返回有界批次和取消令牌，不直接写 UI。初始参数为每卷 2 个目录 worker、最多 8 个属性查询、8,192 条待写记录和 64 MiB 队列上限，先达到哪个就背压；这些是待调优配置，不是性能结论。机械盘与网络盘另测 1/2/4 并发，不默认按 CPU 核数增加随机 I/O。

## 2. 身份、路径和空间模型

**拟议设计：**文件实体与目录项分表。实体键为 `(volume_instance, file_id_kind, file_id_bytes)`，目录项键为 `(parent_entity, raw_name)` 并指向实体。`volume_instance` 绑定卷 GUID 路径、卷序列号、文件系统与本次挂载证据；盘符只作显示。不同 API 的 64/128 位 ID 必须显式标注和验证转换，不截断。NTFS FRN 保留完整值，包括记录复用相关信息；不只保存 MFT 槽号。

文件 ID 仍可能复用。执行计划还绑定目录项父身份、原始 UTF-16 名、类型、链接/重解析标签及规则相关的大小/时间等元数据。时间戳只是过期信号，不是唯一身份。路径大小写按目录/卷实际语义处理；不全局 lowercase，不以显示用 Unicode 归一化字符串执行。长路径使用宽字符 API 与应用清单配置；UNC、尾部空格和显示转义由路径层统一处理。

| 字段 | 拟议读取与语义 | 不能推导的结论 |
| --- | --- | --- |
| `logical_bytes` | 默认数据流 EOF；显式注明 `stream_scope=default_data` | 不是本地占用；不是垃圾大小 |
| `allocated_bytes` | 经适配器校准的系统分配量，记录 `size_source` | 不是独占块，也不一定是可回收字节 |
| `allocation_reported_bytes` | 保存原 API 的 `AllocationSize` 以便差分 | 不对压缩/稀疏情况盲目等同最终占用 |
| `compressed_or_sparse_bytes` | 支持时查询 `FileCompressionInfo`；独立保存来源 | 不把整个逻辑长度向簇大小取整来替代 |
| `streams_status` | `default_only / enumerated / unsupported / denied`；ADS 单列 | 未枚举 ADS 时不宣称总文件内容完整 |
| `link_count / names_complete` | 对硬链接补齐名字并检查范围内外关系 | USN 中的一条名字不代表全部目录项 |
| `reclaimable_estimate` | 默认未知；具备具体操作与共享条件后才估算 | 移入同卷隔离区不等于释放空间 |
| `volume_available_bytes` | 面向当前用户的可用量和全卷 free 分开记录 | 配额、并发写入下不能互相替代 |

**官方行为：**`FILE_STANDARD_INFO` 分别提供 EOF、分配量和链接数；`FILE_COMPRESSION_INFO` 与 `GetCompressedFileSizeW` 提供压缩/稀疏相关占用信息；硬链接共享同一文件实体。[标准信息](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_standard_info)、[压缩信息](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_compression_info)、[压缩大小](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getcompressedfilesizew)、[硬链接与 junction](https://learn.microsoft.com/en-us/windows/win32/fileio/hard-links-and-junctions)

**聚合规则（拟议）：**目录视图同时支持“按名字累计逻辑量”和“扫描范围内按实体去重的占用”。后者在跨目录硬链接时不能让各子树分别去重后再相加；核心维护集合或稳定归属规则，显示共享项。目录自身元数据、MFT、USN、回收站、快照及不可访问内容单列为未归属/未覆盖；目录求和不强制等于卷已用空间。错误和未覆盖内容的值为 `null`，不能以 0 补齐。

## 3. MFT、USN 与补齐大小

**官方行为：**`USN_RECORD_V2` 有 FRN、父 FRN、名字、原因等字段，没有 EOF 或分配量。名字按 offset/length 读取；记录有版本和可变长度。[USN 结构](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2)

**拟议算法：**

1. 以只读卷句柄分块枚举 USN 文件记录，检查输出长度、记录最小长度、版本、对齐、名字边界与整数溢出；保存下一次枚举用的 FRN 游标。不能把这个游标当作变更日志 USN。
2. 构建实体/父子关系骨架；暂时缺父项、名字冲突、目录移动和删除均进入 unresolved 集合，不丢弃成“空目录”。此阶段只允许显示“已发现条目”，不能显示完整大小排名。
3. 对范围内目录使用批量目录信息补齐名字与普通文件大小；可按 ID 打开文件获取 `FileStandardInfo`、`FileIdInfo`、`FileAttributeTagInfo`，对压缩/稀疏对象按能力补充占用。按 ID 打开失败时仅在核验同一身份后才接受路径回退。
4. 对 `link_count > 1` 的实体补充硬链接名字，或重扫有关父目录；不能假定单一 USN 名称覆盖所有别名。枚举更多名字仍受权限/扫描范围约束，无法补齐时设置 `names_complete=false`。
5. 对云占位文件采用第 5 节策略，不为补齐大小打开内容。未知大小保留未知。最后统计 `name_coverage`、`size_coverage`、权限缺口和重查失败数。

`OpenFileById` 是一个独立能力，存在文件系统、共享模式和访问权约束；它既不证明文件仍属于批准的父目录，也不提供删除授权。[官方接口](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-openfilebyid)

**待验证：**枚举加补齐可能比一次目录批量遍历更慢。阶段 0 必须分别测 `enumeration_ms`、`size_enrichment_ms`、`link_repair_ms`、`reconcile_ms`、`db_commit_ms`，总时间包含所有步骤；不以只返回名字的耗时宣传磁盘扫描完成。对应 `TC-WIN-002`、`TC-WIN-003`、`TC-P0-003`。

**原始 MFT 升级门槛（拟议）：**只有上述后端在相同口径下仍不能达到经实测制定的目标时才立项。`FSCTL_GET_NTFS_FILE_RECORD` 是读取记录的官方入口，但不替实现者解决属性解析与在线一致性。[官方记录接口](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_file_record)

研究清单必须包括 fixup、记录序列号、resident/nonresident `$DATA`、runlist、`$ATTRIBUTE_LIST`/扩展记录、多名字与 DOS 别名、ADS、压缩/稀疏、重解析点、并发修改与损坏记录。API 返回记录身份须重新核对。默认不读用户文件内容、不锁业务卷、不创建 VSS、不绕过 BitLocker；离线镜像解析与在线卷扫描分别验证。原始解析缺陷不得污染已发布索引；模糊测试与差分门槛见 `TC-WIN-016`。

## 4. USN 基线与重同步状态机

**官方行为：**JournalID 标识日志实例；`FirstUsn`、`NextUsn`、`LowestValidUsn` 分别帮助判定可读区间与连续性。读到已截断的起点会失败；日志换代不能沿用原检查点。变更原因表示“需要关注的变化”，不是文件新大小。[日志结构](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_journal_data_v0)、[读取参数及错误](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v0)

**拟议状态：**`uninitialized → baselining → catching_up → current`；已知缺口进入 `partial`，连续性破坏进入 `resync_required → baselining`。`current` 仅表示已对账到记录的水位，不是卷的原子快照或持续无误差保证。

1. 基线前查询卷身份与日志 `(J0, First0, Lowest0, U0=NextUsn)`。开启从 U0 开始的读日志/暂存，使扫描期间的事件得到保留。已有日志不可用时允许无增量的遍历，不自动创建、扩容或删除系统日志。
2. 枚举基线使用覆盖当前对象的范围，不把 `HighUsn=U0` 的过滤结果单独当作完整卷；扫描期间更新到 U0 之后的对象必须由回放补入。最简单初版采用完整当前记录枚举加重查，容许重复，拒绝漏项。
3. 取追赶上界 U1，验证 J0 未变，U0 仍处于可证明的有效历史区间。从 U0 消费到 U1，将变更 FRN、旧/新父目录、硬链接变化合并为 dirty 集合。不依赖 rename 事件恰好相邻或成对，不用 USN reason 的计数加减字节。
4. 重查 dirty 实体与受影响目录。删除通过重查/目录对账确认；移动目录使其后代路径缓存失效。对仍在写入、反复变化的对象保留 dirty，设时间/重试预算，不无限追赶一个繁忙卷。
5. 在同一 SQLite 事务中提交条目更新、未完成 dirty 工作、`journal_id`、下一读游标与索引 generation。只有已完成对账的水位才能叫 `applied_cursor`；已读但未处理的是 `received_cursor`。崩溃后重放必须幂等，不能只写游标。
6. 重启核对卷、日志 ID、有效区间、后端版本、索引 schema 与根身份。游标低于 FirstUsn、跨 LowestValidUsn 的不连续区间、超出 NextUsn、日志被重建、解析未知 major version、缓冲队列丢失、设备重接无法确认身份：标记 `resync_required`，重建获准范围。不能直接跳到日志尾端并继续报告完整。

日志读取不设置“仅关闭时返回”作为唯一更新来源，避免长时间打开的文件永远不刷新；即使使用全部适用原因，也要周期性范围核对。可先将“下次打开超过 24 小时的索引”列为核对触发器，间隔是可调设计参数。持续 churn、权限缺口和云盘跳过分别展示，不因日志连续就把 coverage 改成 complete。对应 `TC-WIN-004` 至 `TC-WIN-008`。

## 5. 云占位与默认不触发下载

**官方行为：**Windows 区分 offline、recall-on-open、recall-on-data-access 等属性；Cloud Files API 可从属性和 reparse tag 判断占位状态。`FILE_FLAG_OPEN_NO_RECALL` 有其适用范围，不能据此断言任意第三方同步提供者不会下载。[文件属性](https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants)、[占位状态](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetplaceholderstatefromattributetag)、[CreateFile 标志](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)

**拟议设计：**

- 扫描策略默认为 `cloud_policy=avoid_recall`；注册/已知同步根在进入前判定。父目录本身有 recall 风险时停止递归，不先枚举再决定跳过。
- 优先使用父目录已返回的名称、属性和大小；`CfGetPlaceholderStateFromAttributeTag` 可利用已有值，不要求为了识别再次打开文件。未知 tag、未知 provider、`RECALL_ON_OPEN` 或无法排除风险的祖先，输出 `cloud_metadata_unverified` / `cloud_recall_avoided`。
- 已验证适配器可使用最小属性权限与合适 no-recall/reparse 标志；`OFFLINE`、pinned 或 unpinned 单个标志都不是完整驻留判据。识别后到访问前仍可能被同步程序换成占位文件。
- 不取内容哈希、缩略图、预览、媒体属性，不发起 hydration/dehydration，不把删除云占位文件包装成“只释放本地缓存”。删除可能同步到云端，首版排除该操作。
- 只承诺在明确的 OS/provider/策略测试组合下未观察到 scanner 触发的 recall。不能验证的 provider 保守跳过；严格离线模式禁止进入未验证同步根。网络总流量不能单独作为下载归因。

`TC-WIN-009` 应同时检查提供者日志/回调、占位驻留状态和 CleanManager 的文件访问轨迹；背景同步引起的状态变化记录为混杂因素。

## 6. 权限与 helper 边界

**拟议设计：**GUI、CLI、规则计算和数据库默认在用户权限运行。卷元数据读取由按需启动、只读的 helper 执行；先实现读 helper，再单独评审写操作。UAC 只表示操作系统提权，不表示用户批准了某个清理计划。默认不启用 `SeBackupPrivilege`、不修改 ACL、不取得所有权、不读取其他用户目录来填补缺口。

helper 请求为版本化封闭协议：`request_id`、调用方身份、会话 nonce、期限、批准凭据、计划摘要、固定 `operation`、卷/根身份、条目集合和最大操作预算。首批操作只有 `probe_volume`、`enumerate_ntfs_metadata`、`refresh_metadata`；后续叶子操作需单独加入白名单。没有任意 shell、脚本、自由 IOCTL、任意目标可执行文件或递归删除端点。

IPC 使用仅本机命名管道，显式 DACL 绑定当前用户/登录会话与 SYSTEM，拒绝远程连接；校验连接 token、会话、协议版本和预期已签名程序身份。nonce 防串线但不能替代鉴权；PID、路径或“同一用户”均不足以证明批准。客户端也核验 helper 来源和版本。Microsoft 提供管道访问控制机制，以上具体协议与签名校验组合是本项目设计。[官方命名管道安全](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)

提权 helper 从管理员保护的安装位置启动，不能把用户可写目录中的同名程序当作可信 helper。首次安装/替换必须经可信安装流程。高权限 helper 重新读取并验证所有参数；拒绝使用客户端传来的“已校验=true”。授权凭据绑定实际调用者、计划、范围和一次性执行状态，数据库位于普通用户可写位置时不能成为提权授权的唯一依据。

当前阶段不设计常驻 SYSTEM 服务。取消后停止接受新任务，等待可取消 I/O 结束；不可及时取消的只读工作隔离在本工具持有的 worker 中，状态保留 `cancel_pending`。关闭工具不能把未完成日志标成成功。同用户恶意进程仍能绕过用户态工具自行操作文件；本产品不宣称防御已控制该用户账户或管理员的攻击者。

## 7. 身份、重解析点与执行竞态

**官方行为：**`FILE_FLAG_OPEN_REPARSE_POINT` 可打开重解析点本身；句柄信息和 `SetFileInformationByHandle` 提供基于已打开对象的操作能力。它们不自动核验完整祖先链、原计划或所有路径的原子一致性。[CreateFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)、[句柄操作](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle)

**拟议执行顺序（后续写能力阶段）：**

1. 对计划范围加跨进程操作锁；核验批准、期限、规则版本及索引 generation。锁只协调 CleanManager 自身，不锁住其他程序。
2. 打开并持有根及必要祖先的目录句柄，核验 ID、类型和 reparse tag。尽可能用不共享删除的句柄防止祖先移动；无法取得该约束就跳过，不能退回纯字符串前缀检查。普通 Win32 路径解析在打开最终对象前可能经过祖先链接，最终 no-follow 标志不能替代逐层核验。
3. 打开目标后比较完整卷/文件 ID、父目录项、类型与规则相关元数据，再对同一句柄请求支持的叶子操作。默认跳过 reparse/junction、挂载点和多硬链接对象；硬链接多名字删除只在专门适配与独立测试后开放。
4. 对用户可写/不可信目录中的高权限操作保持禁用；不让 helper 以高权限追随来自低权限命名空间的新目标。已有打开句柄、过滤驱动、共享模式、远程文件系统行为可能削弱假设，必须通过平台适配器能力声明。
5. 不执行“批准父目录后递归删所有内容”。有受保护子项时拆成明确叶子或跳过；新出现内容不继承批准。目录移除仅在确认预期身份且目录已空时尝试。

**限制必须可见：**文件 ID 不证明名称仍在批准的目录下；持有对象句柄不保证数据内容不被其他已有写句柄修改；同卷 rename/隔离与回收站接口有不同的身份保障。Shell 回收站是路径/项目操作，不作为抗竞态的通用句柄删除替代。若回收站后端不能达到当前计划的身份约束，返回 `unsupported_safe_action`；禁止悄悄永久删除。操作系统不支持所需语义时返回 `race_safety_unavailable`，而不是宣传“绝无 TOCTOU”。见 `TC-SAFE-001` 至 `TC-SAFE-006`。

## 8. 应用卸载与启动项能力

所有项目先交付只读 inventory：`source`、稳定 ID、版本、用户/机器 scope、证据、可支持动作和限制。应用显示名或路径关键词不能确立数据所有权；portable 应用和不完整注册信息必须标为低置信度。首版不自动执行卸载或修改启动项。

| 来源 | 拟议清单与后续动作 | 必须保留的边界 |
| --- | --- | --- |
| MSI/传统安装器 | 读取 HKCU/HKLM 卸载注册的 32/64 位视图；MSI 绑定 ProductCode 与安装上下文 | 不用注册大小作为实占；不执行任意拼接的 `UninstallString`；未知安装器引导系统卸载界面 |
| 已适配 EXE 卸载器 | 经签名、版本和路径核验后，以固定程序和独立参数调用 | 这是外部维护操作；不能承诺逐文件预览、事务回滚或可强制安全取消 |
| MSIX/AppX | `PackageManager` 查询当前用户；以准确 PackageFullName 调用移除 | 不默认移除其他用户、预配包或共享框架；异步完成后复查注册状态 |
| WinGet | 若用户已安装，解析明确 ID/source/scope；卸载前再次消歧 | 不自行安装 WinGet；不用显示名模糊匹配；记录工具版本、退出码、重启要求 |
| Run/RunOnce、Startup 文件夹 | 读取来源、原值、目标与用户范围 | RunOnce 可能承载安装事务，默认只读；启动文件夹仅处理已选定入口，不删除目标程序 |
| Task Scheduler | 读取任务路径、主体、触发器和动作；后续按已核验任务修改 Enabled | 并非所有任务都是启动项；系统、企业策略及维护任务默认只读 |
| 打包应用 StartupTask | 管理本应用声明的任务，其他项目引导设置页 | 尊重 `DisabledByUser`、`DisabledByPolicy`；不是任意第三方启动项控制 API |
| Windows 服务 | 首版只读并显示归属未知情况 | 不按名称批量停止/禁用服务，不把“后台运行”等同垃圾 |

**官方行为依据：**[卸载注册数据](https://learn.microsoft.com/en-us/windows/win32/msi/uninstall-registry-key)、[WinGet uninstall](https://learn.microsoft.com/en-us/windows/package-manager/winget/uninstall)、[PackageManager 移除](https://learn.microsoft.com/en-us/uwp/api/windows.management.deployment.packagemanager.removepackageasync?view=winrt-26100)、[Run/RunOnce](https://learn.microsoft.com/en-us/windows/win32/setupapi/run-and-runonce-registry-keys)、[任务 Enabled](https://learn.microsoft.com/en-us/windows/win32/taskschd/registeredtask-enabled)、[StartupTask](https://learn.microsoft.com/en-us/uwp/api/windows.applicationmodel.startuptask?view=winrt-26100)。表中的阶段、白名单和保护策略均为拟议设计。

后续启用动作前必须保留入口原始数据及安全描述、核对未被第三方修改；恢复遇到冲突不覆盖。卸载器成功退出不代表所有残留均可删，也不代表应用已经卸载；必须重新查询来源。未提供可靠取消的卸载器返回 `external_action_running`，不随意杀进程或回滚安装数据库。对应 `TC-WIN-012`、`TC-WIN-013`。

## 9. 打包、签名和升级

**官方行为：**Tauri 可生成 Windows NSIS/MSI 安装包，并提供 WebView2 分发选项；Windows 代码签名与 Tauri updater 签名是两套机制。[Windows installer](https://v2.tauri.app/distribute/windows-installer/)、[Windows code signing](https://v2.tauri.app/distribute/sign/windows/)、[Updater](https://v2.tauri.app/plugin/updater/)

**拟议发布流程：**

1. 阶段 0 只有 CLI/只读探针；桌面首版优先每用户 NSIS，企业 MSI 后续。独立 CLI 可单独分发，不依赖 WebView2 或 GUI 进程。所有二进制记录版本、构建目标、摘要及协议版本。
2. GUI 默认 `asInvoker`；签名 CLI、GUI、helper 和安装器并附可信时间戳。helper 的机器级安装作为单独可选步骤。签名不是 SmartScreen 无提示保证，干净机器必须实测。
3. 安装前检测 WebView2；提供在线 bootstrap 与明确标注体积的离线安装路线。运行时缺失/下载失败须明确反馈，独立 CLI 仍可使用。不会为扫描临时下载浏览器运行时。
4. CLI 安装目录和 PATH 修改显式可选，按当前用户保存；卸载只移除自身加入的准确 PATH 项，不覆盖用户新增内容。helper 更新核对签名、协议兼容及安装目录 ACL。
5. 更新包同时验证 updater 签名、平台代码签名、版本/架构及发布通道；签名密钥不进入仓库。升级时等待/拒绝新写任务，数据库迁移先留可验证备份，失败保持旧版本与 `migration_failed`。
6. 不允许旧 CLI 把未知新 schema 当作可执行计划；跨版本计划失效重新预览。卸载 CleanManager 不清空隔离区或用户报告，是否保留数据单独呈现。

`TC-WIN-014`、`TC-WIN-015`、`TC-SAFE-012` 是发布门槛，目前均未执行。

## 10. 阶段 0 交付与未决项

阶段 0 只实现 `win.directory` 与 `win.ntfs_enum` 的只读小样、空间来源字段和故障注入接口；不执行系统清理、卸载或修改启动项。样本规模、重复次数、缓存定义和空白结果表统一见 [验证计划](../verification-plan.md)。基准限定在明确批准的测试范围内，当前没有实际运行结果。

| 未决项 | 决策所需证据 | 关联用例 |
| --- | --- | --- |
| USN 枚举加大小补齐是否值得默认启用 | 同范围、同权限、同字段的端到端时间及覆盖率 | `TC-P0-003`、`TC-WIN-002` |
| 压缩/稀疏/ADS 的最终占用映射 | 原生 API、生成 manifest 与边界样本差分 | `TC-WIN-003` |
| 普通用户读取卷元数据的实际可用性 | 支持 build、token、ACL、驱动组合 | `TC-WIN-001` |
| 云 provider 的可验证 no-recall 路径 | 指定 provider 版本的轨迹与占位状态观测 | `TC-WIN-009` |
| 句柄操作和回收站的竞态保证范围 | 祖先/目标替换与共享句柄压力测试 | `TC-SAFE-001`、`TC-SAFE-006` |
| ARM64、最低 build、WebView2 安装策略 | 真机启动、签名、安装/升级与错误恢复 | `TC-WIN-014`、`TC-WIN-015` |

以上未决项不得从竞品研究、API 文档或 Windows 开发环境的存在推导为已通过。

## 已实测状态（2026-10-15，开发机 Windows 11 Enterprise 26200 x64）

以下来自 `win.directory` 原型与 [运行记录](../evidence/runs/)、[设备矩阵](../evidence/device-matrix.json)；未列出的组合保持 unverified。

| 能力 | 结果 | 证据 | 限制 |
| --- | --- | --- | --- |
| NTFS 目录扫描（普通权限上下文） | verified | `run_win_dir_tiny`、`run_win_dir_p0_10k`、`run_win_dir_denied` | 开发 shell 为提升令牌且启用了 SeBackupPrivilege/SeRestorePrivilege；CLI 启动即禁用二者，否则 `FILE_FLAG_BACKUP_SEMANTICS` 会绕过 ACL。独立非管理员账户未测 |
| exFAT 目录扫描 | verified | `run_win_dir_exfat` | exFAT 不支持 `FileIdInfo` 与扩展目录信息，回退为 `GetFileInformationByHandle` 与 `FileIdBothDirectoryInfo`，身份标记为 weak |
| 硬链接去重、junction/符号链接边界、中文/空格/长名与超过 260 字符路径 | 单元/集成测试通过 | `cm-platform` 测试 | 卷挂载点与 junction 同为 `IO_REPARSE_TAG_MOUNT_POINT`，统一记为 junction 边界 |
| 压缩/稀疏文件分配量 | 测试通过 | `sparse_compressed_and_alternate_streams_report_measured_allocation` | 通过仅属性句柄的 `FileCompressionInfo` 补查；ADS 不计入逻辑大小 |
| 卷替换与卷离线 | 测试通过（一次性 VHD） | `volume_replaced_during_scan_*`、`volume_going_offline_*` | 需 `CM_ALLOW_VHD_TESTS=1` 与提升 shell |
| 云占位边界 | 测试通过（测试用 Cloud Files 提供方） | `cloud_placeholders_are_never_hydrated_or_populated_by_a_scan` | 扫描期间 FETCH_DATA/FETCH_PLACEHOLDERS 回调为 0；真实 OneDrive 未测，未验证提供方的目录一律记为边界不进入 |
| `win.ntfs_enum`（实验） | unverified | 一次性 NTFS VHD 上与目录基线逐项一致（测试） | 只接受显式卷根与管理员；目录范围请求在读取前拒绝。尚无基准与真实系统卷记录 |
| SMB 共享 | unverified | — | 未连接网络卷 |

### 执行动作（MVP 9.4，`WindowsActions`）

执行前对计划记录的整条父链自上而下逐级以句柄打开并核对身份（持有句柄期间父目录不能被改名或删除），再以仅属性、不跟随链接的方式打开目标，依次检查：缺失、链接/重解析点、身份、对象类型、硬链接数、大小与修改时间；独占打开探测占用，规则要求的 owner 进程以 Toolhelp32 探测，无法映射的 owner 返回 unknown 而非放行。动作作用于同一已核验句柄，执行前关闭 SeBackup/SeRestore 特权。

| 动作 | 结果 | 证据 | 限制 |
| --- | --- | --- | --- |
| delete（同一句柄 POSIX 删除，退回普通删除） | 测试通过 | `cm-platform` actions 测试 | 打开时不给 `FILE_SHARE_DELETE`，他人以删除权限打开的文件记为 `in_use`；多硬链接文件跳过（`unsupported`） |
| quarantine（同卷不覆盖改名） | 测试通过；CLI 端到端通过 | actions 测试、`plan_flow::approved_plan_quarantines_planned_files_once` | 跨卷拒绝（`cross_volume`，一次性 VHD 实测）、目标已存在拒绝（`destination_exists`）；同卷移动不释放数据，报告的空闲差值按实测给出 |
| trash（Shell `IFileOperation` + 进度回调） | 测试通过 | actions 测试 | 网络卷与 `SHQueryRecycleBinW` 失败的卷直接拒绝；`PreDeleteItem` 缺回收标志即中止——在 `\\localhost\C$` 上实测触发中止且文件保留，从不降级为永久删除。回收站按路径操作，最后一次核验与 Shell 打开之间的改名只因结果可恢复而被接受；reconcile 不检查回收站内容，无法确认的回收保持 unknown |
| owner 进程探测 | 实测 | 开发机运行中的 Edge | Edge 运行时 Code Cache 两项均以 `precondition_owner_running` 跳过，操作为 `partial`（退出 5），无文件被移动 |

所有项都因现场前置条件跳过（无失败、无 unknown）时报告 `partial` 而不是 `failed`：没有发生系统错误，只是计划未完成。已提交的计划再次 `plan apply` 时返回已记录的操作报告（同一 `operation_id`），不重复任何动作。

### 只读应用清单（MVP 11.1/11.3）

`cleanmanager apps list` 读取 HKLM（64/32 位视图）与 HKCU 的卸载注册项及 HKCU 的 MSIX 包仓库，全部以 `KEY_READ` 打开；不读取或执行卸载命令字符串，不写注册表。实例 ID 由来源记录与安装位置计算，同名但位置或来源不同的安装保持为独立实例。安装位置按文件身份解析；同一身份、同一路径或互相包含的位置标为共享，不计入任何单个应用的独占空间，未知大小保持未知。没有安装记录或仅名称相似的目录只会被判定为“归属未知”，不会成为清理候选。首版每个实例的卸载能力均为 `unsupported`（只读），卸载、更新与启动项见 `add-mole-parity-suite`。开发机实测：254 个实例（99 MSI、130 MSIX、25 其他），169 个标记为受保护类别，13 个共享位置。MSIX 的间接资源名称尚未解析，显示包名。

