# Spec Delta

## Purpose

实时显示电脑运行状态，帮助用户判断变慢或发热的真实原因，并解释正在运行的进程；状态只用于观察与诊断，不暗示需要清理，也不自动处置进程。

## ADDED Requirements

### Requirement: STS-01 Live metrics with declared availability
系统 SHALL 采集并显示 CPU（总体/每核/负载）、GPU、内存（已用/可用/压缩或缓存/交换/压力）、磁盘（容量、可清除空间、读写速率、健康状态）、网络（各接口速率与历史）、电池（电量、健康、循环）、温度与风扇。每个指标 SHALL 声明 available、unavailable 或 permission_required，不可得的指标显示为未知而非 0。

#### Scenario: Fan sensors are not exposed on this hardware
- **WHEN** 平台不提供风扇读数
- **THEN** 风扇卡片显示不可用及原因，其他指标正常刷新

### Requirement: STS-02 Health score with published weights
系统 SHALL 计算 0–100 健康分，公开各维度权重与阈值，并显示拉低分数的具体维度。健康分 SHALL 不因清理候选数量变化而变化，也不以“清理后提升”作为宣传。

#### Scenario: Memory pressure lowers the score
- **WHEN** 内存压力超过阈值
- **THEN** 健康分下降并在说明中指出内存维度及当前值

### Requirement: STS-03 Diagnosis line by priority
状态页 SHALL 按优先级给出单句诊断（磁盘健康告警、最高占用进程、内存压力、剩余空间不足、电池、温度、I/O），无异常时显示正常；诊断 SHALL 链接到对应详情。

#### Scenario: Free space is critically low
- **WHEN** 系统卷剩余空间低于阈值且无更高优先级问题
- **THEN** 诊断显示空间不足并链接到磁盘分析

### Requirement: STS-04 Process list with explanations
进程列表 SHALL 支持按 CPU、内存、磁盘、网络排序、搜索与置顶，并显示进程的可执行位置、签名/发布者、所属应用与已知用途说明；未知进程 SHALL 显示为未识别，可发起 AI 诊断。状态页 SHALL 不提供结束进程以外的处置，结束进程需逐次确认且系统关键进程不可结束。

#### Scenario: A user opens an unfamiliar process
- **WHEN** 点击一个未知名称的进程
- **THEN** 显示路径、签名与父进程等本地事实，标记用途未识别，并提供 AI 诊断入口

### Requirement: STS-05 Sustained high-usage alerts are opt-in
系统 SHALL 支持用户开启的持续高 CPU 提醒，阈值与时间窗可配置并有默认值；提醒仅在常驻组件运行时生效，关闭后不再采样。

#### Scenario: Alerts are enabled
- **WHEN** 某进程在设定窗口内持续超过阈值
- **THEN** 发送一次提醒，点击打开该进程详情，同一进程在冷却期内不重复提醒

### Requirement: STS-06 Machine-readable status stream
CLI SHALL 提供单次状态快照与按间隔输出的 JSONL 状态流，字段含采样时间、可用性与完整性标记；过期样本保留原采样时间，不伪装为最新。

#### Scenario: An agent watches status for one minute
- **WHEN** 以 5 秒间隔请求状态流
- **THEN** 每行为一个带时间戳的快照，不可用指标为 null 并带原因

### Requirement: STS-07 Display preferences and deduplicated network totals
系统状态 SHALL 跨会话保留用户的显示偏好（面板显示与顺序、进程排序、刷新间隔）。网络合计 SHALL 排除回环接口，并把隧道/虚拟适配器的流量单独显示而不重复计入合计；无法区分时 SHALL 注明合计可能重复。macOS 在可得时显示僵尸进程计数；Windows 没有该概念，不显示该字段。

#### Scenario: A VPN tunnel is active
- **WHEN** 物理网卡与 VPN 隧道同时有流量
- **THEN** 合计只计物理接口，隧道流量单独列出

#### Scenario: Preferences survive a restart
- **WHEN** 用户隐藏某个面板并改变进程排序后重启应用
- **THEN** 状态页按上次的偏好显示
