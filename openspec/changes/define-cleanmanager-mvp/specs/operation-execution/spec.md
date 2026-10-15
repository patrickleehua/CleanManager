# Spec Delta

## Purpose

把已授权计划执行为范围确定、可追踪的系统动作，并在文件变化、权限变化、并发请求和中断时保留真实结果。平台执行能力必须满足它声称的身份与恢复约束，无法保证的动作要明确拒绝，不能由前端临时扩大范围或切换删除方式。

## ADDED Requirements

### Requirement: EXE-01 Live precondition validation
执行器 SHALL 在动作开始前核验计划/授权有效性、卷与对象身份、类型、相关观察、父范围、保护规则和链接/挂载/云边界。对象变化 SHALL 被记录为 stale 或跳过；不能用同名新对象替换原对象。

#### Scenario: The source pathname now names a different file
- **WHEN** 计划生成后原条目被替换
- **THEN** 系统拒绝该条目动作，保留替代文件，并报告身份不匹配

### Requirement: EXE-02 Capability-limited filesystem actions
系统 SHALL 仅执行已验证能满足对应范围与身份要求的 quarantine、trash 或 delete 动作。无法安全约束名称竞态或权限边界时 SHALL 返回 unsupported/跳过；路径规范化、Rust 内存安全或一次检查不足以证明动作安全。

#### Scenario: A backend cannot meet strict identity requirements
- **WHEN** 测试证明检查与 rename/unlink 之间仍可替换目标且当前能力无法约束
- **THEN** 对应严格自动执行能力保持不可用，不以告警文案替代拒绝

#### Scenario: A trash action is unavailable
- **WHEN** 当前卷或权限不支持所选回收站动作
- **THEN** 返回明确错误，不静默改用永久删除或跨卷复制后删除

### Requirement: EXE-03 Single operation for repeated plan submissions
同一计划 SHALL 不重复产生副作用；重复提交或相同去重键/摘要 SHALL 返回已登记 Operation。相同键对应不同参数 SHALL 返回 conflict；未知结果不得作为未执行重跑。

#### Scenario: GUI and CLI apply the same plan simultaneously
- **WHEN** 两个入口提交相同 plan_id
- **THEN** 只有一份执行登记，后续请求返回其状态而不再次移动或删除

### Requirement: EXE-04 Conflict detection across different plans
系统 SHALL 防止不同计划同时修改重叠的实体、目录项或范围。发现正在进行或待核对的冲突 SHALL 拒绝新动作，并提供可查询的冲突说明。

#### Scenario: Two different plans target overlapping directories
- **WHEN** 第一份计划正在执行，第二份计划包含相同或祖先/后代对象
- **THEN** 第二份计划返回 conflict，不因 plan_id 不同而绕过互斥

### Requirement: EXE-05 Durable action outcomes and crash reconciliation
每项动作 SHALL 保留执行意图和结果。OS 动作可能生效但结果未确认时 SHALL 标记 action unknown、Operation reconciliation_required，并先核对实际对象；不得声称整批原子回滚或自动重复副作用。

#### Scenario: The process stops after moving an object but before recording success
- **WHEN** 重启后发现有意图而没有可信终态
- **THEN** 系统进入核对，保留源/目标证据，不直接重做动作或把它算作失败未执行

### Requirement: EXE-06 Cancellation and authorization changes
取消、授权撤销或到期 SHALL 停止开始新的动作，并保留已完成/失败/未开始项目。正在执行的 OS 动作 SHALL 在可安全退出时完成记录；取消不意味着撤销已发生的操作。

#### Scenario: The user cancels after two items have succeeded
- **WHEN** 后续项目尚未开始
- **THEN** 已成功项目保留真实结果，后续项目标记未开始，终态为取消或需核对而非全部回滚

### Requirement: EXE-07 Honest per-item reports
报告 SHALL 区分 succeeded、skipped、failed、not_started 和 unknown，并与逐项明细一致。存在未知副作用时 SHALL 需要核对；存在跳过或失败时不得报告全部完成。

#### Scenario: One file is busy while another succeeds
- **WHEN** 一个条目完成，另一个因前置条件未满足跳过
- **THEN** 报告为 partial，成功与跳过数量准确，不用扫描时大小声称全部清理成功

### Requirement: EXE-08 Restricted privilege boundary
普通范围动作 SHALL 优先以用户权限执行；特权能力 SHALL 验证真实调用者、限定操作类型及可信授权，并拒绝任意 shell/可执行路径/开放递归删除请求。高权限不改变计划范围或保护规则。

#### Scenario: A helper request contains an arbitrary command
- **WHEN** 调用方尝试把命令或模型生成脚本提交给 helper
- **THEN** 系统拒绝未定义请求，不启动通用提权终端或执行该脚本
