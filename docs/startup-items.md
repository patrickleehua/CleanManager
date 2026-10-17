# 启动项（Windows / macOS）

状态：变更 `add-mole-parity-suite` 任务 11.1 的 Windows 只读清单已实现，macOS 的 launchd 作业只读清单也已实现（只在托管 runner 上测试，未在真机验证；Service Management 登录/后台项没有公开的列举接口，见下文“macOS”，所以任务未勾选），11.2–11.3 的 Windows 部分已实现。所有部分都未通过 G3 门禁（任务 18.3），也没有发布（核心 `cm_core::startup`、执行 `cm_app::plans`、平台 `windows/startup.rs` 只读清单与 `windows/startup_actions.rs` 写入）；macOS 只有只读清单 `macos/startup.rs`，不提供启停或删除。命令形状以 [CLI 协议](cli-protocol.md) 为准。

默认关闭（设计 D15）：启动项尚未通过门禁 G3，所以它的 CLI 命令组、对应种类计划的审阅与执行、桌面页面和宿主命令都拒绝使用并说明原因。只为在这台电脑上评估时，用 `cleanmanager experimental --state on` 或桌面“设置 › 实验功能”开启（两者共用一个设置，见 [CLI 协议](cli-protocol.md) 的“实验功能”）；开启后能力仍报告为 `unverified`，不算已发布。

## 哪些条目可以启停（STA-02/04）

| 来源 | 机制 | `toggle_capability` |
| --- | --- | --- |
| `Run` 值（HKCU；HKLM 64/32 位视图） | 只写资源管理器的启用记录 `…\Explorer\StartupApproved\Run`（32 位视图为 `Run32`），与任务管理器相同：`02…` 启用，`03…`+时间停用；`Run` 值本身不变 | 用户级 `available`；系统级非管理员 `permission_required` |
| 启动文件夹文件（用户/公共） | 启用记录 `…\StartupApproved\StartupFolder`，文件本身不变 | 同上 |
| `RunOnce`、策略 `Run` | 只读（安装步骤或由策略管理） | `unsupported` |
| 计划任务、商店应用启动任务 | 没有已验证的启停机制：只给出“在设置 > 应用 > 启动（`ms-settings:startupapps`）或任务计划程序中更改”的说明，不提供开关 | `unverified` |
| 系统组件、终端安全/设备管理代理、本工具自身 | 只读并说明原因 | `unsupported`（`protected: true`） |

系统级条目需要在**管理员终端**里创建和执行计划；CleanManager 从不自行提权，非管理员时返回 `permission_required`（退出 4）。

## 失效项（STA-03）

目标在**已挂载卷**上不存在才是 `broken`；所在卷未挂载为 `target_unreachable`（未知，不是失效），网络路径与无法检查的目标保持原状态。只有 `Run` 值的失效项可以删除（`startup remove-stale`）：执行前再次确认目标仍缺失，先把值（类型与原始数据）和它的启用记录作为备份写入审计存储，**备份写入失败就不删除**。

## 用法与恢复

```text
cleanmanager startup list --json                          # startup_list：来源、目标、发布者、范围、状态与可否启停
cleanmanager startup disable st_0123456789abcdef --json   # plan（startup），需审阅；enable 同理
cleanmanager plan review sp_demo                          # 终端内审阅
cleanmanager plan apply sp_demo --json                    # 先记录原状态，再写启用记录并回读核对
cleanmanager startup remove-stale st_fedcba9876543210     # 失效 Run 值：备份后删除的计划
cleanmanager startup restore --operation op_demo --json   # plan（recovery）：恢复该操作记录的原状态
```

- 每次启停或删除前，原状态（原启用记录、原值）随操作写入审计存储；`operations list` 中这类操作的恢复方式为 `in_app_restore`。
- `startup restore` 生成独立的 `recovery` 计划，同样需要审阅：启用记录只在仍是本操作写入的值时才改回（之后被别人改过则跳过 `changed_since`）；被删除的值只在同名值不存在时才重建（已被重新创建则跳过 `destination_exists`，绝不覆盖）。
- 每个计划都绑定条目位置与执行时观测到的定义和启用记录的摘要；执行前现场状态与计划不一致则跳过（`changed_since_plan`）。
- 测试通过 `CM_STARTUP_TEST_ROOT`（必须是 `HKCU\Software\CleanManagerTest\` 下的键）把注册表重定向到一次性键，真实启动项不会被测试修改。

## macOS（只读清单）

`cleanmanager startup list --json` 在 macOS 上列出 launchd 作业，每个 plist 一行（以 plist 路径而非标签识别）：

| 位置 | `source` / `scope` | 状态来源 |
| --- | --- | --- |
| `~/Library/LaunchAgents` | `launchd_user` / `user` | `gui/<uid>` 域 |
| `/Library/LaunchAgents` | `launchd_system` / `system` | `gui/<uid>` 域（为每个用户加载） |
| `/Library/LaunchDaemons` | `launchd_system` / `system` | `system` 域 |
| `/System/Library/LaunchAgents`、`/System/Library/LaunchDaemons` | `launchd_system` / `system`，`protected: true` | 同上；macOS 组件，只读 |

- 读取 `Label`、`Program`、`ProgramArguments`、`RunAtLoad`、`KeepAlive`、`Disabled`（XML、二进制与 OpenStep 格式）。无法解析、没有 `Label` 或没有程序的 plist 仍是一行，状态 `unknown` 并说明原因。
- 目标（`Program`，否则 `ProgramArguments[0]`；裸名称按 launchd 默认 `PATH` 查找）只做 stat：已挂载卷上不存在为 `broken`；位于 `/Volumes/<名称>` 且该卷未挂载为 `target_unreachable`（未知，不是失效）。
- 发布者来自目标的代码签名所声称的信息（Apple 自身代码为 `Apple`，否则为叶证书名称与 Team ID；只做基本校验，不是信任结论），`publisher_evidence: code_signature`。Apple 签名的目标、安全/设备管理代理与本工具自身的条目同样受保护。
- 状态：`launchctl print-disabled gui/<uid>` / `system` 的覆盖记录优先于 plist 的 `Disabled` 键；未停用且在 `launchctl print gui/<uid>` / `system` 中已加载为 `enabled`，未停用但未加载为 `unknown`（可能已在“登录项与扩展”中关闭、只在其他会话类型加载或被卸载）；查询失败也为 `unknown` 并说明原因。`launchctl` 只经受信工具运行器执行这两个只读子命令；不读取后台任务管理（BTM）数据库，不使用 `sfltool`。
- Service Management 登录项与后台项没有公开的列举接口：清单只给出一行 `sm_background_item`、状态 `unknown`，指向系统设置“通用 › 登录项与扩展”（`x-apple.systempreferences:com.apple.LoginItems-Settings.extension`）。
- 本构建在 macOS 上不启停、不删除任何条目：每一行 `toggle_capability: unsupported` 并说明原因；`startup disable|enable|remove-stale` 对受保护条目返回 `policy_denied`，其余返回 `unsupported`，不生成计划。能力行 `startup.toggle` 为 `unsupported`，`startup.inventory` 为 `unverified`。
- 测试：`CM_STARTUP_TEST_ROOT`（绝对路径的目录）作为假 `/`，读取其下的 `Library/LaunchAgents`、`Library/LaunchDaemons` 与 `System/Library/Launch*`；用户目录为 `CM_STARTUP_TEST_FOLDER`（默认 `<根>/home/Library/LaunchAgents`）。重定向时从不运行 `launchctl`，状态取自 plist 的 `Disabled` 键并在原因中说明。
