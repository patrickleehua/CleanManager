# Spec Delta

## Purpose

以固定、可解释的维护任务目录替代“一键加速”：每项任务说明解决什么问题、是否适用当前系统、是否需要管理员权限，执行后逐项报告真实结果，并明确不做注册表优化、关闭服务或系统精简。

## ADDED Requirements

### Requirement: MNT-01 Fixed per-platform task catalog
系统 SHALL 提供版本化维护任务目录，每项任务有 ID、平台、解决的问题、影响、是否需要管理员、适用条件与预计耗时。macOS 至少覆盖快速预览/图标缓存刷新、DNS 刷新、Spotlight 检查、损坏偏好文件检测、过期保存状态清理与磁盘校验；Windows 至少覆盖重启开始菜单与搜索、图标/缩略图缓存刷新、DNS 刷新、到期的固态盘 TRIM 与系统映像健康检查。

#### Scenario: A user opens the maintenance page
- **WHEN** 查看维护任务
- **THEN** 每项显示问题描述、影响、权限要求与适用性，不出现“加速 x%”式承诺

### Requirement: MNT-02 Applicability before execution
每项任务执行前 SHALL 判定适用性（平台版本、硬件、设置状态、上次执行时间等），不适用的任务 SHALL 自动跳过并给出原因；有风险或环境不明确时 SHALL 跳过而不是尝试执行。

#### Scenario: TRIM is not due
- **WHEN** 固态盘最近一次优化在周期内
- **THEN** TRIM 任务结果为 unchanged 并显示上次执行时间

### Requirement: MNT-03 Six explicit task outcomes
每项任务结果 SHALL 为 applied、unchanged、skipped、unavailable、attention 或 failed 之一，并附原因与可读摘要；attention 用于需要用户后续处理的发现。整体报告 SHALL 列出各结果数量，存在 failed 或 attention 时不得显示“全部完成”。

#### Scenario: Disk verification reports a problem
- **WHEN** 磁盘校验发现错误
- **THEN** 该任务结果为 attention，说明发现与建议的系统修复途径

### Requirement: MNT-04 Batched administrator authorization
需要管理员权限的任务 SHALL 在开始前集中列出并一次性请求提权；用户拒绝时这些任务 SHALL 为 skipped，其余任务继续。提权 SHALL 只用于目录中定义的固定动作，不接受任意命令。

#### Scenario: The user declines elevation
- **WHEN** 管理员授权被拒绝
- **THEN** 需要管理员的任务全部 skipped（未授权），用户级任务正常执行

### Requirement: MNT-05 Prohibited system tweaks
维护目录 SHALL 不包含修改注册表性能/隐私调优项、禁用或删除系统服务、系统精简/组件移除、内存“释放”、重建全部应用注册数据库等高风险或效果不可证的动作。

#### Scenario: A user searches for a registry optimizer
- **WHEN** 在维护页搜索“注册表优化”
- **THEN** 无对应任务，界面说明产品不提供此类操作的原因

### Requirement: MNT-06 Read-only performance diagnosis
系统 SHALL 提供只读性能诊断，报告持续高 CPU 进程、内存/交换压力、长时间失控进程、闲置虚拟机与未推出的磁盘镜像，并给出可选的后续动作入口；诊断本身 SHALL 不结束进程或更改设置。

#### Scenario: A runaway process is detected
- **WHEN** 某进程长时间保持高 CPU
- **THEN** 诊断列出该进程、持续时间与所属应用，提供在状态页查看，不自动结束

### Requirement: MNT-07 Maintenance runs are recorded
每次维护运行 SHALL 记录任务版本、执行者入口、提权情况、逐项结果与时间，进入活动历史；维护动作 SHALL 不提供通用“撤销”，但对有记录的可逆设置变更提供恢复入口。

#### Scenario: A user reviews last week's maintenance
- **WHEN** 打开活动历史中的维护记录
- **THEN** 显示各任务结果与原因，不可逆任务没有撤销按钮
