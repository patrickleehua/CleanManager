# Spec Delta

## Purpose

让桌面和 CLI 按需浏览同一份扫描索引，定位大目录和大文件，并在大量条目、刷新和部分扫描下保持查询可解释。分页、排序和空间汇总必须绑定数据版本，避免把不同扫描时刻的结果混成一个列表或收益数字。

## ADDED Requirements

### Requirement: IDX-01 Revision-bound queries
系统 SHALL 为可查询结果提供 scan_id 和 revision，并将游标绑定扫描版本、范围、排序及过滤条件。游标失效或条件不同 SHALL 返回 cursor_stale 或参数错误，不混入其他版本数据。

#### Scenario: A client reuses a cursor after replacing the revision
- **WHEN** 原游标绑定的 revision 已不可用
- **THEN** 查询返回 cursor_stale，客户端可重新请求第一页而不误认旧位置

### Requirement: IDX-02 Bounded pagination and complete selection
系统 SHALL 提供目录、文件和候选的有界分页，默认每页 50 项，允许 1–500 项；超出范围 SHALL 报错。列表 SHALL 提供稳定条目 ID 与后续游标，用户可以访问完整结果；显示前几项不等于批准全部结果。

#### Scenario: A directory contains more entries than one page
- **WHEN** 客户端遍历相同 revision 的所有页面
- **THEN** 每个符合条件的条目恰好出现一次，末页明确无后续游标

#### Scenario: The requested page exceeds the limit
- **WHEN** 客户端请求 501 项
- **THEN** 返回参数错误，不导出不受限的整盘列表

### Requirement: IDX-03 Stable filtering and ordering
系统 SHALL 在完整查询范围内执行大小排序、目录/文件类型过滤和名称过滤，提供确定的相同大小排序次序。分页 SHALL 不先截断数据再进行客户端局部排序。

#### Scenario: The largest file is discovered outside the first stored batch
- **WHEN** 用户按已知逻辑大小降序查询同一已发布 revision
- **THEN** 最大文件按全范围排序出现在前部，未知大小被单独标示

### Requirement: IDX-04 Hierarchy and aggregate provenance
系统 SHALL 区分目录项和实体身份，呈现层级、逻辑/分配小计、统计依据和覆盖缺口。父子条目或共享实体 SHALL 不被重复加总为所选收益；未知范围 SHALL 继续随汇总显示。

#### Scenario: A parent and child are both included in a summary request
- **WHEN** 选择集合包含同一目录及其子项
- **THEN** 汇总消除包含重叠或要求细分，不把相同字节累计两次

### Requirement: IDX-05 Local persistence and safe index rebuild
系统 SHALL 默认将索引保留本机，并在正常重启后查询已发布结果及其时间。可再生索引损坏时 SHALL 报告并允许重建；重建 SHALL 不删除操作日志、待核对记录或恢复清单。

#### Scenario: The scan index is damaged while recovery history exists
- **WHEN** 用户启动应用且扫描索引无法读取
- **THEN** 系统说明索引需要重建，仍保留操作/恢复记录，不把恢复数据一并初始化清空
