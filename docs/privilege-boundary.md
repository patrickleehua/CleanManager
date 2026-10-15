# 提权边界与特权 helper 威胁模型

对应 `add-mole-parity-suite` 任务 13.1（Windows 短生命周期提权工作进程，已实现）、13.2（macOS 签名特权 helper，**规划中，未实现**）与 13.3（本文），以及 EXE-08、MNT-04、D10、D12。

## 原则

- 普通动作一律以当前用户权限执行；CLI 启动即禁用令牌中的 SeBackupPrivilege/SeRestorePrivilege。
- 提权只服务于目录中定义的固定动作：没有“执行命令”“运行脚本”“删除路径”之类的请求类型，模型生成的内容、文件名或用户输入都不会成为可执行的东西。
- 提权不改变计划范围与保护规则：提权进程只运行已审阅计划摘要里列出的管理员任务。
- 一次运行最多一次提权请求；拒绝即跳过，不重试、不降级为其他途径。

## Windows：短生命周期提权工作进程（13.1）

实现：`crates/cm-platform/src/windows/elevation.rs`（管道、启动器、工作进程循环）、`crates/cm-core/src/maintenance.rs`（`WorkerGrant`、`WorkerRequest`/`WorkerReply`、`WorkerSession`）、`crates/cm-app/src/maintenance.rs`（运行编排）、`crates/cm-cli/src/optimize_cmd.rs`（入口）。

### 生命周期

1. `plan apply` 核对计划摘要、种类规则与交互审阅回执后，在任何任务运行前收集需管理员的任务（已排除受保护与目录不一致的任务）。没有这样的任务或进程已提权时不启动工作进程。
2. 父进程（未提权 CLI）创建命名管道 `\\.\pipe\cleanmanager-maint-<父 PID>-<128 位随机数>`：单实例 + `FILE_FLAG_FIRST_PIPE_INSTANCE`（名称被抢占则失败）、`PIPE_REJECT_REMOTE_CLIENTS`、受保护 DACL `D:P(A;;GA;;;<当前用户 SID>)`（只有一个允许项，不继承）。
3. 通过 `ShellExecuteExW("runas")` 启动**同一个可执行文件**，隐藏第一参数 `__maintenance-worker`，参数固定为 `--pipe NAME --parent-pid PID --plan-digest DIGEST --tasks id@ver,... --idle-seconds N`。因此唯一一次 UAC 同意覆盖的正是这些任务；`ERROR_CANCELLED` 记为拒绝。
4. 父进程等待连接（60 s，工作进程提前退出即放弃）。`GetNamedPipeClientProcessId` 不是刚启动的那个进程的客户端一律断开并计数，继续等待自己的工作进程。
5. 工作进程校验参数（每个选项恰好一次、管道名必须是本机前缀加小写十六进制、摘要为 64 位小写十六进制、每个任务都在本构建目录中且版本一致且确实需要管理员），以 `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION` 打开管道（服务端只能识别、不能模拟这个高权限客户端），并要求 `GetNamedPipeServerProcessId` 等于参数中的父 PID，否则拒绝并退出。
6. 工作进程发送 hello（协议 `cm-maintenance-worker/1`、自身 PID、是否提权）。父进程要求协议一致、PID 等于启动的进程；生产启动器还要求 `elevated: true`。
7. 父进程对每个管理员任务发送一行 `{"op":"run","plan_digest":…,"task_id":…,"task_version":…}`，工作进程先探测再执行固定实现，回复该任务的结果；父进程只接受与请求同一任务、同一版本的结果。
8. 结束：父进程发送 `{"op":"done"}` 并等待最多 5 s。工作进程在 `done`、父进程断开（管道 EOF）、空闲超时（默认 120 s）或第一次拒绝时退出。父进程等待每个任务的时限为任务超时 + 30 s，超时记为 failed（`worker_error`）。

### 命令集

工作进程只理解两种请求，均为严格 JSON（拒绝未知字段），单行上限 4096 字节：

| 请求 | 接受条件 | 回复 |
| --- | --- | --- |
| `run` | `plan_digest` 等于启动参数中的摘要；任务 ID@版本在启动参数的授予列表中；该任务在本构建目录中且需要管理员；本会话尚未运行过该任务 | `result`（该任务的 `maintenance_task_run`） |
| `done` | 总是 | `bye`，随后退出 |

其他任何输入——命令字符串、脚本、带额外字段的请求、目录外任务、本计划之外的管理员任务、用户级任务、其他计划的摘要、重复运行——都回复 `refused`（`malformed_request`、`line_too_long`、`wrong_plan`、`task_not_granted`、`already_run`）并立即结束会话。当前管理员任务为 `windows.trim-when-due@1.0.0`（System32 `defrag.exe X: /L`）与 `windows.image-health-check@1.0.0`（DISM API CheckHealth，只读）。

### 校验方式（测试）

自动化测试从不弹出 UAC，也不运行真实任务：启动器可注入。

| 边界 | 测试 |
| --- | --- |
| 任意命令、多余字段、超长行、其他计划摘要、非授予任务、目录外任务、用户级任务、重复运行被拒 | `cm-core` `maintenance::tests::exe08_mnt04_worker_refuses_commands_foreign_tasks_and_other_plans`；经真实管道：`cm-platform` `elevation::tests::a_worker_refuses_arbitrary_commands_and_other_plans`、`a_worker_runs_only_granted_tasks_over_the_pipe` |
| 授予列表只能是目录中的管理员任务；参数多余/重复/远程管道名被拒 | `exe08_worker_grant_accepts_only_catalog_admin_tasks`、`pipe_names_and_arguments_are_closed_sets`；经真实二进制：`cm-cli` `optimize_flow::exe08_worker_entry_refuses_bad_starts` |
| 非本进程调用：父进程拒绝不是自己启动的客户端；工作进程拒绝不是父进程的服务端 | `the_parent_refuses_a_client_that_is_not_its_worker`、`a_worker_refuses_a_server_that_is_not_its_parent` |
| 管道 DACL 只含当前用户一个允许项，同名第二实例失败 | `the_pipe_dacl_grants_only_the_current_user` |
| 拒绝 UAC → 管理员任务 skipped，其余继续，仅请求一次 | `cm-app` `maintenance::tests::mnt04_declined_elevation_skips_admin_tasks_and_the_rest_continue`；`cm-cli` `optimize_flow::mnt04_declined_elevation_and_prt06_protection` |
| 跨进程端到端：一个普通子进程作为工作进程，经真实管道只运行计划内的管理员任务 | `optimize_flow::exe08_worker_child_runs_only_the_plans_admin_tasks`（`CM_MAINTENANCE_TEST_ELEVATION=direct`） |

`CM_MAINTENANCE_TEST_SCRIPT` 把全部任务替换为脚本结果；只有它存在时，`CM_MAINTENANCE_TEST_ELEVATION=declined|direct` 才会替换 UAC 启动器。二者都只会减少实际效果（脚本任务不触碰系统）。

### 威胁与缓解

| 威胁 | 缓解 |
| --- | --- |
| 同用户的恶意进程抢先创建同名管道，冒充父进程指挥工作进程 | 名称含 128 位随机数；父进程以 first-instance 创建，被抢占则失败；工作进程要求服务端 PID 等于父 PID |
| 恶意进程连接父进程的管道冒充工作进程，伪造结果 | 父进程只接受 PID 等于所启动进程的客户端，并核对 hello 中的 PID 与提权状态 |
| 远程连接 | `PIPE_REJECT_REMOTE_CLIENTS`；工作进程只接受 `\\.\pipe\` 本机前缀 |
| 服务端借高权限客户端身份执行操作 | 客户端以 identification 级别连接，服务端无法模拟 |
| 通过协议注入命令或超出计划的任务 | 请求类型封闭、严格反序列化、单行上限；任务须同时在启动参数授予列表、本构建目录与需管理员集合中；第一次拒绝即退出 |
| 工作进程长期驻留成为提权后门 | 无服务注册、无自启动；完成、断开、空闲超时或拒绝即退出 |
| 用户可写目录中的同名 DLL/可执行文件被提权加载 | DLL 以 `LOAD_LIBRARY_SEARCH_SYSTEM32` 加载；外部工具只用 `GetSystemDirectoryW` 得到的 System32 绝对路径与固定参数 |

### 已知限制

- 跨账户提权（标准用户输入另一个管理员账户凭据）时，工作进程的令牌用户与管道 DACL 中的当前用户不同，无法打开管道：管理员任务记为 skipped（`elevation_unavailable`）。
- 真实 UAC 路径（`UacLauncher`）尚无真机记录：自动化测试从不弹窗，本机实测时 Shell 已提权（任务在进程内执行，`already_elevated`）。
- 已在运行的外部工具（defrag/DISM）在父进程放弃等待后可能继续运行到自身结束；工作进程随即因管道断开退出。

### 移除

Windows 工作进程不安装任何东西：没有服务、计划任务、注册表项或文件。它与 `cleanmanager.exe` 是同一个可执行文件，删除程序即移除；运行中的工作进程会在父进程退出或空闲超时后自行结束。

## macOS：签名特权 helper（13.2，规划中）

**未实现。** 按 D12 的设计：以 Service Management 注册的签名守护进程，只接受固定命令分组（维护任务分组、风扇模式、电池充电上限、网络低延迟）；校验调用者的代码签名与版本、硬件型号在已验证列表；异常温度、helper 卸载或产品退出时恢复系统自动控制；卸载流程注销 helper 并移除其 plist 与二进制。各 macOS 版本的注册细节、可验证硬件型号与真机测试记录在实现 13.2 时补充到本节；在此之前 macOS 上没有任何提权路径。
