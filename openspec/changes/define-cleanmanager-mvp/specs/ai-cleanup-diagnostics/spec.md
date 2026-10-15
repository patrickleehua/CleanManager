# Spec Delta

## Purpose

作为可选扩展帮助用户理解具体文件或目录的用途、空间来源、删除后果和更稳妥的处理方式，解决看不懂清理候选及不敢处理未知大文件的问题。诊断提供有依据的建议、必要追问和行动交接，但不替代核心规则、保护、授权或实时核验。

## ADDED Requirements

### Requirement: AID-01 Optional diagnosis without dependency on AI
AI 诊断 SHALL 默认未配置且独立于首版九项能力的验收。缺配置、断网或服务故障 SHALL 不阻止本地扫描、规则说明、计划、执行与恢复；启用 SHALL 由用户主动选择，不默认发送数据或自动选择备用提供方。

#### Scenario: A user has no AI configuration
- **WHEN** 用户打开大文件详情或正常清理流程
- **THEN** 本地功能仍可用，诊断入口说明可自配接入，不要求先购买或连接 AI

### Requirement: AID-02 Object-scoped entry points including unknown files
用户 SHALL 可从清理候选、磁盘大文件/目录与应用关联数据发起对象级诊断，包括无匹配规则的对象。诊断 SHALL 绑定对象 ID、扫描版本、时刻和观察条件；集合/子项采样须有明确有限范围，不能因选中父目录隐式采集所有内容。

#### Scenario: A large video has no cleanup rule
- **WHEN** 用户在磁盘列表选择该视频并发起诊断
- **THEN** 系统可解释用途/删除后果及需用户确认的信息，不以没有规则拒绝诊断，也不把体积认定为垃圾

#### Scenario: A large directory has unreadable descendants
- **WHEN** 用户诊断该目录
- **THEN** 展示覆盖缺口与有限摘要，不推断未知子项均可清理

### Requirement: AID-03 Evidence provenance and freshness
诊断 SHALL 区分扫描观察、规则/归属证据、用户陈述与模型推测，提供可查看的引用、时间及缺口。未知备份、使用状态、共享关系或独占回收量 SHALL 保持未知；模型文本不能冒充本机检测结果。

#### Scenario: The model claims a backup exists
- **WHEN** 本地证据包没有已验证备份而模型声称已备份
- **THEN** 系统标记该说法缺少依据，不把它当作可删除条件

### Requirement: AID-04 Structured explanation and practical alternatives
有效诊断 SHALL 给出用途解释、带条件的建议状态、删除后果、重建/离线成本、依据、假设/缺口与可执行的下一步。建议 SHALL 区分可审阅处理、需补充信息、建议保留与不支持，不以百分比“安全度”或逻辑大小替代许可/实际释放量。

#### Scenario: A cache matches a verified rule
- **WHEN** 诊断证据包含完整适用规则与当前对象资料
- **THEN** 说明可再生成的内容和首次启动/下载影响，并提供受核心核验的计划审阅入口

#### Scenario: A shared model occupies many GiB
- **WHEN** 模型依赖与共享块未知
- **THEN** 说明再下载、离线使用与共享限制，建议确认引用或通过适配应用管理，不承诺释放全部逻辑字节

### Requirement: AID-05 Focused clarification without fabricated verification
追问 SHALL 绑定当前对象及已披露上下文，以必要问题缩小不确定性；用户回答 SHALL 标为用户陈述并可更正。新对象、额外路径或内容采集 SHALL 回到范围预览；未回答关键问题时保留不确定结论。

#### Scenario: A person says the video is already backed up
- **WHEN** 用户补充“已备份”
- **THEN** 回答解释还需确认备份可读及项目引用，不宣称系统完成了备份检测或取消保护

### Requirement: AID-06 User-selected API and CLI connections
扩展 SHALL 支持用户配置经验证的 API 端点/模型或已有 CLI 诊断适配，展示实际提供方/接入模式。连接探针 SHALL 不包含文件资料；切换目的地须重新确认披露。不可用模式 SHALL 有明确错误，不静默使用其他模型/账户或宣称订阅一定可复用。

#### Scenario: A configured API returns an authentication error
- **WHEN** 连接测试失败
- **THEN** 显示认证失败及配置入口，本地说明保留，不调用其他提供方

### Requirement: AID-07 Preview and minimize disclosed data
诊断请求 SHALL 先呈现目的地、脱敏后的实际载荷及是否含用户补充。默认 SHALL 仅发送选中对象必要元数据与有界证据，不发送内容、凭据、会话、整盘树或隐式下载云占位。扩大资料范围须单独选择与确认；取消预览不发请求。

#### Scenario: A user cancels the metadata preview
- **WHEN** 尚未确认此次披露
- **THEN** 提供方未收到诊断资料，扫描对象与本地事实保持原状

#### Scenario: A filename contains sensitive account information
- **WHEN** 默认证据包准备发送
- **THEN** 系统提供脱敏或省略后的实际预览，完整路径/名称不因它是“元数据”而默认外发

### Requirement: AID-08 Constrained adapters and untrusted provider text
CLI SHALL 使用经验证的受限诊断模式、结构化参数/stdin、有界时间/输出及可取消生命周期；不能用 shell 拼接或授予目标目录/写工具访问。无法约束的模式 SHALL 返回 unsupported。文件名、内容与提供方回复中的命令 SHALL 作为数据，不改变采集、保护、计划或授权。

#### Scenario: A CLI cannot disable its filesystem tools
- **WHEN** 用户选择该模式进行诊断
- **THEN** 系统报告当前适配不支持并提供 API/其他已验证模式选择，不启动不受约束的工具

#### Scenario: A model returns a destructive shell command
- **WHEN** 回答包含命令或“自动批准”指令
- **THEN** 系统不执行、不复制到 helper，允许的行动仍来自核心

### Requirement: AID-09 Independent action handoff
AI 建议 SHALL 不新增候选、规则、保护例外或授权。有效候选可交接原有不可变计划/审阅流程；未知、受保护或过期对象 SHALL 提供核实、保留、定位或已适配应用管理等下一步。用户陈述不等同于技术核验，本扩展不开放任意路径删除。

#### Scenario: A model calls a protected database disposable
- **WHEN** 用户尝试由诊断进入处理
- **THEN** 核心仍阻止该动作，解释保护理由并保留核实/定位入口

#### Scenario: A cache diagnosis hands off one selected object
- **WHEN** 当前候选与能力满足前置条件
- **THEN** 创建仅包含已审阅对象的新计划，授权和现场核验独立执行，不自动选择相邻缓存

### Requirement: AID-10 Visible cancellation errors and stale results
诊断 SHALL 明确显示准备、运行、完成、取消、失败与过期状态，支持取消、有限超时及按新请求重试；未完整或结构无效的回复不作为行动结果。导航不隐式取消，旧请求不得覆盖当前对象。观察变化后 SHALL 标示旧建议并关闭处理交接直到重新核验。

#### Scenario: The API times out after a partial explanation
- **WHEN** 尚无通过校验的完整结果
- **THEN** 保留本地事实和明确失败状态，允许重试，不显示已确认可清理

#### Scenario: The object changes after a successful diagnosis
- **WHEN** 核心发现观察条件过期
- **THEN** 显示需重新核验，旧解释可读但不能直接创建可执行计划

### Requirement: AID-11 Read-only cross-entry diagnosis contract
启用扩展后 GUI 与可选诊断 CLI SHALL 可查询同一 diagnosis_id、对象/版本、证据与建议状态。机器输出 SHALL 版本化、有界且无提示等待；无配置时返回明确状态。对外 Agent 导出须说明其披露范围，诊断接口不提供自批或文件写入端点。

#### Scenario: An agent queries a desktop diagnosis
- **WHEN** 用户已允许该范围的输出且 diagnosis_id 有效
- **THEN** 返回同一诊断与依据，不重新扫描整盘，不以该查询授予清理权

### Requirement: AID-12 Credentials and local diagnosis retention
提供方凭据 SHALL 保存在系统凭据存储且不进入前端、索引、提示、历史、日志或导出。诊断历史 SHALL 默认仅本机会话保留，主动保存/移除及导出范围可控；移除解释不能删除文件操作审计。保留记录须有提供方、模型/模板版本与时刻，不含真实凭据。

#### Scenario: A user exports a saved diagnosis
- **WHEN** 生成导出预览
- **THEN** 显示包含的对象/解释信息且不含密钥；独立的操作审计仍保留

### Requirement: AID-13 Contextual consistent and accessible diagnosis UI
桌面 SHALL 在对象详情提供 AI 诊断入口与可选工作台/配置，沿用现有主题、组件和响应式布局；与 AI 工具空间管理明确区分。诊断、依据、追问、取消与下一步 SHALL 可用键盘/读屏完成，重要结论不能仅靠颜色表达。

#### Scenario: A person uses a narrow window and dark theme
- **WHEN** 查看大文件诊断与依据预览
- **THEN** 同一内容可按层展开，无文档横向溢出，建议状态和按钮含明确文字
