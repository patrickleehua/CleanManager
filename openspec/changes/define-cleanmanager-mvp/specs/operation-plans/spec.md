# Spec Delta

## Purpose

为人工和 Agent 提供可以交接、审阅和验证的同一份操作计划，明确具体对象、操作模式、前置条件与失效时间。内容完整性与用户授权是两个独立事实，计划不能自行批准，也不能在执行时扩展到新增或未审阅的文件。

## ADDED Requirements

### Requirement: PLN-01 Finite immutable plans
系统 SHALL 将候选选择冻结为有限的对象清单，绑定主体/范围、扫描版本、规则与策略版本、当前身份和前置观察。每份计划 SHALL 采用一个明确模式；改变集合、模式或绑定版本须生成新计划，不现场展开 glob 或新增后代。

#### Scenario: A new file appears after plan creation
- **WHEN** 计划冻结后源目录出现新文件
- **THEN** 新文件不属于该计划，也不因目录处理而被隐式纳入

#### Scenario: A user changes from quarantine to delete
- **WHEN** 用户修改已经生成的计划模式
- **THEN** 系统创建具有新摘要的计划并重新判断授权，旧计划保持可审计

### Requirement: PLN-02 Verifiable body and complete preview
计划 SHALL 提供 ID、完整可分页清单、固定 body 的规范化 SHA-256 摘要、创建/失效时间和关联说明。同一 body 在所有入口 SHALL 得到同一摘要；摘要不证明对象仍存在或用户已经授权。

#### Scenario: CLI hands a plan to the desktop
- **WHEN** 用户在 GUI 打开 CLI 创建的 plan_id
- **THEN** GUI 展示存储的同一 body/摘要，完整对象可访问，不重新生成近似计划

#### Scenario: A client tampers with a serialized body
- **WHEN** 清单、模式或绑定版本被修改而摘要未匹配
- **THEN** 系统拒绝该输入，不采用客户端指定的批准状态

### Requirement: PLN-03 Expiration and unchanged validation duties
计划 SHALL 默认在创建后 900 秒到期。时间达到 expires_at 后 SHALL 不启动新动作；重试或重启不延长旧计划。有效期内 SHALL 仍按当前对象、保护规则和授权重新核验。

#### Scenario: A request arrives at the expiry boundary
- **WHEN** 当前时间等于或晚于 expires_at
- **THEN** 系统返回 plan_stale，保留旧计划，用户需重新计划

### Requirement: PLN-04 Independent trusted authorization
写操作 SHALL 由可信人工交互或用户已配置的有限策略授权，授权绑定计划摘要、主体、模式、范围、有效期及撤销状态。OS 提权、plan hash、请求 ID、approved 字段、非交互或跳过提示参数 SHALL 不构成批准。

#### Scenario: A privileged agent submits an unauthorized plan
- **WHEN** Agent 拥有管理员权限，但计划缺少有效授权
- **THEN** 系统返回 needs_approval，目标保持未改动，并提供计划审阅入口

### Requirement: PLN-05 Bounded preauthorization without repeated prompts
有限策略 SHALL 默认关闭，由用户主动配置精确根/卷、规则版本、模式、风险、生命周期判据、有效期及单次/累计限额。完整命中有效策略的计划 SHALL 可直接执行并审计，未知、越界或超限时拒绝自动授权。

#### Scenario: A recurring cache plan fully matches an enabled policy
- **WHEN** 计划符合所有策略条件且可用额度足够
- **THEN** 系统使用该策略授权而不重复询问，记录策略版本与额度消耗

#### Scenario: A rule is upgraded or the target size is unknown
- **WHEN** 新规则版本未纳入策略，或计划无法核算规模
- **THEN** 系统拒绝自动匹配，不自动扩大旧策略

### Requirement: PLN-06 Atomic policy quotas and revocation
系统 SHALL 跨进程保留和核算单次/累计额度，避免拆批或并发规避限制。重试不重复扣同一已登记动作；已产生副作用的额度不作为未执行释放。撤销/到期 SHALL 阻止尚未开始的新动作。

#### Scenario: Two clients consume the remaining policy allowance
- **WHEN** 两份计划各自符合单次限额，但合计超过累计余额
- **THEN** 额度保留具有统一结果，超限计划无法启动，不因两个入口同时提交而绕过

### Requirement: PLN-07 Shared user-level threat boundary
系统 SHALL 说明产品授权规则约束本工具入口，不承诺隔离同用户任意 shell 或恶意进程。特权操作 SHALL 使用能核验 caller 与授权范围的独立可信边界，不只信任用户可写的计划/配置文件。

#### Scenario: A privileged helper receives a user-edited approval record
- **WHEN** 调用方仅提交可自行修改的 approved=true 或用户数据库记录
- **THEN** helper 不据此认定特权授权，无法验证可信凭据时拒绝动作
