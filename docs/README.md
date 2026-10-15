# 文档索引与交付状态

最近整理：2026-10-15；研究证据保留各自的采集日期。产品暂名 CleanManager；所有架构与默认参数均是实现前设计基线，不表示已有发布版本。用户明确的方向是 Windows/macOS、Rust 优先、Mole 式管理体验、WizTree 式磁盘分析，以及人和 Agent 都能使用。

## 阅读路径

| 文档 | 回答的问题 |
| --- | --- |
| [总体分析](architecture-analysis.zh-CN.md) | Rust 是否合适，三个参考项目分别借鉴什么 |
| [REA 调查与证据](research/rea-investigation.md) | 实际检查了什么，观察值与推断如何区分 |
| [产品与交互](product-and-ux.md) | 页面、人工/Agent 旅程、部分成功与恢复如何展示 |
| [视觉标识与信息架构](visual-identity.md) | 原创标志/图标与资源清单、设计令牌与对比度检查、五工具导航与快捷键、平台惯用语、统一选择组件 |
| [AI 清理诊断扩展](ai-cleanup-diagnostics.md) | 自配 API/CLI、用途与删除影响、依据/追问及原型交互 |
| [v6 可点击原型](../output/ui-preview-2026-10-14/suite-v3/prototype-v6.md) | 五工具信息架构、规格/门禁状态、体验路径与浏览器验证（[v5](../output/ui-preview-2026-10-14/suite-v3/prototype-v5.md) 保留作 AI 诊断交互参考） |
| [OpenSpec 提案](../openspec/changes/define-cleanmanager-mvp/proposal.md) | 首版九项能力、可选 AI 扩展及设计/任务验收规划 |
| [Mole 对标完成度复核](../openspec/changes/add-mole-parity-suite/parity-review.md) | 每项 Mole 功能的归属、实现状态、阻塞门槛，以及对外文案审计（2026-10-15） |
| [核心架构](core-architecture.md) | 模块、数据对象、索引、并发和状态机 |
| [Windows 后端](platforms/windows.md) | NTFS、USN、权限、云占位、应用管理与发布 |
| [macOS 后端](platforms/macos.md) | 批量枚举、APFS、FSEvents、TCC 与平台限制 |
| [规则与执行](safety-and-rules.md) | 候选如何成为固定计划，如何授权和核验 |
| [清理目录](catalog.md) | 第 1 波 Windows 类目、档位计算、占用探测、所属工具委托与实测证据 |
| [手选移入回收站](manual-trash.md) | manual_selection 计划、系统目录只读、占用进程、回收站内永久删除与放回方式 |
| [启动项](startup-items.md) | 可逆启停、失效项备份后删除、管理员要求与恢复计划 |
| [应用卸载](app-uninstall.md) | 两段卸载计划、各平台支持来源、签名核验、运行中处理、残留分组与注册表备份恢复 |
| [系统状态与洞察卡片](system-status.md) | 进程解释与结束进程、关键进程保护、可选高 CPU 提醒、状态流过期标记、概览卡片与旧下载审阅 |
| [常驻工具](resident-utilities.md) | 可选托盘（默认关闭、关闭即停止采样并删除登录启动项）、保持亮屏、清洁屏幕、摄像头/麦克风提示与设备电量 |
| [应用更新](app-updates.md) | 检测通道与“无法检查”、检查开关、签名一致才执行的更新与逐项结果 |
| [系统维护](system-maintenance.md) | 维护任务目录、六种结果、集中提权、禁止项、只读诊断与运行记录 |
| [提权边界](privilege-boundary.md) | Windows 短生命周期提权工作进程的命令集、校验、生命周期与移除；macOS helper（规划） |
| [CLI 与 Agent 协议](cli-protocol.md) | 命令、JSON、退出码、分页和 Skill 边界 |
| [设计决策](decisions.md) | 关键选择、依据与重新评估条件 |
| [分发与许可](distribution-and-licensing.md) | 参考代码、第三方包、签名、更新和隐私 |
| [依赖清单](dependencies.md) · [桌面端依赖清单](dependencies-desktop.md) | 每个锁定依赖的版本、许可、来源与用途（由工具生成并由验证器比对） |
| [安装、更新与卸载](install.md) | 发行布局、自选安装目录与 PATH 冲突、签名自更新与回滚、存储迁移、保留数据的自卸载 |
| [隐私说明](privacy.md) · [第三方声明](third-party-notices.md) | 本机保存什么、什么只在你操作时离开本机；随发行包分发的第三方组件与许可（由锁文件生成） |
| [验证计划](verification-plan.md) | 平台、正确性、竞态、性能、恢复如何验收 |
| [实施路线](roadmap.md) | 任务、依赖、阶段门禁和可领取的第一批工作 |
| [复现说明](research/reproduction.md) | 如何验证契约、复现分析器探针与 REA 检查 |

## 可以机器校验的材料

- [协议 Schema](../schemas/v1/protocol.schema.json) 与 [规则 Schema](../schemas/v1/rule.schema.json)。当前覆盖核心记录形状，不代表完整 CLI 已冻结。
- [合成协议样例](../examples/contracts/README.md) 与 [示例规则](../rules/examples/example-cache.json)。示例不会启用真实清理规则。
- [契约生成器](../tools/generate_contracts.py)、[文档验证器](../tools/validate_docs.py)、[参考分析器探针](../tools/probe_reference_analyzer.py)。
- [证据来源登记](evidence/sources.json)、[REA 原始证据包](evidence/rea-wiztree-4.33.bundle.json)、[制品摘要](evidence/wiztree-artifact-summary.json)。
- [文档校验结果](evidence/document-validation.json)、[分析器复现结果](evidence/disk-cleaner-reproduced.json)。二者都不是 CleanManager 的产品运行结果。

## 文档之间的约束

JSON Schema 负责已定义字段的机器形状；CLI 文档负责参数和输出行为；核心与平台文档负责执行语义。Schema 通过不等于系统操作安全。若三者冲突，应记录并修订，不能挑选最宽松解释。

总体分析保留研究脉络，具体实现以专题文档为准。计划默认有效期统一为 **15 分钟**，到期重建，未到期仍重新核验；页面中的其他时间/规模模板是可调整设计值。产品/路线文档的 DEC 编号是交互默认项，架构 ADR 编号另列于设计决策。

## 当前已经完成与仍待验证

| 类别 | 本轮状态 |
| --- | --- |
| WizTree 4.33 官方 portable ZIP、文件哈希、PE 资源与本机签名查询 | 已检查；未运行程序 |
| WizTree 原生函数与扫描调用链 | 工具后端未配置，已登记 unknown |
| Mole CLI 与 disk-cleaner 源码抽样 | 已固定 commit 并保存位置/哈希；不是全面源码审计 |
| disk-cleaner 两个分类风险案例 | 已用合成 CSV 复现；未执行生成的清理脚本 |
| 设计、任务、协议样例与文档一致性 | 本轮交付，具体校验数以结果文件为准 |
| Mole 风格 HTML 原型与 AI 诊断交互 | 已设计并检查浏览器演示；未连接真实模型/CLI/本机操作 |
| Rust 扫描器、桌面、CLI、授权与清理执行器 | Windows 11 x64 已实现，经单机自动化测试与本机运行记录检验；macOS 后端未实现 |
| Mole 对标能力（`add-mole-parity-suite`） | 多数 Windows 部分已实现，没有一项通过门禁；逐项状态见 [对标完成度复核](../openspec/changes/add-mole-parity-suite/parity-review.md) |
| Windows/macOS 真机产品验收（G0–G4）、CI 实际运行、性能比较、签名发布 | 尚未通过；见两个 OpenSpec 变更的 tasks 与验证计划 |

下一步以两个 OpenSpec 变更 tasks 中未勾选的任务为准：macOS 后端、CI 实际运行、真机门禁记录与签名发布。能否达到 WizTree 的速度，由同口径实测决定。
