# Design

## Context

动机见 [proposal](proposal.md)，Mole 逐项来源与差异见 [对标证据](evidence.md)。本变更建立在进行中的 `define-cleanmanager-mvp` 之上：其 Rust 核心、扫描索引、固定计划（CM-C14N-v1 摘要、900 秒 TTL）、授权/有限策略、durable intent→result 执行器、隔离恢复与 CLI 协议是所有新写动作的唯一通道。当前仍无产品代码；`schemas/v1/rule.schema.json` 只能表达 `application_cache/user_temp/project_build_output` 三种根且强制单一 `owner_app_id`，不足以承载 Mole 规模的目录。

已知约束：

- Mole CLI（GPL-3.0）只作行为参考，不复制源码；Mole GUI 为闭源，仅有官网文字证据；Mole CLI `windows` 分支（v1.34.0）已静态阅读，其能力明显窄于官网描述的 Windows GUI（见 evidence.md W1–W19）。
- MVP 文档中有与本变更冲突的表述（磁盘页不处理未命中规则对象、卸载/维护/状态/风扇为后续、应用页只读、AI 会话一律保护），需在归档前同步（任务 0）。
- 用户已决定：对标 Mole 全部功能（含卸载、维护、状态、常驻工具），磁盘分析手选对象只允许移入回收站。

## Goals / Non-Goals

**Goals:**

- 对标矩阵中每项 Mole 功能都有本变更的规格归属或写明的不采用理由。
- 所有新增写入口复用 MVP 的计划/授权/核验/日志/恢复，不新增旁路。
- GUI、CLI、Agent 看到同一候选与同一档位，修正 Mole “CLI 更宽、GUI 更窄”的分叉。
- 能力按平台、硬件、权限逐项声明，未验证即不可用。

**Non-Goals:**

- 不提供注册表优化、服务禁用、系统精简、内存释放、隐私痕迹重置或“加速百分比”。
- 不运行任意 shell/脚本规则，不接受模型或用户提供的命令参数。
- 不做授权码/试用/计费；不做远程规则订阅（规则随产品版本发布）。
- 不在 Windows 首批提供风扇控制（缺统一可验证接口）；不做按厂商前缀批量匹配残留。

## Decisions

### D1. 新写动作全部是带类型动作的固定计划

在 MVP PlanBody 中引入动作类型而非新的执行器：`fs_trash`、`fs_quarantine`、`fs_delete`（既有）、`tool_adapter`（所属工具/包管理器/卸载器的固定调用）、`registry_backup_delete`、`startup_toggle`、`maintenance_task`、`settings_change`（亮屏/Touch ID/电池等可逆设置），以及实现时补充的 `app_quit`（UNI-02，强制结束只能是 `ask_user`）与 `registry_restore`（UNI-06，目标已存在时跳过）。计划种类与允许动作（契约 `plan/v2`）：`cleanup`（文件三动作 + tool_adapter，唯一可被有限策略匹配的种类）、`manual_selection`（原位 fs_trash，或仅对回收站内对象 fs_delete）、`uninstall`（app_quit、tool_adapter、fs_trash、fs_quarantine、registry_backup_delete、startup_toggle）、`maintenance`（maintenance_task、settings_change）、`startup`（startup_toggle）、`update`（app_quit、tool_adapter，用于 UPD-02 的包管理器/商店/签名校验更新）、`recovery`（registry_restore、startup_toggle、settings_change；隔离区恢复仍用 MVP 的 recovery/v1）。`requires_admin` 必须与动作（需管理员的任务、HKLM、系统级启动项）一致。

依据：沿用摘要、授权绑定、去重、互斥与 reconciliation 语义，避免为卸载/维护各写一套执行器。`tool_adapter` 与 `maintenance_task` 不是精确文件动作，结果记录外部退出码、前后观察和“由外部工具执行”的说明，默认不允许预授权匹配；策略只有显式列出适配 ID/版本时才可匹配。替代方案是给卸载/维护单独的“命令执行通道”，会绕开计划摘要与跨入口交接，不采用。

### D2. 规则 Schema v2：可表达目录，但仍是声明式

新草案 `rule/2.0` 增加：`category`（共享分类：system_data、user_essentials、trash、app_caches、browsers、cloud_office、developer、ai_tools、virtualization、logs、leftovers、firmware_backups、incomplete_downloads、large_items、removable_metadata）、`max_tier`、`rebuild_class`（local_rebuild / network_redownload / not_rebuildable）、扩展的根解析器枚举（系统缓存/日志、用户缓存/日志、浏览器配置档、应用容器、包管理器缓存、项目产物、安装包位置、回收站、可移动卷）、`owner` 改为 `{kind: app|tool|system, id}`、受限匹配器（解析根内的名称/有限 glob、扩展名、缓存目录签名、压缩包载荷）、`probes[]`（CAT-06 的探测类型）、`thresholds`、`tested_registry_ref`、`adapter_ref`。仍禁止脚本、任意正则跨根和 follow_links。

实测登记（CAT-03）是独立数据文件：应用/工具、版本区间、平台/系统版本、测试日期、正反例夹具 ID、证据链接。规则 enabled 要求登记存在且覆盖当前版本，否则档位降为 review_only。规则包随产品发布并纳入签名，不做在线订阅。

替代方案是逐条移植 Mole shell 段：覆盖快，但把归属/影响/探测散落在命令里，无法给出同一解释和档位，不采用。

### D3. 档位由核心计算，入口只呈现

`tier = min(rule.max_tier, registry_tier, probe_tier, protection_tier, evidence_tier)`：版本未登记 → 最多 review_only；任一探测 in_use/unknown → 跳过；硬拒绝或保护 → blocked；部分扫描缺必要证据 → review_only 或拒绝（MVP ANA-05）。依赖类目（node_modules、Pods、venv、vendor、包管理器包存储）固定 `max_tier = review_only`，采纳 Mole Mac App 的 “same-or-safer” 结论；不同之处是 CLI 也使用同一结果。

### D4. 首批目录分波推进

共享分类先定义，再按证据分波启用：

- **第 1 波（G1）**：用户临时与日志、回收站/废纸篓（清空为永久删除计划）、Chrome/Edge/Firefox/Safari 代码与 GPU 缓存、npm/pnpm/yarn/pip/uv/Cargo registry cache/Go build cache/Gradle、Xcode DerivedData、VS Code 系编辑器缓存、Claude/Codex/Cursor 缓存与被替换的旧版本、Windows 错误报告与用户级崩溃转储、Delivery Optimization（经系统接口的 `tool_adapter`）、未完成下载。
- **第 2 波（G2）**：通讯/设计/媒体/游戏应用缓存、云与办公、虚拟化、模拟器运行时（`tool_adapter`）、Homebrew/winget/Scoop 缓存、系统级日志与 NVIDIA 安装缓存（需管理员）、外接卷元数据。
- **后续波**：按实测登记扩充，目标量级参考 Mole 官网所述“200 多类”。

“有意保留”清单（Maven/NuGet 包存储、Cargo src/git、模型缓存等）直接作为 blocked 或 large_items 提示，避免误入。

### D5. 保护策略分三层

1) 编译进二进制的**平台硬拒绝清单**（版本化，按卷+文件身份比较，祖先链接只拒不放）；2) 随规则包发布的**默认保护类目**（凭据、文档、数据库、浏览器站点存储、AI 会话/配置、worktree 等，即 MVP ANA-03）；3) 用户**保护名单**（独立版本化文件，原子写入，读失败即停止写操作）。用户名单只能收紧，不能删除前两层；与 Mole 不同，保存用户名单不会替换默认保护。

AI 会话/worktree 的处理：默认保护层对它们产生 `protected_reviewable` 状态——不进入自动候选或策略，仅在 AI 工具类目中逐对象人工确认后生成计划；worktree 有未提交/未跟踪内容则升级为 blocked。这与 MVP“保护优先于允许规则”一致，因为人工逐对象确认不是允许规则。

危险路径语料复用 Mole 测试思路（系统根、穿越、控制字符、大小写/Unicode、链接）并新增 Windows 设备路径、UNC、短文件名与 ADS 变体，作为执行器与规则变更的必过回归。

### D6. 手选回收站是独立、不可预授权的计划种类

`manual_selection` 只允许 `fs_trash`。Windows 用 Shell 文件操作接口的回收站移动，macOS 用系统 trash 接口。这些接口按路径工作，存在检查到移动之间的名称竞态；对**单个用户手选、可从回收站恢复**的对象，采用“移动前核验身份 + 移动后核验回收站中对象身份”的方式接受该残余风险，核验不一致时结果为 unknown 并引导用户在回收站中确认/放回。此例外仅适用于 `manual_selection` 与回收站动作，不放宽 MVP 对自动规则执行的严格身份要求。

回收站内永久删除是单独的 `manual_selection` 变体（`fs_delete`，目标限于回收站位置），需不可恢复确认。占用进程查询：Windows Restart Manager，macOS 在可用时查询打开文件的进程，失败显示未知；只读，不结束进程。

### D7. 卸载是两段计划

- **macOS**：一个 `uninstall` 计划包含退出应用、本体移入废纸篓、残留分组动作（默认仅缓存/日志选中）、launchd/登录项注销、应用注册缓存刷新。Homebrew 安装的走 `tool_adapter`（含 zap 时先展示其列出的路径）。
- **Windows**：第一段计划执行退出与供应商卸载器/MSI/MSIX/winget/Scoop 适配；完成后核心重新扫描本体与残留，第二段生成残留计划（文件移回收站、快捷方式、注册表 `registry_backup_delete`）供审阅。第一段审阅时显示“预计残留”为估计，不预先授权第二段。

依据：Windows 卸载器会自行删除/新增文件，事先冻结残留清单会过期；拆成两段保持“计划对象固定”。注册表备份存入恢复存储（与审计同等保留，不随索引重建删除），恢复以 `recovery` 计划执行并先检查目标键是否被重新创建。

残留发现按身份证据：bundle ID/包家族名/产品码/卸载注册项/发布者签名/已知数据目录约定；名称变体排除通用词；同身份兄弟实例与测试版使残留为 shared_owner。孤立残留（UNI-08）要求所有已挂载应用位置与安装记录都确认缺席，闲置期默认 30 天。

### D8. 更新只走可验证通道

通道适配：Sparkle（appcast + EdDSA 签名，签名者须与已安装一致）、App Store/商店（打开系统更新或调用系统支持的更新接口）、Homebrew/winget/Scoop（固定 argv 的 `tool_adapter`）、Electron 更新源（仅检测；执行仅在能校验代码签名与发布者一致时，否则打开应用自身）。其他来源只打开官方页面。检测由用户触发或在用户开启的周期内运行，只发送通道所需标识。

### D9. 启动项优先使用平台的“可逆停用”

Windows Run/启动文件夹条目使用系统自身的启用/停用记录（与任务管理器一致），不删除定义即可停用；计划任务通过任务计划程序接口启停。macOS 登录/后台项使用 Service Management 状态接口，用户 launchd 作业通过 launchctl 停用并备份 plist。失效项删除前备份定义。这样大部分启停天然可恢复，恢复记录进入活动历史。

### D10. 维护任务是代码化目录 + 一次性受限提权

维护任务需要调用系统接口，不适合声明式规则，因此每个任务是核心内的固定实现，包含 ID/版本、适用性探测、动作和六种结果映射。提权：Windows 在运行开始时以 UAC 启动一次**短生命周期**的提权工作进程，经 ACL 限定的本地管道只接受本次计划内的固定任务 ID，运行结束即退出；macOS 通过签名的特权 helper（与 D12 同一 helper，命令集分组授权）。拒绝提权时这些任务为 skipped。

macOS 首批：快速预览/图标缓存刷新、DNS 刷新、Spotlight 状态检查、损坏偏好检测（只报告/隔离）、过期保存状态、磁盘校验（只读）、登录项审计与失效 LaunchAgent 报告。Windows 首批：重启开始菜单与搜索宿主、图标/缩略图缓存刷新、DNS 刷新、到期 TRIM（先读上次优化时间）、系统映像健康检查（只读 CheckHealth/ScanHealth，不自动修复）、失效启动项（转 startup-items）。重建全部 LaunchServices 数据库、注册表调优、服务停用永久列入禁止清单（MNT-05）。

### D11. 状态采样器与健康分

核心内的采样器按平台收集公开接口指标：Windows 性能计数器/系统信息接口、GPU 引擎计数器、存储可靠性计数器；macOS host/process 统计、IOKit 电池与传感器（无 root 可得部分）、磁盘可清除空间。需要 root 的指标（如功耗）只在 helper 已安装且用户允许时提供，否则 unavailable。健康分采用公开权重（CPU 30、内存 25、磁盘 20、温度 15、I/O 10）与阈值，作为本产品可调设计值；诊断行优先级见 STS-03。进程解释来自本地知识表（进程名+签名+路径）与签名信息，未知时提供 AI 诊断入口。CLI 提供快照与 JSONL 流。

### D12. 常驻组件与特权 helper 分离

常驻组件是独立的轻量 Rust 托盘/菜单栏进程，按需创建小面板 WebView，只读取核心状态与发送提醒，不执行清理写动作；仅在用户开启时注册为登录项。保持亮屏用系统电源断言（可设时长、退出即释放），清洁屏幕用全屏遮罩与输入拦截并带超时退出，隐私提示与蓝牙电量读取系统公开状态。

特权 helper（macOS 以 Service Management 注册的签名守护进程）只接受固定命令集：风扇模式、电池充电上限、网络低延迟、维护任务分组；校验调用者签名与版本，验证硬件型号在已验证列表；异常温度、helper 卸载或产品退出时恢复系统自动控制。Touch ID 配置修改在用户授权的提权下进行，先备份并验证配置语法，失败回滚。Windows 首批不提供风扇/电池写入（缺统一接口），显示不支持。

### D13. 产品生命周期

桌面使用 Tauri 更新器签名校验；CLI 自更新读取签名的发布清单（签名+SHA-256），单实例锁，包管理器安装的副本只提示对应升级方式。自卸载默认保留配置/保护名单/历史/隔离区，隔离区非空时先提示。Doctor 报告复用 capabilities 与存储自检，默认脱敏并预览后导出。界面文本采用资源文件本地化（简体中文、英文），机器字段不本地化。

### D14. 统一信息架构与视觉对齐

| Mole 标签 | 本产品一级工具 | 包含能力 | MVP 页面映射 |
| --- | --- | --- | --- |
| Clean | 清理 | cleanup-catalog、project-artifact-purge、installer-cleanup、回收站 | P04 |
| Software | 软件 | application-inventory、app-uninstall、app-updates、startup-items | P07 |
| Optimize | 优化 | system-maintenance、性能诊断 | 新增 |
| Analyze | 分析 | disk-index-query、manual-trash、disk-insights | P02/P03 |
| Status | 状态 | system-status、resident-utilities 入口 | 新增 |
| — | 扩展：计划/活动/恢复/Agent/AI 诊断/设置 | operation-plans、recovery-history、agent-cli、ai-cleanup-diagnostics | P05/P06/P08/P09/P10 |

视觉：沿用 suite-v3 原型已建立的设计令牌、留白、原位展开、侧栏与底部选择栏；三档徽标、结果摘要、能力说明使用统一组件。可借鉴 Mole 的“五工具一个入口”和“先看后删”节奏，但不使用其星球/动物形象、命名或文案；本产品主视觉另定原创图形（任务 16.1）。原型从 v5 升级到 v6：去掉“扩展概念”标记，改为按本变更能力与门禁状态展示（已规格/实验/不可用）。

### D15. 门禁扩展

| 门禁 | 新增内容 |
| --- | --- |
| G1（核心/CLI） | 规则 v2、保护三层与危险语料、目录第 1 波、手选回收站、构建产物、安装包、空间洞察（只读） |
| G2（桌面首版） | 五工具信息架构、统一选择/结果组件、权限就绪检查、目录第 2 波 |
| G3（软件与系统） | 卸载两段计划与注册表备份、更新通道、启动项、维护目录与提权工作进程、状态采样与 CLI 状态流 |
| G4（常驻与特权） | 托盘/菜单栏组件、亮屏/清洁屏幕/隐私/电量提醒、特权 helper 与硬件矩阵、Touch ID |

每个门禁要求两平台真机证据；单项未通过则该能力关闭，不阻塞同门禁其他已通过能力的发布，但不得宣称对标完成。

“关闭”的实现：每个对标能力登记其门禁与是否已通过；未通过的能力默认关闭，CLI 命令组、`plan/v2` 对应种类的审阅与执行、桌面入口与宿主命令都拒绝并说明门禁。唯一的例外是用户在设置中显式打开的“实验功能”开关（CLI 与桌面共用同一设置，默认关闭）：门禁验收本身需要在真机上运行这些能力，所以保留这一显式开关；打开后能力仍报告为 `unverified` 并带“实验 · 门禁未通过”标记，从不报告为 `available`。关闭开关时常驻工具随之停止并移除其登录项。没有环境变量或隐藏参数可以绕过。

### D16. 对标复核补记的取舍（parity-review N 行）

| 行 | Mole 功能 | 取舍 | 理由 |
| --- | --- | --- | --- |
| N2 | `--debug` 详细日志 | 采用（CLI-14） | 排查扫描/执行问题需要阶段耗时、所选后端与跳过原因；只写标准错误，不改变单条机器记录与任何选择/授权 |
| N4 | 一次卸载多个应用 | 采用（UNI-11） | 一份计划含多个应用实例，审阅按应用分组，结果逐应用报告；单个应用失败不阻止其余应用，受保护类不因批量放宽 |
| N9 | 磁盘视图“可手动清理”标记 | 采用（DIN-05） | 用可验证签名（CACHEDIR.TAG、构建产物签名、已知缓存规则）给出只读线索与入口，不预选、不改档位 |
| N10 | 指定版本安装、nightly、Homebrew/Nix、启动时检查新版本 | 部分采用 | 指定版本：每个签名版本的安装包可单独获取（随 MVP 14.1/14.2 发布）；包管理器渠道在签名发行后另行提交，不在本变更内。nightly 不采用：只发布签名的版本化通道（D8/D13、LIF-01）。启动时自动检查不采用：联网只由用户触发（UPD-04） |
| N11 | Raycast/Alfred 启动器脚本 | 不采用 | 启动器脚本是第三方扩展格式，维护成本由本产品承担但不增加能力；任何启动器都能调用 `cleanmanager`，随附 Skill（CLI-13）是本产品的集成面 |
| N13 | status：代理/隧道流量去重、僵尸进程计数、显示偏好持久化 | 采用（STS-07） | 网络合计排除回环与隧道重复计数并单独显示隧道；显示偏好跨会话保留；僵尸进程计数只在 macOS 有意义（Windows 无此概念），随 12.1 的 macOS 采样实现 |
| N15 | 未入首批的 macOS 维护任务 | 部分采用（14.5） | 采用：网络卷不写 `.DS_Store`（设置变更，先备份原值，可恢复）、指定卷 Spotlight 重建（结果为 attention 级，需用户选卷）。不采用：Mail/Safari/Messages 的 SQLite VACUUM（写入其他应用的私有数据库，应用运行时有损坏风险，收益只是体积）；清除旧调优覆盖（只针对 Mole 自己写过的调优，本产品不写调优，MNT-05）；网络栈/ARP 刷新（W14 已定不纳入）；用户目录权限修复（大范围改 ACL，不可逆）；手动运行 periodic 脚本（系统已按计划运行，没有可验证收益）；共享文件列表/最近项目重置（属于隐私痕迹重置，见 Non-Goals） |

## Risks / Trade-offs

- [目录规模巨大，实测成本高] → 分波启用 + 登记驱动档位；未实测版本自动降为 review_only，而不是阻止扫描。
- [手选回收站接受路径型 API 的名称竞态] → 仅限单对象人工操作且可恢复；前后身份核验，不一致记 unknown 并引导放回；不扩展到自动规则。
- [供应商卸载器行为不可控、可能弹窗/重启] → 保留其界面、记录退出码、只把重新扫描结果当事实；第二段残留计划单独审阅。
- [注册表与启动项修改影响系统稳定] → 仅身份关联键、先备份后删除、可逆停用优先、系统/安全代理只读。
- [特权 helper 扩大攻击面] → 固定命令集、调用者签名与版本校验、硬件白名单、异常恢复自动控制；未通过 G4 不随产品分发。
- [常驻进程资源占用与“后台服务”预期冲突] → 默认关闭、按需创建面板、关闭即停止采样；不承载清理任务，不改变 MVP“无常驻任务宿主”语义。
- [状态指标与健康分被误读为需要清理] → 健康分与清理候选解耦，文案不宣称加速。
- [对标 Mole 视觉导致品牌近似争议] → 原创资源审查列入发布检查（GUI-11）。
- [MVP 文档与本变更表述冲突] → 任务 0 先用 `/opsx:update` 修订 MVP，再进入实现。
- [Mole Windows GUI 行为只有官网描述，开源 Windows CLI 能力更窄] → Windows 目录、卸载与注册表备份以自有实测为准；CLI 中未接入的代码不视为已支持（evidence.md W7）。

## Migration Plan

1. 先修订 `define-cleanmanager-mvp` 的冲突表述，确认本变更依赖顺序；MVP 的 G0 只读工作不受影响。
2. 契约先行：规则 v2、计划动作类型、新命令组的 Schema 与正反例在草案版本内补齐后再写执行代码。
3. 按 G1→G4 推进；每个能力默认 feature 关闭，通过门禁后在能力矩阵中开启。
4. 回退：规则包与能力开关随版本回退；注册表/启动项/设置变更都有恢复记录，回退程序不重放旧计划；helper 与常驻组件可独立卸载并恢复系统默认控制。
5. 归档顺序：先归档 MVP，再归档本变更，使 desktop-management 与 agent-cli 的 ADDED 增量落在已存在的 main spec 上。

## Open Questions

以下不改变规格与任务结构，由对应任务取证后确定：

- 第 1/2 波每个类目的具体应用版本与路径（任务 3.5、3.8）。
- 各 macOS 版本上特权 helper 的注册方式细节与可验证硬件型号列表（任务 13.2）。
- Windows 托盘 GPU/温度在不同 OEM 上的实际可得性（任务 12.1）。
