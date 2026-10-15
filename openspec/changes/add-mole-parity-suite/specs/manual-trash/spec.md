# Spec Delta

## Purpose

允许用户在磁盘分析中对自己看懂的大文件或目录直接“移到回收站/废纸篓”，与 Mole 和 WizTree 的习惯一致，同时保持系统目录只读、可从回收站放回，并沿用固定计划与授权。

## ADDED Requirements

### Requirement: MTR-01 Manual selection creates a trash-only plan
用户 SHALL 能在磁盘分析的 treemap 或列表中选择未命中规则的文件/目录，生成类型为 manual_selection 的固定计划，其唯一允许模式为 trash（平台回收站/废纸篓）。该计划 SHALL 不可改为隔离或永久删除，也 SHALL 不被任何预授权策略匹配。

#### Scenario: A user trashes a large video from the treemap
- **WHEN** 用户右键视频选择“移到回收站”并确认计划
- **THEN** 计划只含该对象、模式为 trash、需本次人工授权，执行后对象出现在系统回收站并可放回

#### Scenario: A preauthorization policy exists
- **WHEN** 已启用策略且 Agent 请求为手选对象创建计划
- **THEN** 计划状态为 needs_approval，不因策略自动执行

### Requirement: MTR-02 System and protected locations are view-only
处于硬拒绝清单、用户保护名单、受保护类目或其祖先关系中的对象 SHALL 只能查看、定位与诊断，界面不提供移入回收站操作，CLI 请求 SHALL 返回 policy_denied。选择目录时 SHALL 检查已知后代是否受保护；后代未完全扫描时 SHALL 拒绝整体处理。

#### Scenario: A user selects the system program folder
- **WHEN** 在磁盘分析中右键系统程序目录
- **THEN** 菜单只有显示位置、查看详情与诊断，没有移入回收站

#### Scenario: A folder contains an unscanned subtree
- **WHEN** 所选目录存在未读取子树
- **THEN** 系统拒绝整体处理并说明覆盖缺口，可改选已知后代

### Requirement: MTR-03 Permanent delete only from the trash
永久删除 SHALL 仅针对已位于平台回收站/废纸篓中的对象，作为单独计划并显示“不可恢复”确认；磁盘分析中的其他对象 SHALL 不提供直接永久删除。回收站不可用时 SHALL 报错，不降级为永久删除。

#### Scenario: The volume has no trash
- **WHEN** 所选对象所在卷不支持回收站
- **THEN** 计划创建失败并说明原因，不提供“直接删除”替代按钮

#### Scenario: A user empties selected trash items
- **WHEN** 用户在回收站视图选择两个对象永久删除
- **THEN** 生成独立计划，确认对话明确不可恢复，执行后报告释放观测

### Requirement: MTR-04 Show which process holds a file
对象因被占用而无法移动时，系统 SHALL 在平台支持时显示占用该文件的进程名称与标识，并提供关闭后重试的指引；系统 SHALL 不自动结束占用进程。平台不支持查询时 SHALL 显示“被占用，占用者未知”。

#### Scenario: A log file is locked on Windows
- **WHEN** 移入回收站因文件被占用失败
- **THEN** 结果显示占用进程名称，条目标为 failed（被占用），可重试

### Requirement: MTR-05 Reveal and inspect without side effects
磁盘分析中的任何对象 SHALL 支持在系统文件管理器中显示、快速预览（平台支持时）、复制路径与发起 AI 诊断，这些操作 SHALL 不修改目标或物化云占位。

#### Scenario: A user reveals a cloud placeholder
- **WHEN** 选择云占位文件并“在文件管理器中显示”
- **THEN** 打开所在目录且不触发下载
