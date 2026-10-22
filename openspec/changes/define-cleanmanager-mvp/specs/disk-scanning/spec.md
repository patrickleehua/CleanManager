# Spec Delta

## Purpose

让用户和 Agent 在 Windows/macOS 上获得指定目录或明确选择卷的磁盘事实，了解哪些范围已读取、哪些仍未知。不同平台可以采用不同扫描机制，但范围、空间字段、错误、取消和完整性必须有一致且可验证的含义。

## ADDED Requirements

### Requirement: SCN-01 Scoped read-only scanning
系统 SHALL 仅枚举用户选择的扫描范围，默认不跟随符号链接、junction/reparse 跳转或跨挂载点扩大范围。扫描 SHALL 不修改目标文件；本工具索引和日志可以写入范围外的用户数据位置。

#### Scenario: A selected folder contains a link to another root
- **WHEN** 用户扫描测试目录，目录内链接指向范围外的 sentinel
- **THEN** 结果识别链接及边界，不遍历 sentinel，也不修改任何目标文件

#### Scenario: The selected root is itself a link
- **WHEN** 用户选择的扫描根本身是符号链接、junction、挂载点或其他扫描器不进入的 reparse point
- **THEN** 系统在写入任何条目前返回 failed 和 `root_is_link`，不跟随链接，提示用户直接扫描链接目标，也不显示完整空目录

#### Scenario: A directory request would require a whole-volume fast path
- **WHEN** 快速后端需要读取该目录之外的整卷命名空间，而请求仅批准目录范围
- **THEN** 系统使用限定范围后端或说明能力不可用，不静默扩大读取范围

### Requirement: SCN-02 Platform capability and fallback
系统 SHALL 按平台、文件系统、范围与当前权限报告扫描能力为 available、permission_required、unsupported 或 unverified，并记录选择后端的理由。不可用的快速路径 SHALL 在范围不扩大且语义可保持时回退到可解释的目录扫描。

#### Scenario: NTFS fast enumeration is not permitted
- **WHEN** 普通用户不能使用卷级元数据能力，但能够枚举选择的目录
- **THEN** 系统完成可读范围的目录扫描并说明回退，不要求先提升权限才能查看结果

#### Scenario: A platform or filesystem has not been verified
- **WHEN** 请求未验证的扫描能力
- **THEN** 系统返回 unverified 或 unsupported 及理由，不继承另一平台的可用声明

### Requirement: SCN-03 Completeness and terminal scan states
系统 SHALL 区分 completed、partial、cancelled 和 failed 终态，记录覆盖范围、跳过范围和已知错误。未扫描子树及无法获取的大小 SHALL 不被报告为零；完成声明 SHALL 仅针对实际已核验的选择范围。

#### Scenario: One subtree is unreadable
- **WHEN** 已读取部分文件，但一个子目录拒绝访问
- **THEN** 返回 partial、已知小计和拒绝范围，不猜测该子目录里的文件数量或字节数

#### Scenario: The root cannot be opened
- **WHEN** 在发现条目前根目录已不存在或读取被拒绝
- **THEN** 返回 failed 和类型化原因，不显示完整空目录

#### Scenario: The root is not a directory or the scan fails
- **WHEN** 选择的根是普通文件，或扫描以 failed 结束
- **THEN** 普通文件根在写入任何条目前以 `not_a_directory` 失败、不被记录为目录；failed 结果的逻辑与分配大小为未知（null），不报告为 0

### Requirement: SCN-04 Cancellation retains confirmed results
系统 SHALL 支持请求取消扫描，停止调度新的工作并保留已确认结果。仍在退场的 I/O SHALL 显示正在停止；未停止前不得发出虚假的完成或取消终态。

#### Scenario: Cancellation arrives during a scan
- **WHEN** 用户在扫描产生部分批次后请求取消
- **THEN** 系统停止后返回 cancelled，仍可查询已确认条目，并标示范围未完整扫描

#### Scenario: Cancellation arrives while one huge directory is listed
- **WHEN** 取消请求在一个包含数万条目的目录列举途中被接受
- **THEN** 系统在下一批确认后停止，返回 cancelled 并保留该批条目；已接受的取消回执不会以 completed 结束

### Requirement: SCN-05 Cloud materialization boundary
系统 SHALL 默认避免读取文件内容或触发云占位文件下载；无法验证无物化行为的 provider、目录或接口 SHALL 跳过并说明未知范围。扫描 SHALL 不将云端同步删除或驱逐数据当作本地缓存清理。

#### Scenario: A provider has no verified metadata-only path
- **WHEN** 扫描遇到尚未验证的云占位目录
- **THEN** 系统跳过该边界、报告部分覆盖，不为取得大小而下载内容

### Requirement: SCN-06 Distinct space measurements
系统 SHALL 分别呈现逻辑大小、分配大小、预计释放量及卷空闲观测，并标明来源与未知值。硬链接、稀疏/压缩文件、APFS clone/快照和共享容量 SHALL 不被简单相加成确定可释放量。

#### Scenario: Two directory entries refer to a hardlinked file
- **WHEN** 同一实体存在两个目录项
- **THEN** 两个位置均可浏览，实体占用采用说明过的去重口径，删除一个名字不被估计为释放全部数据

#### Scenario: Exclusive APFS block ownership is unknown
- **WHEN** 文件系统未提供可验证的独占块信息
- **THEN** 预计释放量保持未知并说明共享块限制，不用逻辑大小替代

### Requirement: SCN-07 Scan freshness and real-platform acceptance
系统 SHALL 记录扫描时间、版本和变化/身份异常，不把扫描声明为静止文件系统的原子快照。双平台能力与性能声明 SHALL 由对应真实设备、同范围和同字段的运行证据支持。

#### Scenario: A mounted volume is replaced during scanning
- **WHEN** 相同盘符或路径对应的卷身份发生变化
- **THEN** 系统停止沿新卷继续旧扫描，报告身份异常并保留旧范围已确认结果

#### Scenario: Only Windows measurements are available
- **WHEN** 尚无真实 Mac 的扫描和权限测试结果
- **THEN** 系统交付材料将 Mac 能力保持未验证，不宣称双平台验收或复制 Windows 性能数字
