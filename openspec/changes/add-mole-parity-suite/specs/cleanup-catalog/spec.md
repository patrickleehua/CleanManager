# Spec Delta

## Purpose

把 Mole 式“能清的都列出来”落成跨 Windows/macOS 的版本化清理目录：每个类目说明来源、再生成代价、默认选择档位和实测依据，让人工与 Agent 获得同一份候选而不是各自拼路径。

## ADDED Requirements

### Requirement: CAT-01 Platform-grouped cleanup catalog
系统 SHALL 提供按平台分组的清理目录，至少覆盖系统数据、用户缓存与日志、回收站/废纸篓、应用缓存、浏览器、云与办公、开发工具、AI 工具数据、虚拟化、应用残留、设备固件与备份、未完成下载及大文件提示。每个类目 SHALL 绑定规则 ID/版本与平台，未覆盖的平台显示 unsupported 而非空结果。

#### Scenario: A Windows user opens the cleanup page
- **WHEN** 扫描完成且 Windows 类目有已启用规则
- **THEN** 列出系统数据、用户缓存、浏览器、开发工具等分组及各自已知大小，未适配的 macOS 专有类目不出现为 0 字节

#### Scenario: A category has no enabled rule on this platform
- **WHEN** 某分组在当前平台只有 experimental 规则
- **THEN** 分组显示“尚未验证”并不可选择，不把候选混入其他分组

### Requirement: CAT-02 Three selection tiers shared by every entry
每个候选 SHALL 属于 default_selected、review_only 或 blocked 之一，并带分类理由。default_selected 仅用于有实测证据、可本地自动重建且不在使用中的对象；需联网恢复、含离线/用户数据或归属不完整的对象 SHALL 为 review_only；blocked 对象 SHALL 不能被任何入口选入计划。GUI、CLI 与 Agent SHALL 使用同一分类。

#### Scenario: A dependency cache needs the network to rebuild
- **WHEN** 规则命中的目录删除后只能重新下载恢复
- **THEN** 候选为 review_only 且默认不勾选，影响说明写明离线与重新下载代价

#### Scenario: An agent requests a blocked candidate
- **WHEN** CLI 计划请求包含 blocked 对象 ID
- **THEN** 返回 policy_denied 及该对象的阻止理由，计划不被创建

#### Scenario: CLI and GUI list the same scan
- **WHEN** 两个入口查询同一 scan revision 的候选
- **THEN** 候选集合、档位与理由一致，不存在只在 CLI 可删的更宽集合

### Requirement: CAT-03 Tested-application registry
每条 enabled 规则 SHALL 引用实测登记：应用/工具名称、版本范围、平台/系统版本、测试日期、正反例与已知影响。登记 SHALL 可在规则详情与 CLI 中查询；超出登记版本范围的安装 SHALL 将候选降为 review_only 并说明版本未验证。

#### Scenario: An installed browser is newer than the tested range
- **WHEN** 规则登记覆盖到旧主版本，本机为更新主版本
- **THEN** 该浏览器缓存候选为 review_only，并显示“此版本未实测”

### Requirement: CAT-04 AI tool data separation
AI 工具类目 SHALL 将可重建缓存与旧版本运行时、会话/历史、worktree 和本地模型分开。缓存与已被替换的旧版本可按 CAT-02 分级；会话与 worktree SHALL 保持默认保护，只允许逐对象人工确认，且不得被预授权策略匹配；有未提交或未跟踪修改的 worktree SHALL 为 blocked。

#### Scenario: Old Codex sessions are listed
- **WHEN** 用户查看 AI 工具类目
- **THEN** 旧会话逐条显示最后活动时间与大小，默认不勾选，批量全选不包含会话

#### Scenario: A worktree has uncommitted changes
- **WHEN** worktree 的 Git 状态显示未提交修改或无法判定
- **THEN** 该 worktree 为 blocked 并说明原因，不进入任何计划

### Requirement: CAT-05 Owner-tool delegated cleanup
当某类缓存只能由所属工具安全清理时，系统 SHALL 使用预定义的固定命令适配（如包管理器的 cache clean、模拟器的不可用运行时删除），先以只读方式取得预览，再作为独立动作进入计划。适配 SHALL 使用固定可执行文件与参数、有时限、记录退出码与输出摘要，不拼接 shell 或接受用户/模型提供的参数。

#### Scenario: A package manager cleanup is planned
- **WHEN** 用户选择某包管理器的“清理未引用缓存”项
- **THEN** 计划显示适配名称、工具版本、预览得到的对象/大小与“由该工具执行”的说明，执行结果记录工具退出状态

#### Scenario: The owner tool is missing or times out
- **WHEN** 所属工具不存在或预览超时
- **THEN** 该项为 unavailable 或部分结果，不改用直接删除目录

### Requirement: CAT-06 Tri-state in-use probes
规则 SHALL 声明所需的占用探测（所属进程运行、打开的数据库/日志句柄、未完成下载句柄、终端安全软件目录）。每个探测结果 SHALL 为 in_use、not_in_use 或 unknown；in_use 与 unknown 均 SHALL 使对象跳过，并在扫描预览与执行前各探测一次。

#### Scenario: A browser is running
- **WHEN** 浏览器进程运行中且其缓存被选入计划
- **THEN** 执行时该项标为 skipped（所属应用运行中），报告提示关闭后重试，不强制结束进程

#### Scenario: Process enumeration fails
- **WHEN** 无法判断所属应用是否运行
- **THEN** 探测结果为 unknown，对象跳过而不是按未运行处理

### Requirement: CAT-07 Versioned age and size thresholds
年龄、大小与保留期阈值 SHALL 随规则版本声明并在候选理由中显示，修改阈值 SHALL 产生新规则版本。修改时间 SHALL 不被描述为最后使用时间；无法取得时间戳时 SHALL 不满足年龄条件。

#### Scenario: A log file has no readable timestamp
- **WHEN** 规则要求“7 天前”但对象时间不可得
- **THEN** 该对象不成为候选，并在覆盖说明中计入未判定

### Requirement: CAT-08 Removable-volume metadata cleanup
系统 SHALL 支持对用户明确选择的外接/可移动卷清理平台生成的元数据文件（如 AppleDouble、索引与临时目录），仅限该卷根下的已知名称，不跨入其他卷或系统卷。

#### Scenario: A USB drive is selected
- **WHEN** 用户对一个外接卷运行元数据清理预览
- **THEN** 只列出该卷内已知元数据对象及大小，系统卷不出现在结果中

### Requirement: CAT-09 Large-item review rows never auto-delete
目录 SHALL 提供只读“大项提示”行（如设备备份、虚拟机磁盘、容器数据、模型库、邮件数据、构建归档），显示大小与处理指引。此类行 SHALL 不可直接选入清理计划，只能跳转到磁盘分析、应用管理或空间洞察。

#### Scenario: A 40 GiB device backup is found
- **WHEN** 清理扫描发现设备备份目录
- **THEN** 显示为大项提示及最新备份时间，提供“在磁盘分析中查看”，不提供勾选框
