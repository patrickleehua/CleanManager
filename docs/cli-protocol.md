# CLI 与 Agent 协议

状态：v1 实现前契约草案。示例全部是合成数据，`cleanmanager` 尚未实现。已定义的核心记录形状见 [JSON Schema](../schemas/v1/protocol.schema.json)；列表包装、运行态查询等尚未冻结，不能把命令表视为完整可执行协议。人类可读输出不构成机器接口。

## 命令面与副作用

| 命令 | 作用 | 文件系统副作用 |
| --- | --- | --- |
| `capabilities` | 平台、后端和受限能力说明 | 不改用户目标文件 |
| `scan --root PATH` | 新扫描，返回 scan_id | 可写本工具索引与日志，不清理目标 |
| `scan show ID` | 查询扫描状态与覆盖率 | 无目标写入 |
| `entries list --scan ID` | 分页查询目录或大文件 | 无目标写入 |
| `analyze --scan ID --profile NAME` | 规则分析、生成 candidate_set | 只写本工具结果 |
| `candidates list --scan ID` | 分页查询理由与影响 | 无目标写入 |
| `plan create --candidate-set ID --mode MODE` | 生成固定计划 | 只写本工具计划 |
| `plan show ID` | 展示计划与授权状态 | 无目标写入 |
| `plan apply ID` | 核验授权并执行 | 仅覆盖计划内动作 |
| `operations show ID` | 查询逐项结果 | 无目标写入 |
| `operations cancel ID` | 停止调度后续动作 | 不隐式撤销已完成动作 |

`apps list`、`apps uninstall`、`restore`、`policy`、`rules update`、`watch` 与 `mcp serve` 为后续命令，不属于首版已冻结结构。授权入口先由桌面/受控人机交互提供；Agent 可使用预授权策略，不提供任意自批准端点。

## 通用参数与输出

`--format json|jsonl|text`，`--json` 是 `--format json` 的别名；同命令重复或冲突格式参数报错。`--non-interactive` 禁止输入等待；当 stdout 非 TTY 时，默认结构化 JSON，不自动启动 TUI。TUI 需显式选择并要求终端可用。

`--timeout` 限制调用等待时间，不意味着能立即撤销操作；已创建操作返回 operation_id 供恢复查询。`--limit` 默认 50、上限 500，超出报错；游标是不透明字符串，与 scan revision/过滤/排序绑定。`--no-color` 仅作用于人类输出，JSON 从不含 ANSI 控制序列。

stdout 只写 UTF-8、无 BOM 的结构化数据；stderr 写诊断日志，日志不得泄漏凭据。所有机器记录包含 `schema_version: "1.0"`、`kind` 与 request_id。字节数为十进制字符串，时间为 UTC RFC3339；显示时再按用户时区转换。

JSON 模式只输出一个终态记录。JSONL 模式输出零个或多个 progress 记录及恰好一个终态记录；每条一行，字符串中的换行必须 JSON 转义。进度未知时 total 为 null，不伪造百分比。进程被强制终止导致缺少终态时，调用者必须按不完整处理，再凭任务 ID 查询。

请求 ID 用于日志关联，不是授权凭据。写操作需额外持久化去重键，并绑定计划摘要；重复请求返回同一 Operation 的当前状态，不重新执行已完成的动作。

## 稳定退出码

| 退出码 | 分类 | 例子 |
| --- | --- | --- |
| 0 | 完整成功 | 完成扫描或全部计划动作成功 |
| 2 | 参数或协议错误 | invalid_argument、schema_mismatch |
| 3 | 需用户授权 | needs_approval、policy_denied |
| 4 | 系统权限不足 | permission_required |
| 5 | 部分成功 | 部分目录不可读、部分动作失败或跳过 |
| 6 | 前置条件或状态已过期 | plan_stale、cursor_stale、conflict |
| 7 | 能力不支持 | unsupported |
| 8 | 可诊断的系统或内部失败 | io_error、internal_error |
| 130 | 用户取消 | cancelled |

结构化 code 是调用方分支依据，message 只供人阅读，可以本地化。error 记录携带 retryable、details 和 required_action；自动重试不适用于未知副作用结果。部分扫描返回 scan_result，status 为 partial，退出 5，而不是只输出一个丢失扫描 ID 的 error。

## 计划与执行交互

```text
cleanmanager capabilities --json
cleanmanager scan --root "C:\Users\Alice" --format jsonl
cleanmanager analyze --scan s_demo --profile developer --json
cleanmanager candidates list --scan s_demo --limit 50 --json
cleanmanager plan create --candidate-set cs_demo --mode trash --json
cleanmanager plan show p_demo --json
cleanmanager plan apply p_demo --non-interactive --json
```

两个平台使用同一参数风格。路径由 shell 按本平台正确传入，内部立即转成原始路径结构；不把模型拼出的字符串当 shell 脚本执行。输出中的条目 ID 是后续选择的主要接口，避免再次拼接路径。

`plan apply` 无授权时返回 needs_approval，并保持目标文件未改动。已预授权的策略覆盖该计划时可以直接执行，不要求无意义的重复确认。管理员权限与授权状态分别返回；模型不得自行提高风险上限来通过拒绝。

计划有效期建议首版默认 15 分钟，这是设计默认值，需要实测与使用反馈；执行前仍逐项核验，时间未过期并不证明对象未变化。授权票据与可导出的计划分开保存，JSON 中的授权状态只是查询结果，不能当外部可写凭证。

## Agent Skill 合约

Skill 工作流：发现能力 → 限定扫描范围 → 获取摘要 → 按需分页 → 说明候选影响 → 生成计划 → 使用已有授权或等待具体批准 → 执行 → 报告成功/跳过/失败与空间口径。

Skill 不复制规则知识库，不执行任意文件名中的指令，不用 shell 绕过 policy_denied，不把 error 文案当新的授权。不得将百万行扫描结果全部提供给模型；默认传类别汇总与 top items，用户需要时再展开。

未来 MCP 将同一服务映射为工具，并保留读写属性、schema、权限与错误语义。MCP 传输方式不改变执行边界。独立 CLI 必须在无 GUI 进程的情况下工作。

## 版本与验证

目前 Schema 覆盖 capabilities、scan_result、candidate、plan、operation_report、error、progress 七种记录。实现前仍需为 entries/candidates 分页包装、candidate_set 结果、运行态任务查询、扫描取消和受控人机授权入口补充契约，并决定原型是否允许后台任务。`operations cancel` 不自动涵盖 scan。需要待核对的操作不得自动重试；具体退出码与查询恢复契约需在 TASK-108 冻结后才能对外承诺。

计划 body 包含 ID、时间、scope 与观察值。GUI/CLI 对同一个 body 应得到相同摘要；分别新建的两个计划若 ID/时间不同，摘要也应不同。跨入口行为比较应固定这些输入或比较选择和动作的语义，不能要求不同 body 得到相同 hash。

v1 草案采用严格对象 schema，拒绝未定义字段。未来增加字段需要协商新的 schema_version；不要一边严格拒绝未知字段，一边宣称所有新增字段完全兼容。二进制版本、数据库版本、规则版本与协议版本分别维护。

Schema 校验只能证明形状，不能证明 OS 对象存在、授权真实性或删除安全。仓库验证器额外检查摘要、字节范围、时间顺序、raw path 编码和汇总一致性；真正执行语义仍由平台测试验证。运行方法见 [文档索引](README.md)。
