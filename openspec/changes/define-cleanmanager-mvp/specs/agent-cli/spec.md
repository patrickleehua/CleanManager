# Spec Delta

## Purpose

让脚本、Claude/Codex 和人工终端用户能够独立调用 CleanManager，共享桌面的扫描事实、候选、计划与执行结果。机器接口必须有稳定格式、状态、分页和授权边界，并配套短小工作流说明，避免模型解析界面文字或自行拼接清理脚本。

## ADDED Requirements

### Requirement: CLI-01 Independent cross-platform entry point
CLI SHALL 在 Windows/macOS 上独立安装和运行，不依赖桌面包、窗口或 GUI 进程。它 SHALL 使用与桌面相同的规则、计划、授权及执行结果语义。

#### Scenario: An agent runs on a machine with only the CLI installed
- **WHEN** Agent 查询能力、扫描、生成计划并使用已有有效授权执行
- **THEN** 命令可完成或返回明确能力限制，无需启动或安装桌面

### Requirement: CLI-02 Complete commands and queryable identifiers
首版 CLI SHALL 覆盖能力、扫描/查询/取消、目录和候选分页、分析、计划创建/查询/应用、操作查询/取消及结果；每种命令须有完整的版本化响应。创建的任务/计划/操作 SHALL 提供可后续查询的 ID。

#### Scenario: A scan is still running in its originating process
- **WHEN** 另一 CLI 调用查询该 scan_id 或请求取消
- **THEN** 返回明确运行状态或取消接收状态，不把 running 混入只允许终态的记录

#### Scenario: A plan application has an uncertain outcome
- **WHEN** 查询 Operation 发现未确认的副作用
- **THEN** 返回 reconciliation_required 及逐项 unknown，不引导调用者盲目重复执行

### Requirement: CLI-03 Machine-clean JSON and JSONL
CLI SHALL 支持 JSON、JSONL 与人类文字格式。机器 stdout SHALL 是 UTF-8 无 BOM 且不含日志/ANSI；诊断写 stderr。JSON 输出一个终态；JSONL 输出有序进度及一个终态，未知总量为 null。

#### Scenario: A non-interactive JSONL scan discovers an unknown total
- **WHEN** 扫描正在枚举且尚无确定分母
- **THEN** 进度不伪造百分比，最终只有一条终态记录，日志不会污染 stdout

#### Scenario: A process is forcibly terminated
- **WHEN** 进程未能输出终态
- **THEN** 工作流将流视为不完整，并凭 ID 查询/核对，不以最后进度推断成功

### Requirement: CLI-04 Exact data and protocol versions
机器记录 SHALL 包含 schema_version、kind 和 request_id；字节数为 u64 十进制字符串，空闲差值可带负号，时间为 UTC。路径 SHALL 保存平台原始编码与显示值；未知大小/估计为 null，执行不重新解析显示路径。

#### Scenario: A byte count exceeds JavaScript safe integer precision
- **WHEN** 返回该计数
- **THEN** JSON 中保持精确十进制字符串，GUI/Agent 不因 number 转换丢失低位

#### Scenario: A client submits an unsupported schema or field
- **WHEN** 请求不符合协商的严格协议
- **THEN** 返回 schema_mismatch/参数错误，不忽略未知字段来接受不明确的动作

### Requirement: CLI-05 Stable exit classifications
CLI SHALL 使用 0 完整成功、2 参数/协议错误、3 授权缺失/策略拒绝、4 权限不足、5 部分结果、6 过期/冲突、7 不支持、8 系统/内部失败或需核对、130 用户取消。调用者 SHALL 通过类型化 code/status 判断，不能依赖本地化 message。

#### Scenario: A subtree is unreadable but confirmed results exist
- **WHEN** 扫描以 partial 结束
- **THEN** 返回含 scan_id 的结果及退出 5，不丢失已知结果

#### Scenario: An operation requires reconciliation
- **WHEN** 副作用无法确认
- **THEN** 返回对应 operation_report、退出 8，并明确禁止自动重试

### Requirement: CLI-06 Bounded output and non-interactive behavior
CLI SHALL 提供不透明游标、有界分页和摘要查询；默认 50、最大 500 项。stdout 非 TTY 时默认机器输出；non-interactive SHALL 禁止输入等待，不自动启动 TUI，也不意味着授权。

#### Scenario: An agent applies a plan without authorization
- **WHEN** 使用 non-interactive 调用
- **THEN** 立即返回 needs_approval、退出 3 和可审阅的 plan_id，不等待输入或自动批准

### Requirement: CLI-07 Trusted human review without a machine self-approval endpoint
CLI SHALL 提供人工审阅存储计划的受控交互入口，使无 GUI 用户可明确批准；机器入口不接受任意批准旗标或模型文字作为凭据。TTY 与同用户进程权限 SHALL 不被宣传为证明真人或隔离恶意 Agent 的机制。

#### Scenario: A person reviews a previously generated plan in the terminal
- **WHEN** 用户查看完整模式/范围/影响并明确确认
- **THEN** 授权绑定该存储计划的摘要，后续机器 apply 使用核心核验的授权状态

### Requirement: CLI-08 Bundled agent workflow guidance
首版 SHALL 提供可用于 Claude/Codex 的精简 Skill/CLI 指引，覆盖能力发现、范围选择、摘要/分页、解释、计划、授权处理、执行和结果报告。规则知识与保护 SHALL 在核心强制执行；Skill 不提供绕过拒绝或自批流程。

#### Scenario: A skill receives policy_denied
- **WHEN** Agent 按工作流调用应用计划
- **THEN** 指引要求解释具体拒绝和已有审阅入口，不改用 shell 删除目标或提高策略限额

### Requirement: CLI-09 Data minimization and task lifetime
机器工作流 SHALL 默认使用必要摘要和按需分页，不默认上传整棵目录树。任务查询 SHALL 明确发起进程退出/中断后的状态；首版不承诺没有任务宿主时继续后台执行，调用超时不代表回滚。

#### Scenario: The process hosting a scan exits
- **WHEN** 后续调用发现没有活动宿主且扫描未完成
- **THEN** 系统将任务标为中断/不完整并保留结果，不永久显示仍在正常运行
