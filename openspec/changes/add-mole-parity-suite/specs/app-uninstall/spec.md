# Spec Delta

## Purpose

提供对标 Mole 的完整卸载：按安装来源选择正确方式移除应用本体，列出并分组残留，让个人数据默认保留、系统级残留单独授权，并能清理已通过其他方式卸载后留下的残留。

## ADDED Requirements

### Requirement: UNI-01 Source-specific uninstall adapters
系统 SHALL 按安装实例来源选择卸载方式：macOS 应用包移入废纸篓、Homebrew cask/formula 通过包管理器；Windows 运行注册的卸载程序、MSI 产品卸载、商店/MSIX 包移除、winget 与 Scoop 包。未适配或无法验证的来源 SHALL 显示能力限制并提供系统设置或供应商入口，不以删除安装目录冒充卸载。

#### Scenario: An app was installed by a package manager
- **WHEN** 用户卸载由包管理器安装的应用
- **THEN** 计划显示“由包管理器卸载”及其命令适配，不直接删除包管理器管理的文件

#### Scenario: The source is unknown
- **WHEN** 应用没有可验证的安装来源记录
- **THEN** 只提供本体位置、移入回收站选项与能力说明，残留不自动预选

### Requirement: UNI-02 Running applications are asked to quit
卸载执行前应用或其辅助进程在运行时，系统 SHALL 先请求正常退出并等待有限时间；仍未退出时 SHALL 询问用户是否强制结束，不得自动强制结束。系统关键进程与本工具自身 SHALL 不可被结束；运行状态未知时 SHALL 跳过卸载并说明。

#### Scenario: The app refuses to quit
- **WHEN** 正常退出请求超时
- **THEN** 显示仍在运行的进程并让用户选择强制结束或取消，选择取消时不执行任何删除

### Requirement: UNI-03 Vendor uninstallers run with fixed registered commands
Windows 卸载程序 SHALL 只使用安装注册信息中记录的命令，经路径存在性与签名/发布者核验后执行，保留其自身界面；静默参数仅在适配已验证时使用。系统 SHALL 记录退出码与时长，并在完成后重新扫描本体与残留，而不是根据退出码推断已完全卸载。

#### Scenario: The vendor uninstaller exits successfully but files remain
- **WHEN** 卸载程序返回成功，重新扫描仍发现安装目录
- **THEN** 结果标为部分完成，剩余对象进入残留预览

### Requirement: UNI-04 Leftovers require identity evidence
残留发现 SHALL 基于应用身份证据（bundle ID、包/产品标识、注册表卸载项、发布者签名、已知数据目录约定），名称变体匹配 SHALL 排除通用词。仍有同身份其他安装实例、测试版或无法判定归属的残留 SHALL 标为 shared_owner、owner_unknown 或 app_reappeared 并保留。

#### Scenario: A beta build of the same app is still installed
- **WHEN** 卸载正式版时发现测试版使用同一数据目录
- **THEN** 该目录标为 shared_owner 并保留，不进入卸载计划

### Requirement: UNI-05 Grouped leftover preview with personal data unselected
卸载审阅 SHALL 将对象分组为应用本体、缓存与日志、设置与偏好、个人数据/文档/模型、启动与后台项、系统级位置，显示路径、所有者证据与大小。仅缓存与日志 SHALL 默认选中；个人数据、设置与模型默认不选；匹配敏感模式（凭据、SSH/云配置、文档目录）的对象 SHALL 带警告；引用该应用的外部配置 SHALL 只读提示。

#### Scenario: An app stores models under its data folder
- **WHEN** 审阅含模型目录的应用卸载
- **THEN** 模型目录在“个人数据/模型”组中默认不选，显示大小与重新下载说明

### Requirement: UNI-06 Windows registry leftovers are backed up first
删除注册表残留前，系统 SHALL 导出被删除键值的备份并记录在操作历史中，备份失败 SHALL 阻止对应删除；只处理与应用身份关联的键，不触及共享或系统键。残留文件与快捷方式 SHALL 默认移入回收站。

#### Scenario: Registry backup cannot be written
- **WHEN** 备份位置不可写
- **THEN** 注册表项删除被跳过并报告原因，文件残留仍可按计划处理

#### Scenario: A user restores registry leftovers
- **WHEN** 用户在历史中选择恢复某次注册表删除
- **THEN** 系统生成恢复计划，导入备份前检查目标键是否已被重新创建

### Requirement: UNI-07 AI command-line tools uninstall
系统 SHALL 识别已安装的 AI 命令行工具及其安装来源、版本、最后使用时间与关联会话/设置目录，支持连同会话与设置卸载；会话与设置 SHALL 默认不选，有未提交修改的 worktree SHALL 被阻止。

#### Scenario: A long-unused AI CLI is uninstalled
- **WHEN** 用户卸载一个 90 天未使用的 AI CLI
- **THEN** 本体与缓存默认选中，会话与设置单独列出且需勾选

### Requirement: UNI-08 Orphaned leftovers from earlier removals
系统 SHALL 发现已不存在对应安装实例的应用残留，前提是在所有已挂载应用位置与安装记录中都确认缺席、身份证据明确且超过可声明的闲置期。此类候选 SHALL 为 review_only；系统级残留需管理员授权；归属不足 SHALL 保持未知。

#### Scenario: A launch agent references a deleted app
- **WHEN** 启动项指向已不存在的应用并有明确身份
- **THEN** 列为孤立残留候选，默认不选，说明指向的缺失路径

### Requirement: UNI-09 Teardown steps are reported individually
卸载 SHALL 逐项执行并报告登记清理步骤（卸载启动/后台项、注销服务与辅助程序、移除登录项与开始菜单/Dock 快捷方式、清除应用注册缓存），每步结果为成功、跳过、失败或未知；任一步失败不得报告卸载完全成功。

#### Scenario: A helper cannot be unloaded
- **WHEN** 后台辅助程序注销失败
- **THEN** 报告为部分完成，指出该辅助程序位置与手动处理指引

### Requirement: UNI-10 Protected application classes
系统组件、系统自带应用、终端安全/设备管理代理及本工具自身 SHALL 不提供卸载；安全/管理代理 SHALL 指向其官方卸载流程。用户购买的平台可卸载应用 SHALL 按普通应用处理。

#### Scenario: A user selects an endpoint security agent
- **WHEN** 打开该代理的应用详情
- **THEN** 卸载按钮不可用，说明需通过管理员或厂商流程移除
