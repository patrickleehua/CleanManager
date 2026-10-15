# Spec Delta

## ADDED Requirements

### Requirement: CLI-10 Command groups for every tool
CLI SHALL 为清理目录、构建产物、安装包、手选回收站、空间洞察、应用清单/卸载/更新、启动项、维护、状态、保护名单、Doctor、补全与自更新/自卸载提供命令组，均支持 JSON 输出与现有退出分类。所有写命令 SHALL 只创建或应用固定计划，缺授权时返回 needs_approval。

#### Scenario: An agent requests an uninstall
- **WHEN** 非交互调用卸载某应用
- **THEN** 返回包含本体与残留分组的计划 ID 与 needs_approval，不移动任何文件

#### Scenario: An agent lists protection entries
- **WHEN** 调用保护名单查询
- **THEN** 返回与桌面相同的条目、类型与来源

### Requirement: CLI-11 Dry-run is a plan preview
`--dry-run` SHALL 返回与真实执行相同筛选逻辑得到的对象清单、档位、大小与跳过原因，且不修改目标、不消耗授权；预览结果 SHALL 可直接转为固定计划供审阅。

#### Scenario: Dry-run and apply select the same set
- **WHEN** 对同一扫描先预览再创建计划
- **THEN** 计划对象等于预览中可选对象，状态变化的项在计划中标为 stale

### Requirement: CLI-12 Non-interactive runs never widen selection
非交互模式 SHALL 只包含 default_selected 对象，不自动包含 review_only、近期项目、个人数据或会话；扩大选择 SHALL 通过显式对象 ID 并仍需授权。不存在等同授权的 `--yes`。

#### Scenario: A scheduled script runs purge
- **WHEN** 非交互调用构建产物清理
- **THEN** 只提出非近期、可本地重建的产物计划，依赖目录与近期项目不在其中

### Requirement: CLI-13 Agent guidance for management tools
随附的 Agent 指引 SHALL 覆盖新增命令组，规定卸载、维护、启动项与特权设置仅在用户明确要求时发起，先展示预览与影响，再交由人工审阅或已配置策略；指引 SHALL 禁止以 shell 命令替代被拒绝的动作。

#### Scenario: A user asks the agent to free space
- **WHEN** 用户只要求释放空间
- **THEN** Agent 运行清理预览并汇报，不主动发起卸载、维护或启动项变更

### Requirement: CLI-14 Diagnostic trace on request
全局 `--debug` SHALL 把诊断轨迹（阶段与耗时、所选扫描/执行后端、跳过与降级原因、外部进程退出码）逐行写到标准错误；标准输出的机器记录 SHALL 与不带 `--debug` 时完全相同。轨迹 SHALL 不含密钥、AI 提供方凭据或授权回执内容；未指定 `--debug` 时不输出轨迹。`--debug` SHALL 不改变选择、授权或执行。

#### Scenario: A scan runs with --debug
- **WHEN** 调用方以 `--json --debug` 运行扫描
- **THEN** 标准输出仍是同一条 `scan_result` 记录，标准错误含阶段耗时与所选后端

#### Scenario: A debug trace never carries secrets
- **WHEN** 已配置 AI 诊断密钥时以 `--debug` 运行诊断命令
- **THEN** 标准错误只说明密钥是否存在，不含密钥本身
