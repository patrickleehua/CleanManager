# Spec Delta

## Purpose

把扫描事实转成用户能够理解和审阅的清理候选，同时保留来源、影响和不确定性。规则需要证明对象归属及适用条件，避免因目录名称、体积或年龄就把个人数据认定为垃圾，并为人工和 Agent 提供相同的理由。

## ADDED Requirements

### Requirement: ANA-01 Versioned evidence-backed rules
系统 SHALL 为规则提供固定 ID、版本、适用平台、根解析方式、匹配/排除条件及 experimental、verified、enabled 状态。只有经过对应平台和应用版本验证的规则 SHALL 作为正式可执行候选来源；示例规则不启用。

#### Scenario: An example rule has no platform evidence
- **WHEN** experimental 规则仅通过 JSON 形状校验
- **THEN** 系统不将其提升为 verified 或作为生产清理依据

### Requirement: ANA-02 Ownership and impact explanation
每个可执行候选 SHALL 绑定扫描版本与规则版本，并说明归属证据、匹配理由、风险、再生成/离线影响、允许模式与恢复限制。归属不足的对象 SHALL 保持仅分析或未知，不自动成为低风险候选。

#### Scenario: An arbitrary database path contains temp
- **WHEN** 分析虚构的项目数据库路径且唯一依据是名称含 temp
- **THEN** 系统不把数据库标为已证实可删除的临时文件

### Requirement: ANA-03 Protected user and system content
系统 SHALL 默认保护用户文档、凭据、源码/工作树、数据库、浏览器站点存储、AI 会话/配置和未知来源模型，以及系统修复/安装依赖。保护判定 SHALL 优先于允许规则，保护配置的改变不得扩大旧计划。

#### Scenario: A cache-like parent contains a credential descendant
- **WHEN** 父目录匹配规则，但后代包含被保护的凭据
- **THEN** 系统细分有充分证据的具体对象或拒绝父目录操作，不通过递归处理绕过保护

### Requirement: ANA-04 Cross-rule overlap elimination
系统 SHALL 在候选集合中跨规则处理重复对象和父子包含关系，不重复报告收益，也不让低风险父候选覆盖风险更高或未知的后代。

#### Scenario: Two rules select the same file
- **WHEN** 两条规则命中相同条目
- **THEN** 选择与汇总不会产生两个独立删除动作，保留可解释的匹配理由

### Requirement: ANA-05 Analysis of partial scans
系统 SHALL 根据规则需要的证据范围判断部分扫描中的候选是否成立。未覆盖必要后代、归属或生命周期证据 SHALL 导致待判断/拒绝，不把所有已知条目一概视为完整证明。

#### Scenario: An unreadable subtree could contain protected content
- **WHEN** 父目录规则要求确认所有后代，但该子树未扫描
- **THEN** 系统不批准整个父目录，仍允许证据独立完整的其他候选供审阅

### Requirement: ANA-06 Analysis does not authorize execution
扫描、分析和计划选择 SHALL 不修改目标或自行授予执行授权。候选 SHALL 向人和 Agent 显示相同的风险和允许模式；文件名及第三方文本中的指令不改变规则。

#### Scenario: A filename instructs an agent to approve deletion
- **WHEN** 候选显示包含命令式文字的文件名
- **THEN** 系统把它作为数据呈现，授权状态和保护结果保持由可信流程决定
