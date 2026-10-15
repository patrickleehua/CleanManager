# Spec Delta

## Purpose

解释那些体积大、看起来像垃圾却不该随手删的系统与虚拟化文件（休眠/分页文件、虚拟磁盘、设备备份、快照、组件存储等），给出它们是什么、能否缩小以及正确的处理入口。

## ADDED Requirements

### Requirement: DIN-01 Known large-object explanations
磁盘分析与概览 SHALL 识别平台已知的大型对象类别（Windows 休眠与分页文件、组件存储、系统还原点、WSL/虚拟机磁盘映像、Hyper-V/容器数据；macOS 本地快照、虚拟机/容器数据、设备备份、系统数据类别），在对象详情显示用途、通常大小影响因素与安全的处理途径。识别 SHALL 基于位置与平台元数据，无法确认时显示为未知。

#### Scenario: hiberfil.sys is selected
- **WHEN** 用户在磁盘分析选择休眠文件
- **THEN** 详情说明其用途与通过系统电源设置调整的途径，不提供移入回收站

#### Scenario: A WSL disk image is large
- **WHEN** 选择 WSL 虚拟磁盘
- **THEN** 说明它包含 Linux 发行版数据、在分发内清理与压缩的途径，以及直接删除会丢失发行版

### Requirement: DIN-02 Insights are read-only guidance
空间洞察 SHALL 只提供解释、在系统设置或所属工具中打开的入口与外部操作说明，不直接修改系统设置、删除快照或缩减系统文件。需要系统工具执行的步骤 SHALL 标明需管理员权限与潜在影响。

#### Scenario: A user asks to shrink the component store
- **WHEN** 打开组件存储洞察
- **THEN** 显示系统自带清理工具的说明与影响，产品本身不执行组件清理

### Requirement: DIN-03 Overview insight cards
概览 SHALL 汇总可查看的洞察卡片（旧下载、设备备份、快照数量、虚拟机/容器、模型库、开发工具大目录等），每张卡片显示已知大小或“未知”及最后观测时间，点击跳转到磁盘分析的对应位置。快照仅在平台无法提供可靠大小时显示数量。

#### Scenario: Snapshot sizes are unavailable
- **WHEN** 平台只能列出本地快照而不能给出独占大小
- **THEN** 卡片显示快照数量与“大小不可得”，不估算为 0 或逻辑总和

### Requirement: DIN-04 Old downloads are reviewed, not assumed junk
“旧下载”洞察 SHALL 依据可声明的年龄阈值列出下载目录中的旧文件，仅作为审阅入口；文件进入处理时 SHALL 走手选回收站或安装包清理流程，而不是作为默认可选的清理候选。

#### Scenario: Downloads has files older than 90 days
- **WHEN** 概览显示旧下载卡片
- **THEN** 打开后为按大小排序的列表，没有预选项

### Requirement: DIN-05 Cleanable directory markers
磁盘分析 SHALL 在目录条目上标记“可手动清理”线索，依据只能是可验证的签名（`CACHEDIR.TAG`、构建产物识别器的项目签名、已启用目录规则覆盖的缓存位置），并给出前往构建产物清理或手选回收站的入口。标记 SHALL 只读、不预选、不改变档位或保护判定；签名无法确认时 SHALL 不标记。

#### Scenario: A directory carries CACHEDIR.TAG
- **WHEN** 磁盘视图列出一个含有效 `CACHEDIR.TAG` 的目录
- **THEN** 条目显示“可手动清理”及依据，选择状态不变，并提供手选回收站入口

#### Scenario: A protected directory looks like a cache
- **WHEN** 受保护目录内含缓存签名
- **THEN** 条目不显示可清理入口，保护判定优先
