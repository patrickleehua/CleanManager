# Spec Delta

## Purpose

帮开发者在项目目录中找出体积大的构建产物与依赖目录，基于项目根证据、Git 跟踪状态和可重建性给出可审阅的清理候选，避免把源码、密钥或近期工作当作垃圾。

## ADDED Requirements

### Requirement: PUR-01 Project roots from explicit markers
系统 SHALL 只在用户配置或默认的项目扫描根内、有限深度下，通过项目标记文件（包管理清单、构建文件、版本库目录、monorepo 工作区标记等）识别项目根。构建产物目录名本身 SHALL 不被当作项目容器继续搜索；扫描根列表 SHALL 可在 GUI 与 CLI 查看和修改。

#### Scenario: A folder named build has no project marker above it
- **WHEN** 扫描根内某个 build 目录的祖先没有项目标记
- **THEN** 它不成为构建产物候选

#### Scenario: A user adds a custom project root
- **WHEN** 用户在设置中加入新的项目扫描根
- **THEN** 下次扫描包含该根，CLI 查询扫描根时同样返回它

### Requirement: PUR-02 Artifact classes by rebuildability
构建产物 SHALL 按可重建性分类：本地编译可重建的输出（编译目录、框架构建缓存、测试/类型检查缓存）与需联网重新获取的依赖目录（如 node_modules、Pods、vendor、虚拟环境）。前者可为 review_only 或经实测为 default_selected；后者 SHALL 始终为 review_only 并说明重新安装代价。目录带缓存目录签名时 SHALL 作为额外证据。

#### Scenario: A project has both target and node_modules
- **WHEN** 同一项目同时发现编译输出和依赖目录
- **THEN** 两者分开展示，依赖目录默认不勾选并注明需联网重新安装

### Requirement: PUR-03 Version-control tracked content is never purged
候选目录内含有版本库元数据、被版本控制跟踪的文件或疑似密钥文件时，系统 SHALL 将其 blocked；无法判定跟踪状态时 SHALL 保留并标记扫描不完整。该检查 SHALL 在发现时与执行前各进行一次。

#### Scenario: A dist directory contains committed files
- **WHEN** Git 显示 dist 中有已跟踪文件
- **THEN** 该 dist 为 blocked，理由显示已跟踪文件数量

#### Scenario: Git is unavailable
- **WHEN** 无法运行版本控制查询
- **THEN** 受影响候选保留不选，结果标记“跟踪状态未知”

### Requirement: PUR-04 Recent projects are not preselected
项目在最近 7 天内有修改时，其产物 SHALL 标记为近期且不预选；非交互调用 SHALL 不自动包含近期项。阈值 SHALL 作为可配置设置并记录在计划中。

#### Scenario: A project was edited yesterday
- **WHEN** 用户查看构建产物列表
- **THEN** 该项目产物显示“近期”标签且未勾选

### Requirement: PUR-05 Purge uses plans and default recoverable mode
构建产物清理 SHALL 生成固定计划并按现有授权与核验执行，默认模式为回收站或同卷隔离；永久删除 SHALL 需要用户显式选择该模式。不在云同步占位上触发下载。

#### Scenario: A user purges three artifact directories
- **WHEN** 用户选择三个产物并确认
- **THEN** 创建固定计划，默认移入回收站，报告逐项结果与估计释放量
