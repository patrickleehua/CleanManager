# REA 调查记录

日期：2026-10-14。调查问题是：怎样实现一个兼顾 Mole 管理体验、WizTree 扫盘能力和 Agent CLI 的 Windows/macOS 产品；并非复制某个二进制或承诺与它性能相同。采用 reverse-engineer-anything 流程，先明确问题和目标，再固定制品、记录证据、登记未知项，最后把观察映射到设计要求。

## 1. 证据等级与范围

| 标记 | 含义 | 使用限制 |
| --- | --- | --- |
| REA | REA 实际检查制品生成的 evidence ID | 只证明对应检查覆盖的属性 |
| OBS | 普通工具的源码观察、签名查询或合成输入实验 | 不冒充 REA 反编译或产品运行证据 |
| DOC | 官方文档或厂商陈述 | 与实机测量、逆向确认分开 |
| ADR | 本项目的设计选择 | 不是参考产品事实 |
| UNK | 尚无足够证据 | 保留下一步可验证问题，不用推测填补 |

本次未启动 WizTree/Mole 产品，未扫描或清理用户磁盘，未运行 disk-cleaner 的扫描、备份或清理脚本。源码仓库使用常规检索和读取；便携包及 PE 使用 REA。官方页面信息来自前序研究和本轮下载入口核验。

## 2. 目标身份

| 目标 | 固定版本/身份 | 本轮覆盖 |
| --- | --- | --- |
| WizTree portable Windows | 4.33，官方 ZIP SHA-256 `38b30aab08710613d8a06a7d3dfb777e43ed436cfd7d1ff8bd33d0b5b6bd2c6d` | ZIP inventory/extraction、WizTree64 PE 资源、本机签名 |
| WizTree64.exe | SHA-256 `117e8e3b212f25442e9be5261e96b8d10e8baa3ca6e87317e2e11f313f03d533` | PE32+ x86-64；未运行、未完成函数分析 |
| Mole CLI | commit `eae1043b57157f1c51ab5600013d341298097d9c` | 入口、模块依赖、JSON 与扫描统计的源码抽样 |
| disk-cleaner | commit `fe16728994e4599242b3fd74b2f1ebff702063a9` | Python/Skill 架构、分类与脚本生成静态检查、两例合成 CSV |
| Mole GUI / WizTreeMac | 官方公开描述，未取得本轮 macOS 制品 | 仅 DOC，不声称内部实现或运行观察 |

WizTree ZIP 来自 [官方 download 页面](https://diskanalyzer.com/download) 实际链接 `files/wiztree_4_33_portable.zip`，没有猜测 CDN 地址。ZIP 为 7,905,571 bytes；64 位 EXE 为 11,017,016 bytes。完整文件哈希与来源见 [制品摘要](../evidence/wiztree-artifact-summary.json) 和 [来源登记](../evidence/sources.json)。

## 3. REA 执行链与原始证据

按依赖顺序执行：`open_binary(zip) → inspect_artifact → extract_artifact → inspect_pe_resources(WizTree64.exe) → open_binary(exe) → binary_overview`。最后一步返回 provider_unavailable；随后登记 unknown、导出证据包并关闭本轮原生会话。

| 对象 | Evidence ID | 证明的范围 |
| --- | --- | --- |
| ZIP 检查 | `ev_4996e85a985656d790a72c14c7f6ac2c448d97de2c957438debdb8bc4b7a82c7` | 制品识别与检查结果 |
| ZIP 清单 | `ev_8ee19e31856ebf5270cbad35564f722991acd2af2486eda09307885bbc9c1bdf` | central directory 清单及图关系 |
| ZIP 提取 | `ev_1ac1d5698402d8ee9ad95edd8428564eca07ab50fa08dfa21ff77f64b496b764` | 提取清单与制品关联 |
| 64 位 PE 资源 | `ev_be7c36ce7aea2b7315085aba8a5474859f3bb10e0fe380ac733eb272fd976a8f` | 格式、架构、资源目录；不是函数调用图 |

manifest 为 `agm_34dc0bb3106c2586a10126a820fc4e2957b48789bdfa449c59171621c08e1753`，图有 126 nodes、126 occurrences、125 edges，包含虚拟目录节点，不能把它写成 126 个文件。

PE 资源检查覆盖 390 个目录项、189 个资源，出现 `UNICODEDATA`、`VCLSTYLE` 及数字资源类型。这不足以证明扫描算法、完整开发框架、MFT 读取方式或调用某个具体 API；本轮未据此生成伪调用链。原始 [REA bundle](../evidence/rea-wiztree-4.33.bundle.json) 包含 6 条 evidence records 和 2 个 unknowns，具体结构以 bundle 为准。

**OBS-WIZ-01：**普通 PowerShell 的版本与 Authenticode 查询显示 FileVersion 4.33.0.0，签名状态 Valid，签名主体 Antibody Software Limited。[查询结果](../evidence/wiztree-windows-metadata.json)。这是当时本机信任环境的验证结果，不是性能或业务安全证明，也不是 REA evidence record。

## 4. 有边界的源码发现

**OBS-MOLE-01：**所固定 commit 的入口是 shell 脚本，`go.mod` 包含 Bubble Tea/Lip Gloss 等依赖；`cmd/analyze/json.go` 带 darwin build tag，定义 scan_status、entries、large_files 与整数大小字段，并通过 JSON encoder 写 stdout。这证明 CLI 已有机器输出路径，不能据此推断独立 GUI 与 CLI 共用核心。[JSON 源码](https://github.com/tw93/Mole/blob/eae1043b57157f1c51ab5600013d341298097d9c/cmd/analyze/json.go)、[go.mod](https://github.com/tw93/Mole/blob/eae1043b57157f1c51ab5600013d341298097d9c/go.mod)。

**OBS-MOLE-02：**扫描源码可见 `os.ReadDir`、并发限制、部分失败传播及 `du` 路径；大小逻辑含硬链接去重和 clone 标识处理。`getActualFileSize` 的代码返回 `min(st_blocks × 512, logical_size)`，缺少 stat 信息则退回逻辑长度。这个单一显示数值并非本项目所需的三种独立空间口径，不能直接拿来当“删除后可释放量”。这是代码阅读，不是对所有文件系统语义的运行验证。[scanner.go](https://github.com/tw93/Mole/blob/eae1043b57157f1c51ab5600013d341298097d9c/cmd/analyze/scanner.go#L1237)。

**OBS-DISK-01：**`scan.py` 调用外部 WizTree，`analyze.py` 分类并可生成 PowerShell，`backup.py` 为 Windows 工作流。没有证据表明此仓库提供自己的跨平台扫描内核。[固定版本](https://github.com/iziqing/-disk-cleaner/tree/fe16728994e4599242b3fd74b2f1ebff702063a9)。

**OBS-DISK-02：**只调用 `analyze_csv`，输入虚构的 `D:\Projects\temp\invoice.db`（100 MiB），返回 high/safe=true。触发依据是路径子串包含 temp，无法证明文件可再生或不含用户数据。

**OBS-DISK-03：**输入父临时目录及其 `.ssh` 子文件，父目录仍被列入 high/safe=true，子项被排除。源码生成器针对目录使用递归清空，只另排除一级 `claude*`；该输出设计不能保证分析阶段排除的后代不被父操作包含。**只复现了分类结果并阅读了生成器，未执行删除，不声称已实际丢失文件。** [初次记录](../evidence/disk-cleaner-probe.json)、[可复现结果](../evidence/disk-cleaner-reproduced.json)、[探针](../../tools/probe_reference_analyzer.py)。

因此只借鉴 disk-cleaner 的“扫描→分析→确认→执行”交互，不复用其清理执行方式。新的规则要识别来源、保护后代、固定对象集合，并在执行前验证实时身份。

## 5. 厂商描述与本项目结论

| DOC/OBS | 可以支持的决策 | 不能得出的结论 |
| --- | --- | --- |
| WizTree 官方描述 Windows MFT 快速路径、Mac 原生批量枚举 | 两平台分别实现原生扫描后端 | 已逆向确认精确调用，或 Rust 必然同样快 |
| Mole CLI 与 GUI 的官方定位不同 | 借鉴交互；本项目主动设计共用 Rust 核心 | 可以直接把开源 CLI 当作 GUI 源码 |
| Mole 源码有 JSON 与部分扫描状态 | 机器输出、覆盖率是成熟需求 | 既有字段足够承担新产品的计划授权协议 |
| 合成 CSV 分类风险 | 规则与执行需要对象级证据 | 所有版本或所有输入都会造成误删 |
| PE 资源与有效签名 | 制品身份、格式可追溯 | 性能、内部算法和运行安全已验证 |

厂商描述出处：[WizTree 平台说明](https://diskanalyzer.com/wiztreemac-vs-wiztree)、[Mole CLI/GUI 对比](https://mole.fit/blog/mole-cli-vs-mac-app)。设计落实见 [ADR](../decisions.md)、[平台设计](../platforms/windows.md) 和 [规则设计](../safety-and-rules.md)。

## 6. unknown 登记与恢复路径

REA 返回的 provider 缺口：Ghidra 12.1.x 安装目录/工具未配置、Java 未发现；Hopper 不支持本机 win32；IDA 未配置 REA_IDA_MCP_CONFIG。已经按技能 recovery 文档阅读诊断，没有将“工具不可用”写成“目标不存在原生函数”。[原始错误](../evidence/rea-native-provider-unavailable.json)。

| ID | 未知问题 | 下一步探针 | 是否阻断 |
| --- | --- | --- | --- |
| `unk_27af2b70e46453db3daca4d997298c191878a96651b472b07838daf1cbea20e3` | WizTree 原生扫描调用路径、数据结构与精确系统接口 | 配置兼容 native provider，针对卷打开/元数据查询/大小汇总建立证据链；必要时在测试卷做动态跟踪 | 阻断“已证实其内部算法”的结论；不阻断自研原型 |
| `unk_5d7c98b2185702add31098322c338e1b4162574b9f3bd4836c07b4a54d7351db` | 自研引擎性能与 WizTree 对比 | Windows + 真 Mac、相同语料/字段/权限/缓存状态，记录完整时间与内存 | 阻断性能宣传；是 G0 技术验证任务 |
| UNK-MAC-RUNTIME（本项目追踪，非 REA ID） | macOS 权限、APFS、云盘防物化、签名责任链 | 按 macOS 文档与测试矩阵运行真实平台夹具 | 阻断对应平台能力发布 |
| UNK-WRITE（本项目追踪，非 REA ID） | 平台写操作可实现的身份绑定与恢复范围 | 对抗 rename/reparse、崩溃与部分失败用例 | 阻断对应自动清理动作 |

unknown 记录是追踪问题，不是失败测试报告。特别是性能 unknown 关联当前调查上下文，并不表示已存在一个 Rust 制品参加对比。

## 7. 停止条件与交付边界

本轮已完成可用 REA 层的制品身份、清单、提取和 PE 资源检查；完成源代码参考与可复现分类探针；将剩余问题落到设计、任务和验收用例。没有为了凑“逆向完成”而把市场说明当作反编译结论。

继续深入 WizTree 需要补齐原生分析 provider；进入 CleanManager 开发则可直接开始只读扫描验证。产品价值来自统一可信计划、平台适配与人/Agent 一致的行为，不依赖复制闭源实现。
