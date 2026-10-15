# 验证计划与平台验收矩阵

日期：2026-10-14。当前阶段：设计与文档契约准备。**CleanManager 尚未实现；所有 OS 产品、平台、性能和执行安全用例均未执行。当前 Windows 主机不是已通过测试的产品环境，macOS 尚无真机结果。**

规范入口：[核心架构](core-architecture.md)、[安全规则与授权](safety-and-rules.md)、[CLI 协议](cli-protocol.md)、[v1 JSON Schema](../schemas/v1/protocol.schema.json)、[Windows 设计](platforms/windows.md)、[macOS 设计](platforms/macos.md)、[路线图](roadmap.md)。本文定义将来的可观察验收条件，不通过写出预期结果来声称功能已经实现。

## 1. 证据与状态分层

| 类型 | 当前证据/用途 | 不能证明的内容 |
| --- | --- | --- |
| 官方行为 | 平台文档所链 Microsoft、Apple、Tauri、Homebrew 一手资料，检索核对日期 2026-10-14 | CleanManager 已实现、API 在所有版本行为一致、任何性能数字 |
| 已有静态研究 | WizTree 4.33 portable 包清单与 PE 资源，见下方 REA 记录 | native 扫描调用路径、MFT 大小算法、运行效果、速度或安全性 |
| 已有受控合成输入 | [disk-cleaner probe](evidence/disk-cleaner-probe.json)，固定 commit 的 CSV 分类结果 | OS 扫描、备份、删除或 CleanManager 产品测试通过 |
| 文档契约检查 | Schema、合成示例、链接、稳定测试 ID 与字段一致性 | OS API、ACL/TCC、云召回、竞态或真实磁盘占用通过 |
| 产品运行证据 | 当前无；未来每次独立 run 记录真实输入和实际输出 | 不得用其他类别证据代替 |
| 拟议设计/验收目标 | 本文“预期输出”、内部状态机、资源参数与阶段门槛 | 官方承诺或已经测得的结果 |

现有研究沿用 [架构分析](architecture-analysis.zh-CN.md)，不重做竞品研究。已读取 [REA bundle](evidence/rea-wiztree-4.33.bundle.json) 中的下列实际记录：

| 操作/证据 | 标识与边界 |
| --- | --- |
| `inventory_artifact` | `ev_8ee19e31856ebf5270cbad35564f722991acd2af2486eda09307885bbc9c1bdf`；ZIP 中央目录清单 |
| `inspect_artifact` | `ev_4996e85a985656d790a72c14c7f6ac2c448d97de2c957438debdb8bc4b7a82c7`；有界静态推论，未执行 |
| `inspect_pe_resources` | `ev_be7c36ce7aea2b7315085aba8a5474859f3bb10e0fe380ac733eb272fd976a8f`；PE type/name/language 与资源关系，非运行或函数语义 |
| 目标摘要 | ZIP SHA-256 `38b30aab08710613d8a06a7d3dfb777e43ed436cfd7d1ff8bd33d0b5b6bd2c6d`；WizTree64.exe SHA-256 `117e8e3b212f25442e9be5261e96b8d10e8baa3ca6e87317e2e11f313f03d533` |
| native 分析限制 | [provider_unavailable](evidence/rea-native-provider-unavailable.json) 已保存；不安装引擎、不重复失败调用来填补设计文档 |
| 未决 native 路径 | `unk_27af2b70e46453db3daca4d997298c191878a96651b472b07838daf1cbea20e3`，open |
| 未决 CleanManager 性能 | `unk_5d7c98b2185702add31098322c338e1b4162574b9f3bd4836c07b4a54d7351db`，open；必须依赖阶段 0 真实运行 |

`record_unknown` 的 Evidence 只记录分析意图，不是答案证据。静态 inventory 成功不能登记为 `TC-WIN-*` 通过。测试 ID 长期稳定；删除或合并用例保留 retired/替代关系，不把旧 ID 重新分配给另一个问题。

状态定义：`未执行`＝没有该用例实际 run；`通过`＝实际输出满足全部断言且证据齐全；`失败`＝已运行且有不符合项；`阻塞`＝曾尝试执行但缺设备/权限/实现；`不适用`＝经记录的平台能力理由排除。这里未实现的产品用例统一保持**未执行**，不以“阻塞”掩盖未开始。某一平台通过不自动使双平台用例通过，故障注入通过也不代替真实 OS 路径通过。

## 2. 执行边界、环境与结果记录

阶段 0 扫描原型只读访问测试目标，允许写入自身索引与测量日志。禁止以验证为由扫描未经批准的用户磁盘、运行清理器、卸载真实应用、修改生产启动项或调整生产 USN/FSEvents/快照。下文 setup 仅在对应测试环境和范围明确后执行。

所有带修改的测试只在明确准备的专用夹具根、一次性虚拟卷/测试卷、测试账号或可还原 VM 上进行。运行前验证解析后的绝对根在测试范围内，记录卷身份、manifest、OS build 和工具版本；外部指向用独立 sentinel 目录检测越界。夹具创建与销毁是单独步骤，不能计入扫描时间或混入“扫描只读”证据。生产云盘、生产安装器与系统修复目录不作实验对象。

| 环境 ID | 拟议配置与用途 | 当前状态 |
| --- | --- | --- |
| ENV-WIN-X64 | Windows 11 x64 物理机、NTFS、SSD，分别普通用户与按需提权；记录实际 build/CPU/RAM/驱动 | 当前只有 Windows 主机上下文；设备细节未采集，产品未运行 |
| ENV-WIN-ALT | 专用 exFAT、SMB 测试共享、可移动介质；HDD 为扩展性能项 | 未准备 |
| ENV-WIN-ARM64 | Windows 11 ARM64 原生硬件、各发布二进制及 WebView2 | 未准备，不能从 x64 仿真推导 |
| ENV-MAC-ARM64 | 真实 Apple Silicon Mac、APFS、非敏感用户根、可控 FDA、实际签名身份 | 未准备；Windows 或容器模拟不替代 |
| ENV-MAC-INTEL | 仍在拟议支持范围内的真实 Intel Mac 与目标系统 | 未准备；未通过则不发布该架构 |
| ENV-CLOUD | 独立测试账号与专用小文件同步目录；指定 OneDrive/iCloud/实际 File Provider 版本 | 未准备 |
| ENV-FAULT | API seam、时钟、SQLite 提交点和事件故障注入 harness | 未实现；结果单独标 injected |

每次 run 至少保存：`test_id`、`run_id`、UTC 时间、代码 commit/二进制 SHA-256、OS/build/架构、FS/卷身份、权限/签名主体、scope 与 fixture manifest 摘要、后端与配置、命令或 harness 参数、输入种子、expected、**actual**、终态/退出码、stdout/stderr、测量原值、trace、观察者及限制。采集敏感路径时使用测试名/脱敏副本；原始测试记录保持可验证关联。

这些是未来测试报告字段，**不是**向严格的 v1 产品 JSON 添加字段。OS trace、观测原值与 performance report 独立存放。没有实际运行时 `actual=null`、`measurements=null`、`evidence_refs=[]`；不能写预期值或 0 伪装测量结果。

运行记录格式见 [run-record Schema](../schemas/v1/run-record.schema.json) 与[空白模板](evidence/runs/run-record.template.json)，存放在 `docs/evidence/runs/`。设备矩阵 [device-matrix.json](evidence/device-matrix.json) 分开登记 expected 与 actual，缺设备时 actual 为 null。平台能力声明只有在引用同平台、已观测设备上 status=passed 且 actual 非空的运行记录时才能标为 verified；文档验证器拒绝用 Windows 运行支撑 macOS 声明（SCN-07）。

## 3. 核心与 v1 协议共同判据

平台内部属性通过核心 Observation/适配层映射到 v1，不增加 Schema 未定义键。特别约束如下：

- `schema_version="1.0"`；终态类型、状态和退出码按 CLI 协议。部分扫描为 `scan_result.status=partial`、`completeness=partial_scope`、退出 5；缺系统权限且无法开始返回 `permission_required`、退出 4；用户批准缺失是 `needs_approval`、退出 3。
- `verified_scope` 只表示获准范围与所声明口径已完成核验，不是全盘原子快照。未覆盖子树数量使用 `skipped_ranges`，不知道里面多少文件时不能编造 `unknown_size_count`。未知对象大小留在内部；scan `logical_bytes` 为已知小计，配合完整性展示。
- 字节数是 u64 十进制字符串，空闲差值是有符号十进制字符串；未知分配量/回收估计为 null。JSON number 不承载字节计数。Windows 原始路径为 UTF-16LE Base64，macOS 为原始字节 Base64；显示文本不进入执行解析。
- **计划默认 TTL 为 15 分钟（900 秒）**；`expires_at=created_at+900s`。边界 `now >= expires_at` 为 stale，退出 6。授权到期、撤销、计划过期和目标变化分别检查；短 TTL 不免除即时核验。旧计划不自动续期，重新计划产生新摘要。
- PlanBody 使用 `CM-C14N-v1`/SHA-256；批准回执独立存储。单独修改 `authorization_status` 不获得授权。未知 Schema/字段拒绝，不能在 `error.details` 内偷偷塞平台错误字段。
- `resync_required`、`current`、`cancel_pending` 等是内部状态。公开停止进度为 `stopping`；不能提前发终态。`race_safety_unavailable` 等细分原因映射顶层 `unsupported`（退出 7），并可在允许的 issue/action code 与日志中说明。
- 崩溃处于 OS 动作与结果日志之间时，operation 使用 `reconciliation_required`，相应 action 为 `unknown`；不得重试并声称未执行。每个跨进程写操作按范围锁和去重键协调。

## 4. 阶段 0 基准：先测语义与完整端到端成本

### 4.1 可复现数据集

以下规模为**预定输入，不是已创建的数据或性能结果**。生成器固定种子 `20261014`，manifest 记录实际文件/目录数量、硬链接拓扑、路径长度分布、内容逻辑长度及测试选项；原生分配量在准备阶段采集，不能从逻辑量猜测。

生成器为 `cm-fixtures`（workspace 中的测试工具，不随产品发布）。准备、核对与销毁是独立命令，生成耗时只写入 manifest 的 `generation` 块，不计入任何扫描测量：

```text
cargo run -p cm-fixtures -- sentinel --path <SENTINEL>
cargo run -p cm-fixtures -- create --profile p0-10k --seed 20261014 --root <ROOT> --sentinel <SENTINEL>
cargo run -p cm-fixtures -- verify --root <ROOT> --sentinel <SENTINEL>
cargo run -p cm-fixtures -- destroy --root <ROOT> --sentinel <SENTINEL>
```

配置档：`tiny`（中文/空格/长名、0 字节、硬链接、指向 sentinel 的文件与目录链接）、`wide`（单目录 5,000 文件）、`deep`（64 层）、`p0-10k`、`p0-100k`（混合中文/空格/长名字）、`p0-1m`。`plan` 子命令只计算条目序列摘要而不写盘，同一配置与种子得到相同 `entries_sha256`。`create` 只接受不存在或为空的根、要求根与 sentinel 互不包含，并先写入根标记；`destroy` 拒绝没有标记的目录，按链接本身删除，结束后复核 sentinel 哈希。链接无法创建时在 manifest 的 `sentinel_links` 记录原因，而不是静默省略。目前未生成 junction、稀疏、压缩与 ADS 夹具，它们属于平台专项（Windows 任务 4.2）。

| 数据集 ID | 预定结构 | 用途与资源边界 |
| --- | --- | --- |
| P0-TINY | 200 个普通文件、20 个目录、已知大小列表；另有明确数量的错误/链接对象 | 人工可复核，先验证对象数和字节语义；不跑性能排名 |
| P0-10K | 10,000 文件、100 目录，普通文件长度按固定分布落在 0–4 KiB | 快速迭代、取消和基线；创建时间不计入扫描 |
| P0-100K | 100,000 文件、1,000 目录，0–4 KiB；混合中文/空格/长名字 | 基础吞吐、批量数据库、首屏和稳定内存 |
| P0-1M | 1,000,000 文件、10,000 目录，0–4 KiB；目录分布固定 | 普通 16 GiB RAM 级开发机的压力候选；预检卷空间，记实际元数据成本，不因逻辑数据小而忽略磁盘开销 |
| P0-SHAPES | 独立宽目录 50,000 项；独立 128 层深目录，每层短名；超平台合法长度作为负例 | 不与文件数吞吐混为同一测试，记录路径拒绝/句柄耗尽 |
| P0-SEMANTICS | 普通/稀疏/压缩文件、硬链接、ADS 或 fork/xattr、链接/挂载、云占位；APFS clone/快照各自独立小样 | 通常不超过数 GiB；与百万文件基准分开，便于准确归因 |
| P0-CHURN | P0-100K 的测试副本，harness 施加 1,000 次有日志的创建/写入/移动/移除 | 在专用夹具中比较无变化、有限变化与持续变化；扫描器本身只读 |

P0-1M 若超设备预算可中止并记录已生成规模，不能把 100K 结果填到 1M 行；生成器、磁盘加密、杀毒软件/Spotlight、同步客户端、索引数据库位置均进入报告。扫描索引放在夹具范围外，防止扫描自身新增文件；不为提高分数关闭安全软件而不记录。

### 4.2 测量方法

1. 每个平台至少一台真实机器，记录 OS build、CPU 架构、内存、SSD/HDD/外接接口、文件系统、簇/块信息、加密、剩余容量、权限、供电/热状态和后台活动。VM 可补故障测试，不能替代真实 Mac 权限/签名和硬件性能门槛。
2. 原型 release 构建，固定编译器、依赖与配置。基线是同项目的目录后端：Windows `win.directory`；macOS `mac.posix`。优化后端必须输出相同范围、字段、硬链接/ADS/fork 口径和权限结果后才能比较。
3. 区分 **首个可见批次** 与 **可完整查询的索引提交完成**。Windows NTFS 路径从卷探测开始，包含 USN 枚举、大小补齐、名字修复、日志对账和数据库提交；macOS 包含批量枚举、缺字段补查、FSEvents 对账和数据库提交。只测读取 MFT/名字不能称扫描完成。
4. 每个 warm 条件先一次不计分预热，再至少 5 次有效测量，保留全部原值，报告 median、min/max；样本这么少不报告可靠 p95。为冷启动另做至少 3 次受控重启后的首扫，并标 `cold_attempt`：系统/控制器缓存是否真的冷未知时明确写 unknown，不使用内存清理器、生产磁盘卸载或 cache purge 强行制造“冷缓存”。
5. 后端顺序交替/随机化，使用同一 manifest。文件新生成后立即扫描可能缓存热，不能标 cold。把首次索引、已有索引重扫、增量 catch-up 分成不同实验，不能混合平均。
6. 记录 wall time、CPU time、条目/实体数、未知大小数、跳过范围、数据/元数据 I/O、属性补查次数、峰值内存、索引/WAL 大小、峰值队列、首次结果时间、完成时间、取消确认与完全停止的延迟。Windows 工作集/Private Bytes 与 macOS RSS/physical footprint 注明口径，不能直接当同一个内存指标比较。
7. 在 10%、50%、90% 工作点各请求取消，至少 5 次/点。测 GUI/CLI 接收取消到停止调度、再到 I/O 退场两个延迟；卡住的 I/O 记 `cancel_pending`/公开 `stopping`，超观察窗口记超时而非只统计成功样本。不在无数据时保证固定毫秒值。
8. 时间限额只用于保护实验，例如单次 P0-1M 首轮观察窗口 15 分钟、worker 队列预算见平台文档；超限记录 incomplete，不当作测得完成时间。调整窗口/预算须另记配置版本，不能悄悄删除慢样本。
9. 正确性以生成 manifest、独立原生元数据观察及静止后的完整枚举对账为主。`du`、Finder、资源管理器、WizTree 等只作口径已声明的辅助对照；有硬链接/clone/快照/权限差别时不要求显示数字机械相等。可选竞品运行需有明确运行范围，当前没有运行证据，也不是 G0 的必要依赖。

### 4.3 结果登记：全部空白

| 实验 | 平台/后端 | 规模 | 时间、内存、取消原值 | 当前结论 |
| --- | --- | --- | --- | --- |
| P0-W-BASE | Windows / directory | 10K、100K、1M | 未采集（null） | 未执行；无性能结论 |
| P0-W-NTFS | Windows / ntfs_enum + 完整补齐 | 相同 manifest | 未采集（null） | 未执行；是否更快未知 |
| P0-M-BASE | 真 Mac / posix | 10K、100K、1M | 未采集（null） | 未执行；缺真机与原型 |
| P0-M-BULK | 真 Mac / bulk + 完整补查 | 相同 manifest | 未采集（null） | 未执行；收益未知 |
| P0-INCREMENTAL | Windows USN / Mac FSEvents | 100K + churn | 未采集（null） | 未执行；收敛与重建成本未知 |

G0 首先要求语义正确、部分结果可解释、扫描无目标内容写入/非预期召回以及有界资源。绝对完成时间、查询响应、内存和取消预算在取得基线后冻结；现阶段不编造“1M 文件 N 秒”。优化路线若与基线相比没有超过测量波动的可复现收益，保留简单后端；准确性下降时不允许用速度弥补。G0 未通过不能宣称双平台能力已完成。

## 5. 待执行：核心与协议矩阵

本节全部为**未执行产品用例**；纯文档 Schema 检查结果见第 10 节。`W+M` 表示 Windows 与 macOS 分别运行并保存结果。下面的“预期”是本项目验收设计，平台原理依据见两端文档，不能改写成官方产品保证。

| ID | 平台/阶段 | Setup 与操作 | 可观察预期输出/断言 | 状态 |
| --- | --- | --- | --- | --- |
| TC-CORE-001 | W+M / G0 | P0-TINY 含一个拒绝访问子树和一项未知大小；只读扫描 | 返回 scan_id/revision、partial/partial_scope、退出 5、已知小计与 null 分配量；issues 标识边界；不猜拒绝子树文件数 | 未执行 |
| TC-CORE-002 | W+M / G1 | 对真实 CLI 传未知 schema_version/字段、重复 JSON 键、冲突 format 和越界 limit | `schema_mismatch` 或 `invalid_argument`、退出 2；目标无变化，不静默忽略字段；默认 limit 50、上限 500 | 未执行 |
| TC-CORE-003 | W+M / G1 | 传超过 JS 安全整数但合法 u64 字节字符串；u64 溢出、负逻辑量、NaN、NUL/错误 raw 编码负例 | 合法值往返精确；非法输入拒绝；显示路径改变不改变执行身份；不可用字段不转 0 | 未执行 |
| TC-CORE-004 | W+M / G1 | 非 TTY 执行 JSON/JSONL；名字含换行与控制字符；强制中断输出 | JSON 单终态，JSONL progress 后恰好一终态，stdout 无日志/ANSI/BOM；强杀缺终态识别为不完整，不推断成功 | 未执行 |
| TC-CORE-005 | W+M / G1 | 同 revision 分页，随后更新索引再用旧游标；另改变排序/过滤条件 | 单次分页不混 revision；旧/异条件游标返回 `cursor_stale`、退出 6；不重复/漏取静止快照条目 | 未执行 |
| TC-CORE-006 | W+M / G1 | 无授权、有效有限策略、越范围策略、撤销票据四组 plan apply；均 non-interactive | 分别 needs_approval/正常进入核验/policy_denied/拒绝授权；没有隐藏等待，Agent 不能自批；拒绝组目标不变 | 未执行 |
| TC-CORE-007 | W+M / G1 | 注入可控时钟，默认创建计划；在 899 秒、900 秒、901 秒及重启后请求执行；期限内替换目标 | expires-created=900 秒；到期 `plan_stale`/6，不续期；899 秒仍核验目标；时间回退不得延长授权，无法证明有效时拒绝 | 未执行 |
| TC-CORE-008 | W+M / G1 | 改 PlanBody 的 scope/mode/路径/规则摘要、动作顺序或重复 action_id；仅改 authorization_status | CM-C14N 摘要/形状校验拒绝篡改；状态字段不形成批准；计划更新生成新 ID/摘要；无目标写入 | 未执行 |
| TC-CORE-009 | W+M / G1 | 规则反例：temp/invoice.db、父 cache 下 `.ssh`、会话/模型、未提交 worktree、共享容器 | 保护规则优先；父项不得吞掉后代；experimental 示例不能作为 enabled 规则；收益不重复计数 | 未执行 |
| TC-CORE-010 | W+M / G1 | 无 GUI 安装的独立 CLI 与 GUI 适配器调用同 scope/revision/规则和候选集 | 计划内容与摘要一致；GUI 不重建另一份计划；CLI 无 WebView 依赖；原始路径不来自显示文本反解析 | 未执行 |

## 6. 待执行：执行安全、权限与恢复

本节动作仅在未来获准的专用夹具执行。安全通过必须包含 sentinel 不受影响、真实 OS 调用轨迹和逐项日志；“返回成功字符串”不够。

| ID | 平台/阶段 | Setup 与操作 | 可观察预期输出/断言 | 状态 |
| --- | --- | --- | --- | --- |
| TC-SAFE-001 | Windows / G1 | 批准后用并发 harness 交换目标、祖先 junction/挂载入口和同名不同 ID；持有不同共享模式句柄 | 原 ID/范围不符为 plan_stale 或 skipped；无法保持祖先约束为 unsupported；所有外部 sentinel 未变；无路径删除回退 | 未执行 |
| TC-SAFE-002 | macOS / G1 | 在 fstatat 与 unlinkat/renameat 间由测试钩子替换末级名字；另移动祖先、跨卷挂载 | 后端不声称两次调用原子；未具备安全能力的计划在动作前 unsupported；不能仅因未复现错误而通过严格竞态保证 | 未执行 |
| TC-SAFE-003 | W+M / G1 | 父目录候选含保护叶子，批准后新增文件；另做父子跨规则重复选择 | 只处理冻结叶子或跳过父目录；新增文件和保护项保留，收益只算一次；不调用无界递归清空 | 未执行 |
| TC-SAFE-004 | W+M / G1 | 伪造 IPC PID/UID/path、过期 nonce、重放票据、过长消息、低权限可换名根；拒绝提权/服务批准 | helper 拒绝无效调用且无副作用；UAC/root 不当批准；低信任命名空间不由 root 清理；记录最小化错误 | 未执行 |
| TC-SAFE-005 | W+M / G1 | 一实体两个硬链接，一个在 scope 外；扫描后再添加链接/复用文件 ID；weak identity 夹具 | 默认跳过共享/弱身份写动作，或仅经适配处理精确名字；不声称删一名字释放全体字节；身份复用使计划 stale | 未执行 |
| TC-SAFE-006 | W+M / G1 | Trash 不可用/权限拒绝/跨卷；请求严格身份模式但后端只能按路径操作 | 输出 unsupported 或明确失败；不得改为永久删除；恢复能力如实为 unavailable/unknown；失败组 sentinel 与目标内容不变 | 未执行 |
| TC-SAFE-007 | W+M / G1 | 在 durable intent 前后、OS 动作后但 result 前、事务提交后分别注入进程退出 | 重启按已持久 intent 与实际对象核对；不明 action=unknown、operation=reconciliation_required；不隐式重跑副作用 | 未执行 |
| TC-SAFE-008 | W+M / G1/后续 restore | 同卷隔离后创建同名新文件；测试恢复冲突；含 ACL、ADS/fork、xattr、稀疏属性样本 | 恢复不覆盖新对象；保留隔离源和冲突记录；各元数据项实测声明可恢复/不支持；无“一切回滚”承诺 | 未执行 |
| TC-SAFE-009 | W+M / G1 | 同卷隔离、硬链接与后台写入三组；采集 before/after free；保留延迟回收条件 | processed_logical_bytes 与 observed_free_delta_bytes 分离，差值可负；隔离不报告已释放同等数据；未知观测为 null | 未执行 |
| TC-SAFE-010 | W+M / G1 | 专用测试卷限制可用容量，模拟备份/索引/日志写失败；锁住部分目标并中途取消 | 不删除未验证备份源；逐项区分失败/跳过/not_started；取消不回滚既有成功；日志不可持久化时不继续新写动作 | 未执行 |
| TC-SAFE-011 | W+M / G1 | 两进程用重叠范围不同计划并发；同去重键重复请求；同 key 不同摘要 | 范围冲突被拒绝或串行；同 key 返回同一 Operation；异参同 key conflict；进程消失后先核对后恢复 | 未执行 |
| TC-SAFE-012 | W+M / G2 | 签名错误/过期协议更新包、规则增权、helper/客户端版本错配和 DB 迁移故障 | 更新被拒绝或回到可用版本；不扩大旧授权；未知 schema 不执行；未完成操作日志不随索引重建丢失 | 未执行 |

## 7. 待执行：Windows 专项

官方行为入口：[USN 记录](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2)、[Journal 状态](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_journal_data_v0)、[文件属性/recall](https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants)。测试预期是 [Windows 拟议设计](platforms/windows.md) 的要求，不是这些官方文档对 CleanManager 的认可。

| ID | 平台/阶段 | Setup 与操作 | 可观察预期输出/断言 | 状态 |
| --- | --- | --- | --- | --- |
| TC-WIN-001 | Win11 NTFS/exFAT/SMB / G0 | 同类夹具分别位于各 FS；普通用户/提权读取；只授权目录后请求自动后端 | backend 与权限理由可见；目录范围不偷偷整卷读取；NTFS 接口失败可回退，exFAT/SMB 不走 NTFS 枚举 | 未执行 |
| TC-WIN-002 | Win11 NTFS / G0 | 静止 P0-100K；directory 与 ntfs_enum 完整扫描，并采集枚举/补齐阶段 | 名称、身份、已知逻辑/分配量按同口径一致；USN 不凭空生成大小；结果就绪时间含属性补齐、硬链接修复、对账与提交 | 未执行 |
| TC-WIN-003 | Win11 NTFS / G0 | 稀疏、压缩、0 字节、小 resident 文件、ADS 与多硬链接夹具；原生 API 独立取值 | 标注默认流/ADS 覆盖、分配来源；共享实体不重复算；缺字段 null；不得用逻辑簇对齐替代压缩/稀疏实占 | 未执行 |
| TC-WIN-004 | Win11 NTFS / G0/G3 | 基线扫描期间执行有限 create/write/rename/delete，记录 harness 操作日志；停止变化后对账 | U0 起事件不漏；新对象/目录移动被补入；最终与静止完整扫描一致；无未处理 dirty 才提高 applied_cursor | 未执行 |
| TC-WIN-005 | Win11 NTFS / G3 | 注入 JournalID 更换、游标早于 FirstUsn/有效历史、ERROR_JOURNAL_ENTRY_DELETED；实卷复现只限一次性卷 | resync_required，获准范围重建；不能跳尾续传并报完整；默认不创建/删除/扩容系统日志 | 未执行 |
| TC-WIN-006 | Win11 NTFS / G3 | 在索引批次/游标提交点注入崩溃；另注入 malformed/未知 major USN 记录 | 重启幂等回放；条目与检查点不分离提交；错误记录导致失效/重建而非越界解析或静默跳过 | 未执行 |
| TC-WIN-007 | Win11 NTFS / G3 | 目录移动、hard-link change、拆散 rename 对、长时间不关闭文件并继续写 | 通过重查/父目录对账更新，不依赖配对事件或 CLOSE；繁忙对象保留 dirty，部分范围不报 verified_scope | 未执行 |
| TC-WIN-008 | Win11 可移动 NTFS / G3 | 保存检查点，卸载测试卷后将不同卷映射到同盘符；另模拟 schema/权限变化 | 卷身份不符时拒绝旧检查点，重新探测；旧计划 stale；不把盘符一致当同卷 | 未执行 |
| TC-WIN-009 | Win11 + 测试 cloud provider / G0 | 在线/部分驻留/离线占位与 recall-on-open 父目录；扫描前后 provider 日志、访问 trace、驻留量 | 默认不进入未验证/高风险目录；已适配路径无可归因非预期 recall；未知字段保留；仅无网流量不足以通过 | 未执行 |
| TC-WIN-010 | Win11 NTFS / G0 | 长路径、中文、空格、大小写敏感目录、合法特殊名字和原始 UTF-16 code units | 原始路径往返正确；显示转义不污染执行；不把不同大小写对象合并；非法输入单独拒绝 | 未执行 |
| TC-WIN-011 | Win11 SMB/本地 / G0 | 枚举中断网/拔掉测试介质、阻塞属性调用和用户取消 | 保留已知 scan/revision 与错误边界，部分/取消退出码正确；UI 停止进度可见；不承诺瞬时取消或自动越范围重连 | 未执行 |
| TC-WIN-012 | Win11 / G3 | VM 内测试 MSI/EXE/MSIX/WinGet：同显示名不同来源、退出成功但仍注册、需要重启 | 清单按稳定身份消歧；未知卸载器不执行字符串；来源重查决定结果；当前用户移除不影响其他用户/预配包 | 未执行 |
| TC-WIN-013 | Win11 / G3 | 夹具 Run/RunOnce、Startup 入口、测试计划任务、自身 StartupTask，模拟策略禁用 | 显示类别/作用域；RunOnce/系统项只读；禁用入口不删目标程序；不绕过 DisabledByUser/Policy；恢复冲突不覆盖 | 未执行 |
| TC-WIN-014 | Win11 x64 / G2 | 干净 VM/真机的签名 NSIS、独立 CLI、缺 WebView2、离线安装、PATH 冲突和卸载 | 签名可验证；GUI 默认普通权限；CLI 独立工作；runtime 不可用错误明确；PATH 只改自身项；用户审计/隔离数据保留 | 未执行 |
| TC-WIN-015 | Win11 ARM64 + 目标 build / G2 | 原生架构安装/启动/扫描/升级，WebView2 与 helper 同架构/协议验证 | 每一发布构件架构正确；错误架构/协议明确拒绝；不能以 x64 仿真代替原生结果；未通过则该能力 unverified | 未执行 |
| TC-WIN-016 | Windows/离线 parser / 原始 MFT 立项后 | 专用 NTFS 镜像，畸形 fixup/runlist/attribute list、扩展记录、ADS、多名字与在线差分；模糊测试 | 无越界/崩溃/失控分配；不明记录隔离，覆盖率如实；与原生查询差异解释；默认 raw backend 仍禁用直到门槛通过 | 未执行 |

## 8. 待执行：macOS 专项

官方行为入口：[Apple 批量属性接口源码](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/getattrlistbulk.2)、[APFS](https://developer.apple.com/documentation/foundation/about-apple-file-system)、[FSEvents](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html)、[TN3150 dataless](https://developer.apple.com/documentation/technotes/tn3150-getting-ready-for-data-less-files)。所有环境在真实 Mac 上独立登记，不能由 Windows 静态 PE 证据推断。

| ID | 平台/阶段 | Setup 与操作 | 可观察预期输出/断言 | 状态 |
| --- | --- | --- | --- | --- |
| TC-MAC-001 | 真 Mac/APFS / G0 | P0-100K 用 posix/bulk 同权限扫描；注入缺属性位、单条 ATTR_CMN_ERROR 和畸形缓冲 | 相同字段一致；按 returned attrs 解析；单条错误不变成 0；补查计入总成本；解析器不会越界 | 未执行 |
| TC-MAC-002 | 真 Mac/APFS case-sensitive、外接 FS / G0 | 不同大小写/Unicode 名、深目录、symlink、挂载点、FIFO、app package；bulk 不支持注入 | 原始字节不归一化；默认不跨卷/跟随链接；特殊文件不读内容；降级到 posix 有理由；视图折叠不改变口径 | 未执行 |
| TC-MAC-003 | 真 Mac/APFS / G0 | 普通/稀疏/压缩、resource fork/xattr 小样；采集 stat、bulk、必要 Foundation 原值 | `st_blocks*512` 与各 alloc 字段按来源保留；逻辑/分配/fork 不混用；差异可解释或标 unknown，不推出可释放量 | 未执行 |
| TC-MAC-004 | 真 Mac/APFS / G0 | 硬链接跨目录、扫描根别名、Data 卷重复入口；资源属性缺失 | 按实体去重并保留目录项；跨子树不重复汇总；无法覆盖的元数据明确说明；不把 clone 当硬链接 | 未执行 |
| TC-MAC-005 | 真 Mac/APFS 专用卷 / G0 | 预备文件与 clone，改变其中部分块；分别只读观察逻辑/分配/共享证据 | 两实体有独立 ID；共享块影响 estimate，reclaimable_estimate_bytes=null、shared_blocks_unknown；不声称独占空间已知 | 未执行 |
| TC-MAC-006 | 真 Mac/APFS 专用容器 / G0/G1 | 预备可回收测试快照及同容器双卷；由 harness 处理测试文件，记录 before/after | 空闲池不双加；快照可能保留块，实际释放不保证等于文件分配；扫描器不删除快照；额外系统回收单列 | 未执行 |
| TC-MAC-007 | 真 Mac/APFS / G0/G3 | 监听启动后建基线，扫描期间有限 churn，最终静止；检查 stream/扫描起点 | 不存在先扫后监听的盲区；扫描中 dirty 子树重扫；最终按 manifest/全量重扫对账；flush 不当静止证明 | 未执行 |
| TC-MAC-008 | 真 Mac + 注入 / G3 | 注入 MustScanSubDirs、UserDropped、KernelDropped 和队列溢出；实际高负载补测 | 合并事件重扫标记子树；丢事件重建 stream 全部获准 roots；无静默继续完整，保留 rescan reason | 未执行 |
| TC-MAC-009 | 真 Mac/APFS / G3 | 移动/删除监控根，旧路径创建新目录；卸载并接入同名不同卷 | root_changed/身份失效，不跟随同名替代；重新绑定 scope；设备号不能代替卷/日志 UUID | 未执行 |
| TC-MAC-010 | 真 Mac + 注入 / G3 | 游标持久化各点崩溃；日志 UUID 变更、ID 回退/回绕、恢复历史失败 | dirty 与 checkpoint 一起持久；重启不漏未处理范围；历史失效重新基线，不 SinceNow 冒充完整 | 未执行 |
| TC-MAC-011 | 真 Mac/iCloud+provider / G0 | 测试云目录有 dataless 父/子对象；用 trace、provider 日志和驻留状态观察默认扫描 | 默认跳过未验证根；不以 exists/stat 深层路径先探测；有证据说明未触发非预期物化；无法归因时不判通过 | 未执行 |
| TC-MAC-012 | 真 Mac + 注入 / G0 | worker 的 setiopolicy 设置失败、EDEADLK、线程迁移与异常退出；另网络 FS | 策略失败保守跳过；EDEADLK 不开启物化重试；线程退出恢复原策略；不把 no-materialize 宣传为所有网络 I/O 禁止 | 未执行 |
| TC-MAC-013 | 真 Mac/真实签名 / G0/G2 | GUI、终端 CLI、GUI 子进程与 helper 分别访问受控 TCC 目标；FDA 授予/撤销；POSIX ACL 拒绝 | 每种主体的实际权限独立记录；EPERM 原因不明时不臆断；root 不视为 FDA/SIP 豁免；结果为部分或 permission_required | 未执行 |
| TC-MAC-014 | 真 Mac/SMAppService/XPC / G1/G2 | helper 注册批准/拒绝、错误签名 peer、协议不匹配；主 app 移动/升级 | 同意注册不等于同意删除；peer 真实身份校验，拒绝伪造消息；版本/注册状态清晰；无通用 root shell | 未执行 |
| TC-MAC-015 | 真 Mac/测试应用 / G3 | 同 bundle ID 多副本、测试卸载器、brew formula/cask、pkg receipt、共享 App Group | 精确实例/来源识别；普通 Trash 仅移包；未知 receipt 不递归清文件；zap 默认不执行；不 sudo brew；卸载结果再查 | 未执行 |
| TC-MAC-016 | 真 Mac/登录与后台项 / G3 | 自身 SMAppService、测试 LaunchAgent plist、第三方后台项/MDM 限制 | 自身状态真实，文件存在不等于运行；第三方通过系统设置/供应商流程；不改私有数据库或绕过 MDM | 未执行 |
| TC-MAC-017 | 真 Mac/签名公证 / G2 | 包含 CLI/helper 的正式签名包，正常下载 quarantine；离线首次启动、坏签名/缺票据对照 | 所有构件签名/公证验证，Gatekeeper 表现留证；不清 quarantine 或关闭系统保护；独立 CLI 分发也有验证路线 | 未执行 |
| TC-MAC-018 | 真 Mac/架构与升级 / G2 | 各拟支持 OS 和 ARM64/Intel 包；CLI 路径冲突、身份变更、helper/DB 升级失败 | 每组合分别通过才支持；不擅写系统 PATH/覆盖命令；失败保留可用旧状态；TCC 与服务批准变化可解释 | 未执行 |

## 9. 待执行：基准与桌面查询

| ID | 平台/阶段 | Setup 与操作 | 可观察预期输出/断言 | 状态 |
| --- | --- | --- | --- | --- |
| TC-P0-001 | W+M / G0 | 准备 P0-TINY/10K/100K/1M manifest，记录生成资源并独立取原生样本 | 实际条目数/身份/逻辑量可复核；生成与扫描时间分离；超预算规模如实记录；不以逻辑大小推算全部磁盘开销 | 未执行 |
| TC-P0-002 | W+M / G0 | 按第 4 节执行 cold_attempt、warm 条件，交替后端顺序 | 每次原值与缓存限制齐全，warm 至少 5 次、cold_attempt 至少 3 次；报告 median/range，不挑最快或伪造 p95 | 未执行 |
| TC-P0-003 | Windows NTFS / G0 | 同 manifest/字段/权限比较 directory 与 ntfs_enum，完整时间分解 | 补齐大小/名字和对账提交全部计时；覆盖不足不可作为更快结果；结论限定实测硬件与数据 | 未执行 |
| TC-P0-004 | 真 Mac/APFS / G0 | 同 manifest 比 posix/bulk；计入补属性与 DB/FSEvents 成本 | 提供真实结果和误差范围；不引用 Windows 秒数或竞品宣称；无真机则保持未执行 | 未执行 |
| TC-P0-005 | W+M / G0 | 1/2/4/8 并发，P0-1M/SHAPES；记录工作集/RSS、队列、DB/WAL | 无无界每文件任务；触顶背压/部分结果可见；不同 OS 内存指标分开；不隐藏达到资源限额的 run | 未执行 |
| TC-P0-006 | W+M / G0 | 在 10%/50%/90% 处取消，分别普通本地 I/O、阻塞/离线 I/O | 记录确认与完全停止延迟；未完成 I/O 不发成功终态；超窗口样本保留；不存在未经实测的硬延迟承诺 | 未执行 |
| TC-P0-007 | W+M / G0/G3 | P0-CHURN 的无变化、有限与持续变化实验；日志缺口注入 | 对账最终可收敛或明确 partial；重建时间与增量时间分开；持续变化不无限阻塞整个 UI | 未执行 |
| TC-P0-008 | W+M / G0 | 对扫描进程采集访问轨迹并与 setup/change harness 分开；测试文件 sentinel/hash 在扫描前后由独立 observer 检查 | 扫描器不请求用户内容读取/写入和下载；自身索引写入在指定位置；内容校验 observer 的读取不混成扫描行为 | 未执行 |
| TC-UI-001 | W+M / G2 | P0-1M 索引树分页、排序、treemap 展开和扫描中取消；固定订阅窗口 | UI 不接收整盘路径数组；背压/分页 revision 可见；记录帧/查询/取消延迟后与 G0 冻结预算比较 | 未执行 |
| TC-UI-002 | W+M / G2 | 未知大小、云跳过、部分权限、无 helper、过期计划与部分执行数据 | 页面不显示假 0/假 100%；部分范围、权限和批准分开；计划默认 15 分钟且到期阻止执行；错误与 CLI 一致 | 未执行 |
| TC-UI-003 | W+M / G2 | 键盘和系统读屏器操作磁盘表、预览与执行结果；缩放/中英文长名字 | 焦点顺序/名称/状态可读；treemap 有可访问表替代；危险动作不能只靠颜色；进度不淹没读屏 | 未执行 |

## 10. 已执行文档检查与产品结果分开登记

本节登记平台设计、验证计划及其引用契约的静态检查。**文档检查通过不改变第 5–9 节任何 OS 产品用例的“未执行”。**

| ID | 平台/范围 | Setup 与检查 | 可观察预期输出 | 当前状态与证据 |
| --- | --- | --- | --- | --- |
| TC-DOC-001 | Windows 主机 / 文档 | 只读检查两平台文档及本计划的 UTF-8、围栏、表格列数与 Markdown 本地链接 | 文件可读、无替换字符、围栏闭合、所有本地目标存在 | 通过，2026-10-14；3 文件、32 个本地链接，无错误；仅文档检查 |
| TC-DOC-002 | Windows 主机 / 文档 | 提取本文件表格测试定义与两平台全部 TC 引用，检查唯一性与解析 | 测试 ID 唯一；引用均有定义；产品行全部未执行 | 通过，2026-10-14；70 个唯一 ID，引用无缺失，67 个产品用例全部未执行 |
| TC-DOC-003 | Windows 主机 / 契约引用 | 将平台映射与当前 v1 Schema、CLI/安全文档核对，检查默认计划示例时间差 | TTL=900 秒；byte/path/status/error 映射符合 v1；不把内部字段加进严格 JSON | 通过，2026-10-14；2 schemas、8 协议示例、1 规则示例；默认计划时间差 900 秒；仅契约检查 |

仓库提供 [文档验证器](../tools/validate_docs.py)，用于 Schema/合成示例与链接检查；该脚本的 `passed` 只具有文档契约含义，不能复制进产品测试状态。2026-10-14 执行 `python -B tools/validate_docs.py` 返回 passed：2 schemas、8 协议示例、1 规则示例、9 个负例拒绝、127 个本地链接、16 个 Markdown 文件、6 条 REA 记录、2 个 REA 未决项、2 个既有探针匹配。另用只读检查确认本计划 ID 唯一、平台引用完整。以上计数是该日期的检查快照，后续修改应重新运行并记录新结果。

### 10.1 持续集成（CI）映射与状态

[CI 工作流](../.github/workflows/ci.yml)定义两个 job：`windows-x64`（`windows-latest`）与 `macos-arm64`（`macos-14`），Rust 工具链取自 [rust-toolchain.toml](../rust-toolchain.toml)。每个 job 先记录实际 `runner.os`、`runner.arch`、`uname -m` 与镜像版本（同时写入 job summary），与预期架构不符时告警。

| CI 步骤 | 对应任务/需求 | 通过只说明 |
| --- | --- | --- |
| 危险路径语料与保护模糊测试 | add-mole-parity-suite 2.2（PRT-03/04/05） | [危险路径语料](../tests/corpus/dangerous-paths.json)全部条目在 cm-core 模型机上被拒绝（含确定性变异，以及“解析到硬拒绝根内的路径绝不 Allowed”的性质测试）；runner 本机的硬拒绝身份链在真实 junction/符号链接/挂载点夹具上全部拒绝 |
| `cargo test --workspace --locked`、`cargo clippy --workspace --all-targets -- -D warnings` | define-cleanmanager-mvp 1.5 | 受控单元、契约与合成夹具测试和 lint 通过 |
| 契约生成幂等、依赖清单、文档验证器 | define-cleanmanager-mvp 1.5 | `schemas/`、`examples/`、`rules/` 与依赖清单和生成器一致；文档契约检查通过 |
| 桌面 `npm ci`、`npm run build`、`npm test` | define-cleanmanager-mvp 1.5 | webview 类型检查、构建与 vitest 通过 |
| 最后一步：CI 未改动任何受跟踪文件 | 证据边界 | CI 没有写入 `docs/evidence/**`、设备矩阵或运行记录 |

CI 通过**不**改变第 5–9 节任何用例状态，也不把真机、签名/公证或系统权限（UAC、FDA/TCC、SIP）验收标为完成；托管 runner 不是 ENV-WIN-X64 / ENV-MAC-ARM64 参考设备。**截至 2026-10-15 该工作流从未运行**：仓库没有 Git 远端，只做过 YAML 解析检查。等价命令已在本地 Windows x64 主机执行；macOS 一侧只有 `cargo check --target aarch64-apple-darwin` 编译检查，本机硬拒绝语料测试将在首次 macOS 运行时第一次执行。首次远端运行结果应单独登记。

## 11. 阶段门槛、偏差处理与未决点

| 门槛 | 必须取得的证据 | 当前结论 |
| --- | --- | --- |
| G0 双平台只读可行性 | TC-P0 全部；CORE-001；Windows 基础语义/云/路径/取消；macOS 批量/APFS/云/权限；两端真实运行及误差说明 | 未达成；无原型和实测 |
| G1 有限写闭环 | CORE 全部、SAFE-001 至 SAFE-011 的适用组合；默认 TTL/授权；helper 身份与限定范围 | 未达成；不能发布任意目录清理 |
| G2 桌面与分发 | UI 全部；平台签名/安装/架构/升级及 SAFE-012；只为实际测试组合背书 | 未达成 |
| G3 增量与应用管理 | USN/FSEvents 连续性、重建、崩溃用例；每类应用/启动项适配单独验收 | 未达成；可按能力保持 unsupported/unverified |
| 原始 MFT 专项 | 先有端到端性能不足证据，再完成 TC-WIN-016 与语义/安全回归 | 未立项；默认禁用 |

发现失败时保留 run 原值、缩小能力声明并登记问题；不能修改 expected 去迎合现有 bug。修复后关联新 run，旧失败不覆盖。云端下载、越界操作、身份替换、安全日志丢失是对应能力的发布阻断项；对不支持场景正确拒绝可通过拒绝用例，但不能据此把实际清理能力标 available。

仍未解决的具体问题：

1. Windows USN 枚举加大小/硬链接补齐是否比批量目录遍历更快；原始 MFT 是否值得增加复杂度。
2. APFS 压缩、fork/xattr、clone 和快照的占用可解释性；不能获得独占块信息时如何呈现未知估算。
3. 实际 OneDrive/iCloud/File Provider 版本的默认不召回行为与可归因观测；没有证据就保留跳过策略。
4. Windows 祖先句柄与共享模式、macOS 最后一级名称竞态能支持哪些严格写操作；特别是用户可写目录的特权 helper 边界。
5. CLI/GUI/helper 的 TCC/FDA 与签名责任链、最低系统/架构矩阵，以及正式签名、公证与更新配置。
6. 核心 Observation 指纹的规范化细节与内部诊断字段，当前 v1 尚未暴露的异步查询/应用管理协议；必须先明确契约再实现，不能在严格 Schema 中暗加字段。

现有 REA native provider 不可用不阻塞官方 API 设计或自有原型开发，但必须继续保留“竞品 native 实现未知”。产品能力必须由对应实际运行证据验收，不能由验证规范或文档检查结果代替。
