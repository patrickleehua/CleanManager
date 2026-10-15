# 文档索引与交付状态

最近整理：2026-10-15；研究证据保留各自的采集日期。产品暂名 CleanManager；所有架构与默认参数均是实现前设计基线，不表示已有发布版本。用户明确的方向是 Windows/macOS、Rust 优先、Mole 式管理体验、WizTree 式磁盘分析，以及人和 Agent 都能使用。

## 阅读路径

| 文档 | 回答的问题 |
| --- | --- |
| [总体分析](architecture-analysis.zh-CN.md) | Rust 是否合适，三个参考项目分别借鉴什么 |
| [REA 调查与证据](research/rea-investigation.md) | 实际检查了什么，观察值与推断如何区分 |
| [产品与交互](product-and-ux.md) | 页面、人工/Agent 旅程、部分成功与恢复如何展示 |
| [AI 清理诊断扩展](ai-cleanup-diagnostics.md) | 自配 API/CLI、用途与删除影响、依据/追问及原型交互 |
| [v5 可点击原型](../output/ui-preview-2026-10-14/suite-v3/prototype-v5.md) | 体验路径、视觉接入和浏览器验证范围 |
| [OpenSpec 提案](../openspec/changes/define-cleanmanager-mvp/proposal.md) | 首版九项能力、可选 AI 扩展及设计/任务验收规划 |
| [核心架构](core-architecture.md) | 模块、数据对象、索引、并发和状态机 |
| [Windows 后端](platforms/windows.md) | NTFS、USN、权限、云占位、应用管理与发布 |
| [macOS 后端](platforms/macos.md) | 批量枚举、APFS、FSEvents、TCC 与平台限制 |
| [规则与执行](safety-and-rules.md) | 候选如何成为固定计划，如何授权和核验 |
| [清理目录](catalog.md) | 第 1 波 Windows 类目、档位计算、占用探测、所属工具委托与实测证据 |
| [CLI 与 Agent 协议](cli-protocol.md) | 命令、JSON、退出码、分页和 Skill 边界 |
| [设计决策](decisions.md) | 关键选择、依据与重新评估条件 |
| [分发与许可](distribution-and-licensing.md) | 参考代码、第三方包、签名、更新和隐私 |
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
| Rust 扫描器、桌面、CLI、授权与清理执行器 | 尚未实现 |
| Windows/macOS 真机产品验收、性能比较、签名发布 | 尚未执行；见阶段 0 和验证计划 |

下一步可直接领取 TASK-001 与 TASK-002，建立环境矩阵、只读语料和核心契约。能否达到 WizTree 的速度，由同口径实测决定；无需等反编译完成才开始自研只读原型。
