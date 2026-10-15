# Spec Delta

## Purpose

让不使用终端的用户完成扫描、理解空间、审阅计划、执行、查明结果与恢复的完整旅程，获得清楚且一致的双平台管理体验。桌面呈现必须能解释部分结果和限制，并与 CLI 共享计划，而不是维护第二套清理判断。

## ADDED Requirements

### Requirement: GUI-01 Complete management journey and page states
桌面 SHALL 提供范围选择、概览、磁盘、候选、计划审阅、执行结果、只读应用清单、活动记录、隔离恢复和设置入口。相关页面 SHALL 具有空、加载、部分、成功、失败和取消状态，保留扫描范围/时间/完整性说明。

#### Scenario: A first-time user has no scan
- **WHEN** 用户打开桌面
- **THEN** 界面引导选择范围并开始普通权限扫描，不展示虚构垃圾或要求先批准删除

#### Scenario: A completed scan has unreadable ranges
- **WHEN** 用户查看磁盘或概览
- **THEN** 保留已知结果和缺口说明，不显示整盘已完整扫描或无权限区域为零

### Requirement: GUI-02 Treemap and virtualized list consistency
磁盘页 SHALL 联动 treemap 与目录/大文件表，采用相同范围、版本、条目 ID 和面积口径，并按需加载层级/分页。未知空间 SHALL 不捏造面积；图表不可用时表格仍可浏览所有结果。

#### Scenario: A user selects a visible rectangle
- **WHEN** 图表和列表来自相同 revision
- **THEN** 列表定位同一条目并显示一致信息，不因图表局部排序选择其他文件

#### Scenario: Chart rendering fails with a large index
- **WHEN** treemap 渲染失败
- **THEN** 页面说明问题并继续提供可排序、过滤和分页的表格

### Requirement: GUI-03 Explain candidates and modes before execution
清理页 SHALL 显示理由、风险、影响、保护情况、允许模式、预计释放依据和恢复限制，不默认勾选未确认候选。计划审阅 SHALL 可以访问完整对象清单，明确区分同卷隔离、回收站和永久删除。

#### Scenario: A person chooses quarantine
- **WHEN** 用户审阅同卷隔离计划
- **THEN** 界面明确说明该移动不立即释放数据空间，并展示实际恢复条件

### Requirement: GUI-04 Shared plans and distinct permissions
GUI SHALL 读取和审阅 CLI 创建的同一计划及摘要，所有动作交由共享核心处理。OS 读取权限、写入提权和产品授权 SHALL 分别呈现；有效预授权策略不引入逐次重复确认。

#### Scenario: An agent-created plan already matches a policy
- **WHEN** 用户在 GUI 查看该计划
- **THEN** 界面显示真实策略来源并允许按核心授权执行，不重建计划或再次索取无意义批准

### Requirement: GUI-05 Keyboard and assistive technology parity
核心扫描、表格浏览、计划审阅、执行结果及恢复旅程 SHALL 可通过键盘和 Narrator/VoiceOver 完成。图表 SHALL 提供功能等价表格；缩放、虚拟化和状态更新不得丢失焦点或只用颜色表达危险。

#### Scenario: A screen-reader user reviews a long plan
- **WHEN** 用户只用键盘和读屏器浏览分页/虚拟列表
- **THEN** 能读到条目名称、状态、模式与错误，并完成审阅，不因滚动卸载节点失去当前焦点

### Requirement: GUI-06 Visible task lifetime and recovery
导航 SHALL 不隐式取消任务；取消 SHALL 显示停止过程和真实终态。界面 SHALL 说明进程退出影响，重启后呈现保留结果或待核对状态，不承诺没有任务宿主也会继续执行。

#### Scenario: The desktop restarts after an interrupted operation
- **WHEN** 存在未确认动作
- **THEN** 活动页显示核对入口及已知结果，不显示清理完成或自动重复计划

### Requirement: GUI-07 Verified distribution and compatibility
公开首版 SHALL 为每个声明支持的系统/CPU 组合提供实际安装、权限、平台签名/公证及升级验证。GUI/CLI 可独立安装；更新 SHALL 检查核心/协议/存储兼容，不丢失待核对日志、恢复清单或用户内容。

#### Scenario: An update changes storage format incompatibly
- **WHEN** 迁移无法可靠完成
- **THEN** 更新停止或回退至可用状态，保留操作/恢复数据并说明问题，不重新执行旧计划
