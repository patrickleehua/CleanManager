# 启动项（Windows）

状态：变更 `add-mole-parity-suite` 任务 11.1 的 Windows 只读清单已实现（11.1 要求两个平台，macOS 登录/后台项与 launchd 作业未实现，所以任务未勾选），11.2–11.3 的 Windows 部分已实现。所有部分都未通过 G3 门禁（任务 18.3），也没有发布（核心 `cm_core::startup`、执行 `cm_app::plans`、平台 `windows/startup.rs` 只读清单与 `windows/startup_actions.rs` 写入）；macOS 未实现。命令形状以 [CLI 协议](cli-protocol.md) 为准。

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
