# 清理规则 授权与恢复设计

状态：产品实现要求。本文的授权流程属于未来 CleanManager 产品，并不要求本次文档整理重复征求许可。

## 信任边界

信任入口是本机用户的明确操作、已配置的有限自动化策略，以及经过校验的发行规则集。文件名、路径、CSV、扫描数据库、LLM 建议、前端 IPC 和外部卸载器参数都是需验证的数据。任何一个数据源都不能自行授予执行权限。

普通用户进程可以做只读扫描、生成候选与计划。特权 helper 只暴露固定操作，认证调用者、验证请求范围、限制长度与并发，并在操作现场重新核验对象。扫描提权与写入授权分开。GUI 的 WebView 不直接获得任意文件系统写入和任意 shell 能力。

同用户恶意进程可能修改用户配置或直接调用系统删除文件，产品规则无法单独构成对完整 shell Agent 的安全沙箱。需要更强隔离时使用 OS 隔离与独立授权代理；不得宣传“安装 Skill 后 Agent 就不可能误删”。

## 规则分类与默认行为

| 规则类别 | 首版默认 | 需要证明的条件 |
| --- | --- | --- |
| 已验证应用的可再生缓存 | 可产生低风险建议 | 精确根目录、应用身份、已测试版本、生命周期条件 |
| 系统用户临时目录中的过期文件 | 条件候选 | 已知根、年龄、保护规则、不在使用中或不确定则跳过 |
| 构建产物 | 需审阅；本地可重建输出经实测可默认可选，依赖目录始终仅审阅（`add-mole-parity-suite` project-artifact-purge） | 项目身份、可重建性、版本控制跟踪状态、近 7 天修改；目录名不足以证明 |
| 包管理器下载与依赖缓存 | 需审阅或固定维护适配 | 离线影响、共享依赖、工具版本和副作用范围 |
| 应用日志与崩溃转储 | 需审阅 | 诊断价值、保留期、是否正被写入 |
| 浏览器站点存储与 IndexedDB | 默认不选 | 可能是离线数据与登录状态，不能当普通缓存 |
| Claude/Codex 会话与配置 | 默认保护 | 用户成果、活跃会话、恢复成本与明确选择 |
| Git worktree | 默认保护 | Git 登记、未提交/未跟踪内容、占用与分支关系；无证据不删 |
| AI 模型与大型资源 | 默认不选 | 本地独有数据、来源、重新获取成本 |
| 卸载残留 | G3 能力（`add-mole-parity-suite` app-uninstall），个人数据默认不选 | 包身份、签名/注册信息、共享关系与置信度 |
| 系统修复缓存、凭据、用户文档 | 不进入通用清理规则 | 仅独立且明确支持的维护能力可以触及相应系统范围 |

临时文件不是天然无价值；修改时间也不等于最后使用时间。锁定/占用检测不足以证明可删除，应作为拒绝或跳过条件之一，而非唯一允许条件。规则分开描述收益、风险、置信度和恢复成本。

变更 `add-mole-parity-suite` 把候选统一分为默认可选、仅审阅、拒绝三档，由核心按规则上限、实测登记、占用三态探测、保护与证据完整性取最严结果；GUI、CLI 与 Agent 读同一档位。保护分三层：编译进程序的平台硬拒绝清单、随规则包发布的默认保护类目、用户保护名单；用户名单只能收紧，读取失败时停止全部写操作。占用探测结果为未知时按占用处理。

## 声明式规则字段

固定 ID/version、平台、应用识别、根解析器、匹配条件、年龄、排除条件、操作模式、影响、恢复说明、权限、测试证据和启用状态均需记录。根解析器使用已审核名称，例如系统用户缓存根或应用注册位置；配置中不允许任意可执行脚本。

规则状态为 `experimental → verified → enabled`；未经过真实平台样例验证的规则不能标记 verified。仓库中的 `rules/examples` 仅用于契约验证，不作为已启用生产规则。新规则或版本变化重新计算摘要，既有授权不会无条件扩展。

### rule 2.0 草案

[rule-v2 Schema](../schemas/v1/rule-v2.schema.json) 在 1.0 字段之上增加：`category`（15 个共享分类）、`max_tier`（默认可选 / 仅审阅 / 拒绝的上限）、`rebuild_class`（本地可重建 / 需联网重新下载 / 不可重建）、`owner.kind`（app、tool、system）、扩展的根解析器枚举、受限 `match`（解析根内的字面路径分量、单段通配、扩展名、缓存目录签名、压缩包载荷、最大深度 ≤ 8）、`probes`（占用三态探测）、`thresholds`（年龄基准只允许修改/创建时间）、`action_kind`（filesystem、tool_adapter、review_only_row）与 `adapter_ref`、`requires_admin`、`tested_registry_ref`。

Schema 与验证器共同保证：

- 没有正则、脚本或任意可执行字段；路径分量与通配不能含分隔符、`..` 或 `**`；`follow_links` 恒为 false。
- 需联网重新下载或不可重建的对象，档位上限只能是仅审阅或拒绝；`blocked` 与仅审阅行没有处理模式；`tool_adapter` 必须引用适配器且不带文件处理模式。
- `enabled` 规则必须引用[实测登记](../schemas/v1/tested-registry.schema.json)条目，该条目针对同一规则版本、结果为 passed、不是合成样例，并覆盖规则声明的全部平台。

`rules/examples/v2` 与 `rules/registry/examples` 是合成样例，规则恒为 experimental，登记标记 `synthetic: true`，不能支撑任何启用规则。

### 规则作者说明

1. **写规则**：使用 rule 2.0（1.0 文件仍可加载并在内部转换）。根只能是枚举中的解析器，路径分量是字面名字，通配只作用于单个路径段；不允许正则、脚本、命令或 `follow_links`。新规则从 `experimental` 开始。
2. **本地校验**：`python tools/validate_docs.py` 校验 Schema 与正反例；`cargo test -p cm-core rules` 覆盖 Rust 加载器（未知字段、脚本解析器、`..`、`**`、跟随链接都会被拒绝）。
3. **晋级**：`verified` 需要 `evidence_refs`；`enabled` 还需要 `tested_registry_ref` 指向同一规则版本、`result: passed`、非合成且覆盖规则全部平台的实测登记。加载器在构建规则集时检查，失败则整个规则集不可用，而不是静默跳过。
4. **分析结果**：只有 enabled 且平台匹配的规则产生候选。规则根按文件身份（卷 + 文件 ID）匹配扫描条目，不按路径文本；内置保护类目（凭据、数据库及其 WAL/SHM、浏览器站点存储、AI 会话/配置、Git 工作树、模型权重、身份解析出的用户文档与系统修复目录）优先于任何规则，受保护或含受保护后代的对象为 `blocked`（AI 会话与干净 worktree 根为 `protected_reviewable`，见“保护策略”），含未读后代或时间未知为 `pending_evidence`。同一对象只产生一个候选；硬链接多名字降为仅审阅。
5. **版本**：修改匹配、阈值、档位或影响都要升版本；规则集摘要随之改变，旧授权不会自动覆盖新版本。

### 首批启用规则（Windows，2026-10-15）

| 规则 | 档位 | 实测范围 | 证据 |
| --- | --- | --- | --- |
| `microsoft.edge.code-cache@1.0.0` | 默认可选（本地可重建） | Edge 155.x，Windows 11 26200 x64 | `run_win_rule_microsoft_edge_code_cache`：一次性 headless 配置档生成 `Code Cache`，只匹配该目录，删除后重新生成且页面正常 |
| `google.chrome.code-cache@1.0.0` | 默认可选（本地可重建） | Chrome 155.x，同上 | `run_win_rule_google_chrome_code_cache` |
| `windows.user-temp.stale-files@1.0.0` | 仅审阅 | Windows 11 build ≥ 22000 | `run_win_rule_user_temp_stale`：只匹配顶层 7 天前文件；旧数据库被阻止，新文件与子目录文件不匹配 |

以上验证使用一次性浏览器配置档与受控 TEMP 夹具，没有扫描或修改真实用户配置档。版本区间外的安装不会使用这些登记（应按 CAT-03 降为仅审阅，待对应实现）；浏览器运行中由执行期 `owner_process` 探测跳过。macOS 尚无启用规则。

发现候选后跨类别去重：一个父目录和它的子文件不能分别加总收益，也不能让低风险父目录覆盖高风险子项。保护清单优先于允许规则；遇到受保护后代时细分具体对象或放弃父目录操作。

## 保护策略（三层）

变更 `add-mole-parity-suite` 的 `protection-policy`（PRT-01～06）。三层按强弱顺序判定，取第一个拒绝；下层永远不能解除上层，用户名单只有“保护”条目、没有“放行”条目：

| 优先级 | 层 | 内容 | 能否移除 |
| --- | --- | --- | --- |
| 1 | 硬拒绝（`hard_deny`，版本 `hard-deny/3`） | 文本预过滤（相对路径、`..`、控制/格式字符、设备与管理共享路径、ADS、保留名、系统目录表）＋按“卷 + 文件身份”比较的平台根：Windows 系统目录、Program Files、ProgramData 根、用户根与各用户主目录根、凭据与 Vault、卷根与卷元数据、回收站根、Recovery、Config.Msi、终端安全软件数据、本工具存储与各卷隔离区（`.cleanmanager-quarantine`）；macOS `/System`、`/bin`、`/sbin`、`/usr`（`/usr/local` 之下除外）、`/private/etc`、`/private/var/db`、`/Library/Apple`、钥匙串、TCC、`/Library/SystemExtensions`、`/Library/Updates`、`/Users` 与各主目录根（仅本身）、卷根与卷元数据、废纸篓根、本工具存储。对象或任一祖先是链接/junction/挂载点时，再用解析后的真实位置检查；解析失败即拒绝 | 不能 |
| 2 | 默认保护类目（`default_category`，版本 `protection/2`） | 凭据、数据库及 WAL/SHM、浏览器站点存储、AI 会话/配置、Git worktree、模型权重、身份解析出的用户文档与系统修复目录（MVP ANA-03） | 不能 |
| 3 | 用户保护名单（`user`） | `exact`（对象本身）、`descendants`（路径及其下全部，`--path` 默认）、`ancestors`（路径及其所有上级目录）、`glob`（单段 `*`/`?` 与整段 `**`，根必须是字面量）、`rule`（某规则的全部动作）、`maintenance_task`（某维护任务/启动项） | 仅交互终端 |

判定落到候选上时：

- 硬拒绝、用户条目、受保护规则命中的候选为 `blocked`，理由分别为 `hard_deny:…`、`user_protected:<条目>`、`user_protected_rule:<条目>`；候选不会被静默丢弃。删除整棵目录时，包含受保护对象的上级目录同样被拒绝。
- 默认类目一般为 `blocked`（`protected:<类目>`）。AI 会话（`.claude`/`.codex` 之内的对象）与干净的 worktree 根为 `protected_reviewable`：档位仅审阅、不进入 `--all-default-selected`、不被策略匹配，普通 `plan create` 也拒绝，只能由独立的逐对象人工确认流程生成计划（CAT-04）。AI 目录本身、worktree 内部对象、包含会话或 worktree 的上级目录为 `blocked`；worktree 有未提交/未跟踪修改为 `worktree_dirty`，Git 状态未知为 `worktree_status_unknown`，两者都是 `blocked`（当前尚无只读 Git 探测，worktree 根一律按未知处理）。
- 用户名单存于存储目录的 `protection.json`（`schema_version: 1`，原子替换写入，CLI 与桌面端共用并通过锁文件串行写入）。文件缺失视为空名单；无法读取、解析失败、版本不符或含无效条目（相对路径、穿越、指向硬拒绝位置）时整份名单不可用：`protect add/remove`、`plan apply` 与 `protect list`（无法列出用户条目）返回 `protection_unavailable`（退出码 8），并给出文件位置供修复或移走；`scan`、`analyze`、`protect check` 等只读命令继续；`analyze` 的候选全部为 `pending_evidence` 并带 `user_protection_unavailable`。
- 执行前（`plan apply`）对每个动作重新按三层检查；计划生成后新加的保护使尚未执行的对象成为 skipped（受保护），已完成的不回滚。

示例（`--store` 可省略，默认使用本机存储目录）：

```text
cleanmanager protect list --json
cleanmanager protect add --path "C:\Users\me\Projects\keep-cache" --note "offline build cache" --json
cleanmanager protect add --glob "C:\Users\me\AppData\Local\Temp\keep-*" --json
cleanmanager protect add --rule google.chrome.code-cache --json
cleanmanager protect check --path "C:\Users\me\Projects\keep-cache\index" --json
cleanmanager protect check --path "C:\Windows\Temp" --json
cleanmanager protect remove u_1
```

`protect check` 输出 `protection_check` 记录，按顺序列出三层各自的结果（`clear`/`denied`/`unavailable`）与理由，`decision` 取第一个非 `clear` 的层；只检查对象本身与上级，不遍历其后代。`protect add --path` 在保存前先做文本校验，再按身份检查：指向硬拒绝位置（包括大小写变体与链接别名）的条目被拒绝（`invalid_argument`）。新增保护只会收紧范围，机器调用者可以执行；`protect remove` 会放宽范围，必须在交互终端中输入条目 ID 确认，非交互调用返回 `invalid_argument`。硬拒绝（`hd_*`）与默认类目（`dc_*`）条目只列出，不能移除。

规则作者：规则不能声明例外来越过任一层；需要处理默认保护类目中的对象时，应走对应能力的逐对象审阅流程，而不是放宽规则。新增或修改硬拒绝表、类目时须升级 `hard-deny/*` 或 `protection/*` 版本，候选集标识随之改变。

## 计划完整性与授权

计划先展开为有限的固定对象集合，绑定 scope、扫描 revision、规则集摘要、策略摘要、操作模式、对象身份、前置条件与到期时间。禁止在执行时重新展开宽泛 glob 或递归吞入新增文件。

计划摘要使用 `CM-C14N-v1`：对仅含字符串、整数、布尔、null、数组和 ASCII 字段名的 PlanBody 进行紧凑 UTF-8 JSON 编码，键按字典序排列，不归一化字符串、不转义普通非 ASCII 字符，不允许浮点数、重复键或未知字段；摘要为 SHA-256。数组按协议固定顺序，动作按 action_id 排序。此规范是项目自己的有界序列化规则，不冒充完整 RFC 8785 实现。

摘要不证明用户同意。授权回执单独绑定 plan digest、主体、来源、允许动作、规模上限、有效期和撤销信息。交互确认展示具体内容；自动策略必须由用户预先配置，并限制目录、规则版本范围、风险、大小/数量和模式。执行器重新验证策略有效性，不能只相信传入的 `approved: true`。

CLI 的 `--non-interactive` 只禁止提示；`--yes` 即使将来提供，也只跳过已授权操作的重复提示，不能授予更大权限。Agent 不获得绕过策略的任意 approve 或 delete-path 工具。

### 已实现的授权模型（MVP 组 8）

- **计划**：`plan create` 只接受 `allowed` 且模式在允许范围内的候选；目录候选展开为当时观察到的文件，目录本身从不被处理。每项绑定 CM-OBS-v1 元数据指纹；计划默认 900 秒有效，到期瞬间即失效，规则集摘要变化视为过期。计划与回执存于 `audit.db`，读取时重算摘要，不一致即拒绝。
- **人工审阅**：`plan review ID` 只在 stdin 与 stdout 都是终端时可用，展示模式、对象、规则、到期与摘要，要求输入摘要前 8 位。生成的回执绑定计划 ID、摘要、模式、主体（OS 用户）与计划到期时间，可用 `plan revoke` 撤销。非交互调用直接返回参数错误。
- **有限策略**：`policy create` 只能创建暂停状态的策略（默认模板：30 天、单次 1 GiB/1,000 项、滚动 24 小时 2 GiB/2,000 项）；`policy enable` 需要终端并输入策略 ID；暂停、撤销只收紧权限，可由任何入口执行，撤销后不可再启用。策略只覆盖精确 scope、精确 `rule@version`、同一模式、全部为默认可选档位且大小全部已知的计划。
- **额度**：使用 SQLite `BEGIN IMMEDIATE` 在读取用量前取得写锁，跨进程原子保留；同一操作不重复保留；已成功的量计为消耗，未知结果的量保持占用直到核对，其余释放。
- **威胁边界**：TTY 只是呈现控制，不证明操作者是人；同一用户下能驱动控制台或直接修改用户数据库的进程不在本工具的隔离范围内（PLN-07）。产品规则约束的是本工具的入口：没有接受 `approved` 字段、模型文本或提权状态作为授权的接口。
- **特权 helper**：MVP 不安装任何特权 helper，全部动作以当前用户权限执行；CLI 启动即禁用令牌中的 SeBackupPrivilege/SeRestorePrivilege。核心与 CLI 不存在执行任意命令、脚本或开放递归删除的入口（EXE-08）。需要特权的能力（系统级维护、风扇等）属于 `add-mole-parity-suite` 的 G3/G4，需独立的受限 helper 设计与验收。Windows 系统维护的管理员任务已改由一次 UAC 启动的短生命周期工作进程执行，只接受已审阅维护计划摘要内的固定任务 ID（见 [提权边界](privilege-boundary.md) 与 [系统维护](system-maintenance.md)）；macOS 特权 helper 仍为规划。

## 执行状态机

1. 解析并校验计划格式、摘要、授权、有效期、平台与 scope。
2. 获取跨进程写锁，拒绝与其他操作的范围冲突。
3. 对逐项写入 durable intent，再重新打开并核验目标对象、类型、大小/相关时间、链接与祖先范围。
4. 检查保护规则更新、卷替换、对象重用、云召回、挂载边界和权限变化。
5. 使用平台最可靠的句柄或相对路径机制执行受限操作。仅 canonicalize 再按路径删除仍有竞态，不能当充分保护。
6. 持久化逐项结果；记录回收站/隔离对象定位信息、失败原因和可恢复能力。
7. 汇总实际完成项、跳过项与失败项，另行记录卷空闲空间观测。

规则或前置条件改变返回 stale/skip，不自动替换为更大范围的新计划。批次部分成功不能返回“全部完成”。崩溃后先核对 intent 与实际对象，不直接重试。请求去重键对应同一个已登记 Operation；参数不同的同 key 请求失败。

## 隔离 回收站与恢复

同卷隔离采用受限私有目录与唯一名称，不能向用户目录任意放置备份。记录原目录身份、原名、对象身份和元数据；源路径重新出现新对象时禁止默认覆盖。跨卷移动是复制与删除两阶段，需额外验证，不沿用同卷原子 rename 的承诺。

回收站支持和恢复能力按平台、卷类型、权限逐项探测。失败时返回错误，不能无提示降级为永久删除。用户不能以普通权限恢复特权处理项时，提供受限恢复流程，而非启动整个界面为管理员。

隔离容量上限和保留时间是可配置策略；到期永久清除是另一个有授权的计划，不能静默新增。磁盘已满时，备份操作可能不可行，必须预检并给出明确结果。仅把文件移到同卷新目录通常不会腾出空间。

永久删除没有通用撤销。硬链接、快照、压缩、延迟回收与同时写入都会影响释放空间，报告分别保存操作对象统计与卷空闲差值。恢复日志不应承诺完整保留所有 ACL、ADS、扩展属性或稀疏结构，具体能力以平台验证为准。

### 已实现的 Windows 行为（MVP 10.1–10.3）

- **隔离位置：** 存储目录与目标同卷时为 `<store>\quarantine\<operation_id>\<action_id>`，否则为 `<卷根>\.cleanmanager-quarantine\<operation_id>\<action_id>`；跨卷时拒绝（`cross_volume`），从不复制后删除。两处都属于硬拒绝（本工具存储按身份，`.cleanmanager-quarantine` 另有文本与身份两层），因此清理计划、候选与动作都不会指向隔离内容；隔离内容只能经恢复计划处理。
- **持久清单：** 每个隔离成功的对象在审计库（不随索引重建删除）记录原路径、对象身份、原父目录身份、隔离位置、逻辑大小、隔离时间与提醒时间（默认 30 天）。缺原父目录身份、身份不是强身份时显示为 `manifest_incomplete`、`restore_capability: unsupported`，不能建立恢复/清空计划；记录本身不可解析时列表报审计错误，而不是显示为可恢复。`recovery list` 在每次查询时实时探测：`recoverable`、`target_conflict`、`volume_offline`、`volume_changed`、`identity_changed`、`missing`、`parent_changed`、`unknown`。
- **元数据（经测试验证）：** 同卷 rename 移动的是同一 NTFS 文件记录，往返后文件 ID、创建/修改时间、属性（如隐藏）、显式 ACL 与备用数据流保持不变（`metadata_restored: timestamps, attributes, acl, alternate_streams`）。限制：每次移动更新 NTFS 变更时间；不应用隔离目录可继承的权限；扩展属性随文件记录移动但未经测试，因此不声称 `xattrs`。
- **恢复：** 独立 `recovery/v1` 计划，只接受对该计划摘要的人工审阅授权（清理批准、旧批准或其他类型批准都不生效，策略不匹配）。执行时以不含 `FILE_SHARE_DELETE` 的句柄核验隔离对象，固定原父目录并核对其身份，然后以 `NtSetInformationFile` 在**原父目录句柄下**按原名不替换地 rename；原名已有任何对象即 `destination_exists`，两者保留，不改名、不覆盖。卷离线或换卷时清单保留，不恢复到同盘符的新卷。成功以两个句柄的最终路径确认，无法确认即 `unknown` 并进入核对。
- **清空：** 另一份 `recovery/v1`（`purge`）计划与单独授权；只删除核验通过且逻辑大小未变的对象，空的隔离目录保留。保留期到期只把 `reminder_due` 置为 true，不删除任何内容。
- **目录项（构建产物，`add-mole-parity-suite` 4.x）：** 清理计划默认只含文件；仅内置构建产物目录（规则前缀 `purge.`）在 trash/delete 模式下额外冻结观察到的每个目录为 `kind: directory` 计划项（纳入摘要）。执行器先完成全部文件项，再自底向上处理目录：其内任一计划项未成功即 `precondition_descendant_kept`；否则以不含 `FILE_SHARE_DELETE` 的 `DELETE|FILE_LIST_DIRECTORY` 句柄打开（固定并核对祖先链、`FILE_FLAG_OPEN_REPARSE_POINT`，链接/联接只检查不跟随），核对卷+文件 ID，在**同一句柄**上用 `FileIdBothDirectoryInfo` 确认为空，再以 POSIX 语义置删除；列出之后新出现的条目会使文件系统拒绝置删（`ERROR_DIR_NOT_EMPTY`），置删后也无法再在其中创建，因此从不递归、不会卷入新文件（`precondition_new_descendant`）。trash 模式可先尝试整目录进回收站：现场子树须与计划逐项一致（身份、类型、大小、时间、单一硬链接、无链接、无额外项），Shell 移动后再按身份核对回收站内子树，不一致记 `unknown`；任何拒绝都退回逐文件 + 空目录删除。从回收站还原文件时 Explorer 会重建父目录。quarantine 模式不含目录项，因为隔离恢复需要原父目录（身份）仍在，代价是空目录骨架保留。
- **构建产物与安装包的执行前检查：** 构建产物计划在每个产物首个动作前重读 Git 索引（不运行 git；`.git` 文件形式的 worktree/子模块会解析到实际 gitdir；split index、未知版本、截断或 hg/svn 工作副本为“未知”），出现已跟踪文件或无法判断即该产物全部动作以 `protected` 跳过。安装包计划按文件身份绑定，被同名新下载替换即 `precondition_identity_changed`；挂载中的镜像与被其他进程打开的文件在现场 in-use 探测中跳过（预览阶段已标为 mounted/in_use，并给出推出指引）。
- **手选移入回收站与启动项（`add-mole-parity-suite` 6.x、11.2–11.3，plan/v2）：** `manual_selection` 只接受已发布扫描的条目 ID，动作只有原位 `fs_trash`（回收站内对象的 `fs_delete` 是另一份计划，审阅时另需输入 `DELETE` 确认不可恢复）；硬拒绝、默认类目、用户名单命中对象、祖先或任一已知后代即只读，未完整读取的目录整体拒绝，云占位拒绝，卷无回收站时建计划即失败、从不降级为永久删除。执行时文件以句柄核验后经 Shell 移入回收站并以同一句柄确认落点，目录只在现场子树与计划绑定成员一致时整体移动；名称被替换或无法确认记 `unknown` 并给出放回指引；被占用为 `failed`（`in_use`）并列出 Restart Manager 报告的占用进程，从不结束进程。启动项只写任务管理器所用的启用记录，变更前先把原状态写入审计存储；失效 `Run` 值仅在备份写入成功后删除；恢复是独立 `recovery` 计划，从不覆盖之后被改动或重新创建的值。两类计划都不可被策略匹配，只能由人工审阅授权。详见 [手选移入回收站](manual-trash.md) 与 [启动项](startup-items.md)。
- **应用卸载（`add-mole-parity-suite` 9.x，plan/v2 `uninstall`）：** 两段计划分别审阅：第一段只含退出请求与一个按来源选择的 `tool_adapter`（只取该实例注册项中的卸载命令，或系统 `msiexec /x`、包管理器），建计划与执行前都核验程序路径、哈希与 Authenticode 签名者与注册发布者一致，不一致或无签名为 `policy_denied`；应用不退出或运行状态未知时卸载程序不运行，强制结束只来自另行审阅的计划并在执行时由人确认；执行后重扫，仍有剩余即 `partial`。残留须有身份证据（通用词不匹配），共享/归属不明/已重装的对象保留，只预选缓存与日志；注册表键先完整备份到审计存储再删除，备份失败即不删，恢复从不覆盖重新创建的键。未知来源只给能力说明与系统设置入口，从不删除安装目录冒充卸载。详见 [应用卸载](app-uninstall.md)。
- **历史与空间口径：** `operations list` 给出每次操作的计划类型、授权来源、规则版本、计数与恢复入口：隔离为产品内恢复、回收站只给系统回收站指引、永久删除与清空明确“无撤销”、结果未确认先核对。报告区分处理量、隔离量（同卷移动，不计释放）、仅删除/清空计入的估计释放量与实测卷空闲差值（可为负，原样报告）。

## 更新与审计

规则包必须带版本、摘要、适用引擎范围与来源验证；引擎保留当前与上一版可回退规则。更新不能自动扩大旧策略授权。应用自身更新需签名校验，失败保持已安装可用版本。

审计记录仅保留完成任务所需的身份、规则、理由、时间、结果和恢复定位；默认不保存文件内容。保留期和脱敏选项独立于实际隔离区保留期。导出给 Agent 的文件名同样可能敏感，默认摘要与按需展开。

首版必须覆盖的误删场景见 [验证矩阵](verification-plan.md)。disk-cleaner 的合成输入发现与 REA 证据见 [调查记录](research/rea-investigation.md)。
