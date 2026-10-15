# Spec Delta

## Purpose

在一个页面查看并管理随系统或登录启动的程序与后台服务，说明每一项的来源与影响，只对可验证的条目提供启停，并能在备份后清理指向已不存在程序的失效项。

## ADDED Requirements

### Requirement: STA-01 Startup inventory with sources
系统 SHALL 列出登录项、后台项与启动项：macOS 包括 Service Management 登录/后台项及用户/系统 launchd 作业；Windows 包括启动文件夹、Run 类注册项、计划任务的登录/开机触发与商店应用启动任务。每项 SHALL 显示名称、来源、目标程序、发布者证据、作用范围（用户/系统）与当前状态。

#### Scenario: A Windows Run entry points to an app
- **WHEN** 打开启动项页面
- **THEN** 该项显示来源为注册表 Run、目标路径、发布者与启用状态

### Requirement: STA-02 Toggle only validated items
启停 SHALL 仅对来源与目标已验证且平台提供受支持接口的条目开放；无法匹配或受系统管理的条目 SHALL 打开系统设置对应页面，不猜测修改。系统级条目 SHALL 需要管理员授权。每次变更 SHALL 记录并可恢复原状态。

#### Scenario: An unmatched background item is shown
- **WHEN** 后台项无法对应到具体作业或程序
- **THEN** 提供“在系统设置中打开”，不提供开关

#### Scenario: A user re-enables a disabled item
- **WHEN** 用户在历史中恢复之前禁用的启动项
- **THEN** 系统按记录恢复原状态并报告结果

### Requirement: STA-03 Broken entries are backed up before removal
目标程序已不存在的启动项 SHALL 被标为失效；删除失效项前 SHALL 备份其定义，备份失败则不删除。失效判定 SHALL 检查所有已挂载卷，卷离线时 SHALL 标为未知而非失效。

#### Scenario: The target lives on an offline drive
- **WHEN** 启动项指向当前未挂载卷上的程序
- **THEN** 状态为“目标不可达（卷离线）”，不作为失效项删除

### Requirement: STA-04 Security and system entries are protected
系统组件、终端安全/设备管理代理与本工具自身的启动项 SHALL 只读显示，不提供停用或删除。

#### Scenario: A user views an antivirus startup entry
- **WHEN** 选择安全软件的启动项
- **THEN** 开关不可用并说明受保护原因
