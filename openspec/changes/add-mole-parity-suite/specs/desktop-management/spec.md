# Spec Delta

## ADDED Requirements

### Requirement: GUI-08 Five-tool information architecture with extensions
桌面 SHALL 以“清理、软件、优化、分析、状态”五个主工具为一级入口，并以“计划、活动、恢复、Agent、AI 诊断、设置”为同一导航中的扩展入口；两平台的入口顺序、名称与快捷键 SHALL 一致，平台不支持的功能在原位置显示能力说明而不是隐藏。

#### Scenario: A first-time user opens the app
- **WHEN** 打开桌面
- **THEN** 看到五个主工具和扩展入口，清理工具引导开始扫描，其他入口可直接浏览

#### Scenario: A macOS-only feature is viewed on Windows
- **WHEN** Windows 用户进入只在 macOS 提供的功能位置
- **THEN** 显示不适用说明，不出现空白页

### Requirement: GUI-09 Shared selection and review pattern
清理、构建产物、安装包、卸载残留与维护任务 SHALL 使用统一模式：摘要 → 分组 → 可展开明细；明细行显示三档徽标、大小与理由；底部选择栏显示已选数量、已知大小与未知项；任何写动作 SHALL 先进入计划审阅，再执行并展示逐项结果。

#### Scenario: A user selects items across two groups
- **WHEN** 在两个分组勾选对象
- **THEN** 选择栏显示合计数量与已知大小，未知大小单独计数，点击后进入同一计划审阅

### Requirement: GUI-10 Consistent result summaries
所有写操作完成页 SHALL 使用同一结构展示处理数量、已处理逻辑量、预计释放、卷空闲前后观测、跳过/失败/未知及其原因；部分测量时 SHALL 显示“至少”而非精确值。

#### Scenario: Sizing timed out for some items
- **WHEN** 部分对象大小未测完
- **THEN** 结果显示“至少 X”及未测项数量，不显示伪精确总数

### Requirement: GUI-11 Original visual identity aligned with reference interactions
界面 SHALL 采用本产品自有的设计令牌（颜色、字体、间距、图标）并提供浅色/深色主题，可参考对标产品的交互节奏与信息层级，但 SHALL 不使用其名称、标志、吉祥物、星球插画、文案或其他品牌资产。

#### Scenario: A design review checks assets
- **WHEN** 发布前审查界面资源
- **THEN** 所有图形与文案为原创或许可明确，资源清单无对标产品素材

### Requirement: GUI-12 Platform-native affordances
对象操作 SHALL 使用平台惯用语与入口：macOS“废纸篓/在访达中显示”、Windows“回收站/在资源管理器中显示”，右键菜单、快速预览与快捷键遵循平台约定；跨平台行为语义保持一致。

#### Scenario: The same plan is shown on both platforms
- **WHEN** 审阅移入回收站计划
- **THEN** macOS 显示“移到废纸篓”，Windows 显示“移到回收站”，计划字段与结果含义相同

### Requirement: GUI-13 Permission readiness before long operations
在扫描、清理或卸载开始前，桌面 SHALL 检查所需系统权限（如完全磁盘访问、管理员、辅助功能）并一次性说明缺口与影响，允许以当前权限继续；运行中 SHALL 不弹出可避免的权限请求打断任务。

#### Scenario: Full Disk Access is missing
- **WHEN** 用户在未授予完全磁盘访问时开始清理扫描
- **THEN** 先显示哪些类目因此不可读并提供授权指引，用户可选择继续扫描可读部分

### Requirement: GUI-14 Responsive progress for every long step
超过约一秒的操作 SHALL 显示进度或活动指示及当前阶段；分母未知时不显示伪百分比。异步结果 SHALL 带生成序号，过期结果不得覆盖当前视图。

#### Scenario: A slow uninstall scan finishes after the user switched apps
- **WHEN** 旧应用的残留结果在用户选中另一应用后返回
- **THEN** 当前视图不被旧结果替换
