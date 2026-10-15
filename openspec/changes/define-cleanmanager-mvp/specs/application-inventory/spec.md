# Spec Delta

## Purpose

为人工管理提供来源可追溯的应用清单和关联目录，帮助解释哪些空间与哪个安装实例有关、哪些是共享或归属未知。Windows 与 macOS 可以采用各自的身份来源，但不能把未找到安装记录自动等同于可删除残留。

## ADDED Requirements

### Requirement: APP-01 Read-only identified installations
首版 SHALL 提供只读应用清单，记录安装实例、名称、版本、位置、来源和可获取的身份/签名证据。不同位置或来源的同名应用 SHALL 不合并为一个可操作对象；未知字段保持未知。

#### Scenario: Two installations share a display name
- **WHEN** 应用清单发现两个不同位置/来源的实例
- **THEN** 分别展示身份与证据，不按显示名称把它们及所有数据合并

### Requirement: APP-02 Proven association and shared storage
应用目录关联 SHALL 标明证据、共享关系、扫描时间及覆盖缺口。共享容器或不确定目录 SHALL 不重复作为每个应用独占容量，也不自动认定为该应用的清理目标。

#### Scenario: Two applications use the same shared container
- **WHEN** 显示应用关联空间
- **THEN** 清楚标明共享关系，汇总不把同一占用算成两个独立清理收益

### Requirement: APP-03 Missing installation is not proof of residue
系统 SHALL 区分未发现安装、来源不支持和已证实卸载信息。仅缺少安装记录或目录名像应用名称 SHALL 不形成低风险残留候选或授权。

#### Scenario: A data folder has no matching installation record
- **WHEN** 应用清单不能证明其归属与生命周期
- **THEN** 目录保持未知/只读，用户可查看证据，不自动勾选删除

### Requirement: APP-04 Explicit management capability boundaries
首版应用页 SHALL 明确其只读范围，并与磁盘详情、候选和系统定位入口关联。未适配卸载、登录项或外部包管理器动作 SHALL 显示能力限制，不用删除包目录冒充完整卸载。

#### Scenario: An application requires a vendor uninstaller
- **WHEN** 用户查看该应用详情
- **THEN** 系统说明现有能力和供应商处理方式，不生成假定完整的逐文件卸载计划
