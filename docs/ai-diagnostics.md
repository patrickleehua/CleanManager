# AI 诊断扩展：实现与契约

更新：2026-10-15。本文记录 OpenSpec 任务 15.1–15.8 的实现（15.4 的受约束 CLI 适配已实现，但 Win/Mac 模式证据未补齐，任务未勾选；其余已勾选）：契约、证据包、提供方与凭据、CLI 适配、结果校验、交接、桌面与 CLI 入口，以及在本机（Windows 11 x64）实际核验过的内容。产品设计见 [AI 清理诊断扩展设计](ai-cleanup-diagnostics.md)，行为要求见 [AID-01–13](../openspec/changes/define-cleanmanager-mvp/specs/ai-cleanup-diagnostics/spec.md)。15.9（真机 Mac 与全部适配模式的发布验收）尚未进行。

## 分层

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 契约与纯逻辑 | `crates/cm-core/src/diag.rs`、`diag_evidence.rs`、`diag_ground.rs`、`ai_prompt.rs` | 记录类型与预算、证据包与脱敏、提示模板与精确载荷、严格解析、依据核对、交接、生命周期 |
| 凭据 | `crates/cm-platform/src/credentials.rs`、`windows/credentials.rs`、`macos/credentials.rs` | Windows 凭据管理器读写删、CredUI 密钥输入框；macOS 钥匙串为编译通过的占位（返回 unsupported） |
| 服务与适配器 | `crates/cm-app/src/diag.rs`、`diag_http.rs`、`diag_cli.rs` | 提供方设置、预览→确认→执行、取消、保留/移除/导出、API 与 CLI 适配 |
| 入口 | `crates/cm-cli/src/diag_cmd.rs`、`apps/desktop/src-tauri/src/diag.rs`、`apps/desktop/src/views/diag*.ts` | 只读 CLI 与桌面工作台、上下文入口、设置 |

## 契约 diag/1（15.1）

诊断记录不进入首版 `schemas/v1`：它们使用独立的 [schemas/diag/v1/diagnosis.schema.json](../schemas/diag/v1/diagnosis.schema.json)（`$id` `urn:cleanmanager:diagnosis:1`），信封为 `{"contract":"diag/1","kind":…,"request_id":…}`；错误沿用 v1 的 `error` 记录与退出码。生成器 `tools/diag_contracts.py` 由 `tools/generate_contracts.py` 调用，写出 Schema 与 [合成样例](../examples/diagnosis/preview-video.json)；`tools/validate_docs.py` 校验样例并确认 24 个反例被拒绝，`cm-core` 的测试用 Rust 类型解析同一批样例并拒绝同样的变异。

| 记录 | 内容 |
| --- | --- |
| `diagnosis_preview` | 准备（`preparing`）：请求、脱敏证据包、目的地、实际载荷（HTTP 头/正文或 CLI 参数/stdin）、`payload_digest = SHA-256(c14n{destination, payload})`、字节数、是否含用户补充、10 分钟有效期 |
| `diagnosis_record` | `running → completed / cancelled / failed`，`completed → expired`；结果只在 completed/expired 存在，失败原因只在 failed 存在；新鲜度、交接、保留策略 |
| `diagnosis_list` | 摘要分页（每页最多 100） |
| `diagnosis_config` | 已配置提供方、凭据是否存在（从不含密钥）、支持状态 |

关键约束（Rust 与 JSON Schema/语义检查同时执行）：所有对象 `deny_unknown_fields`；版本字段固定（`diag/1`、`diag-evidence/1`、`diag-answer/1`、`diag-prompt/1`）；引用必须指向包内 `ev_N`/`gap_N`；`effective_recommendation` 只会比模型更保守，受保护对象至少为 `keep`，`reviewable` 需要可计划且证据完整的候选；过期或不新鲜的诊断不能交接计划；计划交接只指向被诊断的那个候选；会话保留必须有 `retain_until`。

字段预算：名称 120、位置 240、证据文本 240 字符；证据 24 条、缺口 12 条、子项 5 个；用户问题/陈述 280 字符、最多 8 条；载荷 48 KiB；提供方原始回答 256 KiB（HTTP 响应体 1 MiB）；摘要 280、段落 600、短句 200；声明 10 条（每条引用 6 个）、替代 5、追问 3（每个 4 个选项，60 字符）。

## 证据包与披露（15.2）

`cm_core::diag::evidence::build` 只读取索引元数据、候选集、保护层与只读应用清单：不打开文件、不读内容、不遍历索引以外的目录，云占位只记录其状态（`cloud_not_downloaded` 缺口）。目录只做计数汇总（最多 50 万个索引节点），子项摘要需单独勾选且最多 5 个直接子项。

默认脱敏：用户目录前缀换成 `%USERPROFILE%`/`~`，只保留最近三级上级目录，以账户名或其 8.3 短名命名的目录换成 `[user]`；名称中的邮箱、6 位以上数字、类似密钥的串被替换，控制字符与双向字符被移除；含疑似指令文本（如“ignore previous instructions”“上传”“批准”）的名称整体省略并记录缺口。完整路径是单独的披露选择。每个包都声明不发送的类别：文件内容、目录树、凭据、会话、云端下载。

采集内容只由对象 ID 与披露选项决定；测试用含提示注入的文件名验证采集项与普通名称相同、`.ssh`/`id_rsa` 等不会出现在载荷中。用户的问题与回答作为“用户陈述（未经核验）”证据；任何新增陈述、扩大范围或更换目的地都会改变载荷摘要，必须重新预览确认；确认时还要求提供方设置仍与预览一致。

## 提供方与凭据（15.3）

- **Anthropic Messages API**（默认，`https://api.anthropic.com`，模型 `claude-opus-5-5`，可改）：`POST /v1/messages`，头 `x-api-key` 与 `anthropic-version: 2023-06-01`，`max_tokens` 16000，`output_config.effort` 为 `medium`，不发送 thinking 参数（该模型默认自适应）。只读取 `text` 块；`stop_reason` 为 `refusal` 记为 `refused`，`max_tokens` 记为结构无效。**刻意不启用服务端 `fallbacks`**：AID-06 要求不静默改用其他模型。
- **OpenAI 兼容端点**（含本机）：`POST {base}/chat/completions`，可选 `Authorization: Bearer`。
- 端点只接受 `https://`，`http://` 仅限 `127.0.0.1`/`[::1]`/`localhost`；不允许内嵌凭据、查询或片段。
- HTTP 使用 `ureq` 3（`native-tls`：Windows SChannel / macOS Security.framework 与系统信任库），关闭重定向，不自动重试；一次运行只访问所配置的那个提供方。
- 连接探针：Anthropic `GET /v1/models/{model}`，兼容端点 `GET {base}/models`，不带请求体，也不含任何文件资料。
- 密钥只存系统凭据：目标名 `CleanManager:diag:<provider_id>`（Windows 凭据管理器，泛型凭据）。桌面通过宿主进程打开的 Windows 安全对话框（CredUI）输入，密钥不经过 WebView；设置文件 `<store>/diag-providers.json` 只有端点、模型与可执行文件位置。`Secret` 的 Debug 输出为 `<redacted>`，释放时清零；错误信息里的密钥与 `sk-…` 形式的字符串会被替换。移除提供方会同时删除其密钥。
- macOS：钥匙串适配为占位，返回 unsupported，桌面与 CLI 显示该接入方式不可用（15.9 需真机验证后实现）。

测试使用 std `TcpListener` 本地模拟服务，不访问真实服务、不读取真实凭据：成功、401（只请求一次且未触及另一个已配置的提供方）、超时、超长响应、拒答、截断、限流、无密钥的本机端点、更换目的地后确认失败、取消后迟到回答被丢弃。

## CLI 适配（15.4）

只接受绝对路径的 `.exe`（`.cmd`/`.bat`/`.ps1` 会经过 shell，拒绝），用固定参数向量启动、不经过 shell，证据从 stdin 传入；工作目录是每次新建的空临时目录，环境变量只保留登录所需项（`PATH` 只含 System32）；有时间上限、输出上限（超出即终止进程并丢弃）与取消（终止进程）。

- **Claude Code**：先运行 `--version` 与 `--help`（不发送数据），要求帮助中存在 `--print --output-format --tools --disallowedTools --strict-mcp-config --permission-mode --no-session-persistence --restricted` 且说明 `--tools ""` 可关闭全部工具；缺任何一项即 unsupported，不会启动。参数：`-p --output-format json --tools "" --disallowedTools <全部内置工具> --strict-mcp-config --permission-mode dontAsk --restricted --no-session-persistence [--safe-mode] [--disable-slash-commands] [--model M] --system-prompt <模板>`。执行前再次检查，参数与预览不一致则失败。
- **Codex**：`codex exec` 的 `--sandbox read-only` 仍允许模型运行读取任意文件的 shell 命令，且没有关闭该工具的文档化选项，因此始终 unsupported，不启动。

本机核验（2026-10-15，未发送任何数据）：

| CLI | 位置（npm 全局） | 结果 |
| --- | --- | --- |
| Claude Code 2.1.300 | `%APPDATA%\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe` | 必需参数齐全，可约束；可选 `--safe-mode`、`--disable-slash-commands` 也存在 |
| `claude.cmd` 包装脚本 | `%APPDATA%\npm\claude.cmd` | 拒绝（脚本会经过 shell） |
| codex-cli 0.162.1 | `…\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe` | unsupported（无法关闭 shell 工具） |

尚未在本机用真实账户实际跑一次 Claude Code 诊断（会发送数据并消耗额度），因此“这些参数组合在真实运行中不被拒绝”仍待 15.9 验收；运行器本身用测试二进制模拟 CLI 验证了参数原样传递（含 shell 元字符与空参数）、空工作目录、超时、输出上限、取消与退出码。

## 结果校验与交接（15.5–15.6）

提供方文本只接受一个 `diag-answer/1` JSON 对象（可容忍外面包一层 json 代码围栏）；散文、未知字段、浮点数（如 0.93 的“安全度”）、重复键、截断都记为 `invalid_structure` 且不采用任何内容。随后：

- 移除命令文本（`rm -`、`del /`、`Remove-Item`、代码块等）、“已批准/自动批准”说法和未加限定的“可安全删除”断言；超预算截断、超数量丢弃、无效引用删除，均记录在 `sanitized`。
- 每条声明按依据层核对：观察/规则声明须引用对应层证据；用户陈述须引用用户陈述证据并显示为“未核验”；未经检测的备份、未使用、可释放空间说法记为缺少依据；无规则时声称规则记为与事实冲突。
- 推荐只会更保守：受保护对象至少 `keep`；`reviewable` 需要核心候选可计划、保护层已核对且证据完整（无未读子范围、云占位、弱身份），否则降为 `needs_context`。界面从不显示“安全可删”。

真实场景夹具（`cm-core` 测试）：带规则的浏览器缓存（可交接一个候选，不含相邻缓存）、Downloads 中无规则的安装包（不变成候选）、个人视频与“已备份”的用户陈述、硬链接共享的模型权重（保护、可释放空间未知、应用管理入口）、受保护数据库（模型称可删也保持 keep）、部分扫描的构建目录（未知不当作 0）。

下一步由核心事实决定：`review_plan`（仅当候选可计划、未受保护、诊断新鲜；桌面在计划页创建只含该候选的新计划，仍需审阅与批准）、`keep`、`protect`（加入保护名单）、`locate`（磁盘视图定位，只读）、`verify`（核实清单）、`app_management`（应用页）、`archive`（始终不可用）。模型的替代建议只标注“模型也建议”。

## 生命周期与留存

确认后记录为 `running`；取消立即写为 `cancelled`，迟到的回答被丢弃；导航不取消。完成后 24 小时或同一范围的新扫描显示对象已变化/消失时为 `expired`：解释可读，交接关闭。记录存于 `<store>/diagnoses/<id>.json`，默认会话保留 24 小时，可保存或移除；移除不影响 `audit.db`。导出预览包含对象引用、脱敏证据、提供方/模型、解释与交接，不含凭据、文件内容或操作审计。

## 桌面（15.7）

- 入口：磁盘页详情、清理页候选详情、应用页详情（安装位置在本次扫描中时）的「AI 诊断」，以及顶栏的诊断记录按钮。没有规则的对象也可以诊断。
- 工作台：意图说明与可选问题 → 预览对话框（目的地、范围开关、对象资料、不会发送的内容、实际字节）→ 运行（可取消）→ 结论块（推荐记号用形状+文字）、用途与后果、“每句话的依据”（引用可跳到右侧证据）、追问（回答后重新预览）、下一步；失败/取消/过期/未配置各有状态与入口。
- 设置 › AI 诊断：添加 Anthropic、兼容端点或 CLI（原生文件对话框选择 `.exe`），保存/更换/移除密钥（系统对话框），测试连接，设为默认，移除。
- IPC 只传 scan/entry/preview/diagnosis id；新命令列在 `build.rs` 与 `capabilities/main.json`。与清理页的「AI 与开发者」类目在文案上明确区分。

## CLI（15.8）

```text
cleanmanager diag config
cleanmanager diag preview --scan SCAN --entry ENTRY [--provider ID] [--disclose full_path,child_summary] [--question TEXT]
cleanmanager diag create  --scan SCAN --entry ENTRY [...同上] --payload-digest DIGEST
cleanmanager diag show DIAGNOSIS_ID
cleanmanager diag list [--limit N] [--cursor C]
```

没有提供方时 `preview`/`create` 立即返回 `unsupported`（退出码 7）并说明在桌面配置，不等待输入；`create` 只发送与 `preview` 相同摘要的载荷，否则 `conflict`；完成为 0，取消 130，认证失败为 `permission_required`，其他提供方错误为 `io_error`。CLI 不配置密钥、不创建计划、不批准、不输出 shell 清理命令。GUI 与 CLI 读写同一 `<store>/diagnoses`，同一诊断 ID 在两侧可查询。

本机冒烟（本地模拟的兼容端点，合成目录）：未配置时 0.06 秒内失败；`preview` 后用错误摘要 `create` 被拒且没有发出请求；正确摘要的 `create` 完成，模拟端收到的正文与预览逐字节相同、无认证头；`show`/`list` 返回同一 ID。
