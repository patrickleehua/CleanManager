# Spec Delta

## Purpose

让用户查明操作处理了什么、依据是什么、是否可恢复，以及空间数字实际代表什么。隔离、平台回收站和永久删除有不同恢复边界，恢复和清空必须分别处理冲突与授权，避免为了展示清理收益而丢失内容或历史。

## ADDED Requirements

### Requirement: REC-01 Recoverable quarantine manifests
同卷隔离成功 SHALL 具有可验证的隔离对象、原目录/名称与身份、隔离位置和恢复条件。恢复信息无法可靠保存时 SHALL 不报告可恢复成功；隔离内容不进入普通清理候选。

#### Scenario: An isolated item is queried after restart
- **WHEN** 原卷仍可用且隔离对象未变化
- **THEN** 用户可查到关联计划、原位置、当前隔离位置及支持的恢复动作

### Requirement: REC-02 Restore without implicit overwrite
恢复 SHALL 是范围确定的独立授权动作，重新核验源/目标身份和当前能力。原位置存在新对象时 SHALL 默认拒绝覆盖；卷离线或身份不符时保留清单，不自动恢复到同路径的新卷。

#### Scenario: A replacement file exists at the original name
- **WHEN** 用户恢复隔离项
- **THEN** 系统报告目标冲突，现有文件与隔离文件均保留；另选位置需重新计划

### Requirement: REC-03 Separate purge authorization and retention
永久清空隔离项 SHALL 使用独立固定计划和有效授权，不继承原隔离批准。保留期到期 SHALL 默认提醒而不删除；未知、待核对或冲突内容不得静默清空。

#### Scenario: A quarantined item reaches its retention period
- **WHEN** 默认保留期提醒被触发
- **THEN** 系统提供审阅入口并保持内容存在，不因时间到期自动永久删除

### Requirement: REC-04 History and truthful recovery availability
历史 SHALL 关联计划、授权来源、规则版本、时间、逐项结果及恢复位置/限制。系统 SHALL 按实际平台能力提供产品内恢复或系统恢复指引；永久删除不得显示通用撤销，索引重建不得删除未核对历史。

#### Scenario: A platform trash operation only supports system-managed restoration
- **WHEN** 用户查看该条目历史
- **THEN** 显示系统恢复指引，不提供无法履行的产品内恢复按钮

### Requirement: REC-05 Separate processed amount and observed free space
报告 SHALL 分开显示处理对象/逻辑量、预计释放、实际卷空闲前后观测及有符号差值。同卷隔离或 Trash 移动 SHALL 不按移动大小报告释放；观测变化不得全部归因于本工具。

#### Scenario: One hundred MiB moves into same-volume quarantine
- **WHEN** 隔离动作成功而该卷空闲量没有增加
- **THEN** 报告处理量与已隔离量，空间观测差值为零，不宣称释放 100 MiB

#### Scenario: Background writes reduce free space during an operation
- **WHEN** 空闲量的后观测小于前观测
- **THEN** 报告负的观测差值和归因限制，不修饰为零或虚构清理收益
