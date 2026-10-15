# Spec Delta

## Purpose

让用户把任何想保留的路径或类目加入保护名单，并由不可覆盖的平台硬拒绝清单兜底，使清理、卸载、维护和手选回收站等所有写入口共享同一套“不能碰”的判定。

## ADDED Requirements

### Requirement: PRT-01 User protection list shared by all entries
系统 SHALL 维护用户保护名单，支持精确路径、通配模式及“保护后代/保护祖先”语义，也支持保护整个规则或维护任务。任何候选、残留或手选对象行 SHALL 可一键加入保护；GUI 与 CLI SHALL 读写同一名单，新增保护立即排除尚未开始的动作。

#### Scenario: A user protects a cache from the cleanup page
- **WHEN** 用户在某候选行选择“加入保护”
- **THEN** 该对象从当前选择移除并显示受保护，随后 CLI 查询同一扫描时也将其列为受保护

#### Scenario: Protection is added while a plan is pending
- **WHEN** 已授权计划尚未执行到被新保护覆盖的对象
- **THEN** 该对象在执行时标为 skipped（受保护），已完成的对象不回滚

### Requirement: PRT-02 Fail closed on unreadable or invalid protection data
保护名单无法读取、解析失败或版本不兼容时，系统 SHALL 停止所有写操作并说明原因，只读扫描与查询可以继续。名单中的相对路径、路径穿越或指向系统硬拒绝区域的条目 SHALL 被拒绝并提示，不静默忽略。

#### Scenario: The protection file is corrupted
- **WHEN** 应用计划前发现保护名单无法解析
- **THEN** 返回 protection_unavailable，计划不执行，提供修复或恢复默认名单的入口

### Requirement: PRT-03 Non-overridable platform hard-deny list
系统 SHALL 内置按平台定义的硬拒绝清单，涵盖系统根与系统目录、程序安装根、用户主目录根、凭据/钥匙串、系统更新与组件存储、卷元数据、终端安全软件数据及本工具自身的审计/恢复存储。硬拒绝 SHALL 优先于所有规则、保护名单例外、手选与授权，按卷与文件身份比较以防大小写或链接别名绕过。

#### Scenario: A manual selection resolves to a system directory through a link
- **WHEN** 用户选择的对象经链接或别名指向硬拒绝目录
- **THEN** 计划创建被拒绝并显示被拒绝的真实位置，原链接对象不被处理

#### Scenario: A case variant of a system path is submitted
- **WHEN** CLI 请求使用不同大小写指向同一系统目录
- **THEN** 系统按身份识别并拒绝

### Requirement: PRT-04 Ancestor links can only deny
路径或其任一祖先为符号链接、junction 或挂载点时，系统 SHALL 用解析后的真实位置额外检查保护与硬拒绝；解析结果只能增加拒绝，不能让原本被拒绝的对象变为允许。解析失败 SHALL 视为拒绝。

#### Scenario: A cache directory is a junction into a protected folder
- **WHEN** 规则命中的目录本身是指向受保护位置的 junction
- **THEN** 对象被拒绝，理由指出链接目标受保护

### Requirement: PRT-05 Dangerous-path regression corpus
发布 SHALL 维护双平台危险路径语料（系统根、编码异常、控制字符、穿越、大小写/Unicode 变体、链接、UNC/设备路径等）及其期望拒绝结果，作为每次规则、保护或执行器变更的必过回归与模糊测试种子。

#### Scenario: A rule change is proposed
- **WHEN** 新规则或执行器变更进入发布候选
- **THEN** 语料中所有期望拒绝的输入仍被拒绝，任何放行导致发布阻断

### Requirement: PRT-06 Maintenance task opt-out
用户 SHALL 能把单个系统维护任务或启动项加入保护，使其在任务目录中显示为 skipped（用户保护），不再被批量选择。

#### Scenario: A user opts out of a maintenance task
- **WHEN** 用户保护“刷新图标缓存”任务后运行维护
- **THEN** 该任务结果为 skipped 并注明用户保护，其他任务照常执行
