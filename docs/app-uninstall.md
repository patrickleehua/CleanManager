# 应用卸载

状态：变更 `add-mole-parity-suite` 任务 9.2–9.8 已勾选，9.1（来源适配）的 Windows 部分已实现，但因 macOS 来源未实现而未勾选。没有在测试中移除过真实的 MSIX 包，也未通过 G3 门禁（任务 18.3），没有发布。代码位置：核心 `cm_core::uninstall`、执行与计划 `cm_app::uninstall`、平台 `windows/uninstall.rs`、命令 `apps uninstall …`。macOS 未实现，所有 `apps uninstall` 命令在 macOS 返回 `unsupported`（不伪造废纸篓或 Homebrew 适配）。命令形状以 [CLI 协议](cli-protocol.md) 为准。

## 两段计划（D7）

1. **第一段** `apps uninstall plan create --instance ID`：`uninstall` 计划只含 `app_quit`（退出请求）和一个 `tool_adapter`（按来源选择的卸载方式）。审阅时附带 `leftover_estimate`（按组计数的**估计**，不是计划对象，也不被授权）。
2. 执行后核心**重新扫描**安装目录与卸载注册项，而不是根据退出码推断；仍有剩余时结果为 `partial`（动作代码 `leftovers_remain`，消息含退出码与耗时）。
3. **第二段** `apps uninstall leftovers --operation OP` 给出分组预览；`apps uninstall leftovers plan create --operation OP [--items ID,…]` 生成另一个 `uninstall` 计划，单独审阅、单独执行。

两段都需要 `plan review`（终端人工审阅）后才能 `plan apply`；策略永远不能匹配卸载计划，非交互调用者无法批准。

## 各平台支持的来源（UNI-01）

| 平台 | 来源 | 卸载方式 | 状态 |
| --- | --- | --- | --- |
| Windows | 注册卸载项（`UninstallString`） | 只取该实例注册项中的命令，程序必须是绝对路径的本地 `.exe`（脚本宿主、批处理、裸文件名一律拒绝）；保留其自身界面，不加静默参数（`QuietUninstallString` 只读不用） | 已实现 |
| Windows | MSI（`WindowsInstaller=1` 且键名为 ProductCode，或注册命令是 `msiexec`） | 系统目录的 `msiexec.exe /x {ProductCode}`（修改命令 `/I` 一律改为 `/x`；注册的 msiexec 路径不使用） | 已实现 |
| Windows | MSIX / 商店（当前用户已注册的包） | Windows 包管理器 `PackageManager.RemovePackageAsync`（不经 PowerShell）；计划绑定包清单 `AppxManifest.xml` 的哈希；仅移除当前用户已注册的包全名 | 已实现（真实包未在测试中移除） |
| Windows | winget（注册项带 `WinGetPackageIdentifier`） | App Installer 包目录中的 `winget.exe uninstall --id ID --exact [--source S]`（不用 `%LOCALAPPDATA%` 下的执行别名链接）；找不到 winget 时为 `unsupported`，不回退为删文件 | 已实现 |
| Windows | Scoop（`%SCOOP%\apps\<name>` 或 `~\scoop\apps\<name>`） | Windows PowerShell 执行 Scoop 自己的 `scoop.ps1 uninstall <name>`（固定参数） | 已实现 |
| Windows | npm 全局 AI CLI（Claude Code、Codex CLI、Gemini CLI） | Node.js 安装目录中的 `node.exe npm-cli.js uninstall --global <package>` | 已实现 |
| Windows | 其他/无法验证（无卸载命令、命令不可读或是脚本） | 不提供卸载：返回 `unsupported`，`required_action` 指向“设置 > 应用 > 已安装的应用”（`ms-settings:appsfeatures`）或供应商卸载程序；**从不把删除安装目录当作卸载** | 能力说明 |
| macOS | 应用包、Homebrew cask/formula、App Store | 未实现 | `unsupported` |

## 执行前核验（UNI-02/03/10）

- **保护类（UNI-10）**：系统组件、Windows 包、共享运行时、终端安全/设备管理代理以及 CleanManager 本身没有卸载入口（`policy_denied`）；安全/管理代理只指向管理员或厂商的官方卸载流程。
- **签名与发布者**：建计划时与执行前各核验一次：程序必须是普通本地文件（非链接、非网络路径），记录 SHA-256；Authenticode 签名者（内嵌签名或 Windows 编录签名）必须与注册项的 `Publisher` 一致（忽略 Inc./Ltd./Corporation 等后缀，按词前缀比较），无签名或不一致为 `policy_denied`；`msiexec` 与 Windows PowerShell 必须由 Microsoft 签名。执行前注册项摘要、程序哈希或签名者有任何变化都跳过（`record_changed`、`program_changed`）。
- **运行中的应用（UNI-02）**：映像位于安装目录下的进程（npm 工具用 Restart Manager 查询占用其包文件的进程）先收到 `WM_CLOSE`，有限等待（默认 15 秒）。仍在运行时第一段计划失败为 `still_running` 并列出进程，**卸载程序不运行、不删除任何东西**；运行状态无法判定时跳过（`running_state_unknown`）。强制结束只能来自另一个单独审阅的计划（`--end-running`，仅交互终端可创建），且执行时还需在终端输入 `END` 确认；非交互执行永远不会结束进程。系统关键进程、Windows 目录下的程序和 CleanManager 自身永不结束。

## 残留（UNI-04..09）

- **身份证据**：注册的安装位置与卸载注册项、`%APPDATA%`/`%LOCALAPPDATA%`/`%PROGRAMDATA%` 下与产品名**完全相同**的目录或 `<发布者>\<产品>` 目录、包家族名数据目录、`HKCU/HKLM\SOFTWARE\[<发布者>\]<产品>` 键。名称变体只有去版本/架构后的整名、去空格和连字符形式；通用词（Update、Common Files、Microsoft、Cache…）与少于 3 个字符的名称从不匹配，不做子串或厂商前缀匹配。
- **保留标记**：另一个已安装实例（如测试版）可能使用的对象为 `shared_owner`；只按渠道名（“Foo Beta”→“Foo”）或过短的顶层名匹配为 `owner_unknown`；该应用已被重新安装（注册项与第一段记录的不同）为 `app_reappeared`。三者都显示为 `kept`，不能进入计划。
- **分组与默认选择（UNI-05）**：本体、缓存与日志、设置与偏好、个人数据/模型、启动与后台项、系统级位置。**只有缓存与日志默认选中**；名称匹配凭据/密钥/SSH/云配置/文档等敏感模式的对象带警告且从不预选；模型目录附“大、通常可由应用重新下载”的说明；家目录配置文件（`.gitconfig`、shell 与 PowerShell 配置、`.npmrc`、VS Code 与 Windows Terminal 设置等）中以完整路径提到安装目录的（`C:\Tools\Foo` 不匹配 `C:\Tools\FooBar`），只作只读提示（`external_references`），从不成为残留对象或计划动作，也从不修改。
- **执行方式**：文件与文件夹移入回收站（`fs_trash`，与手选移入回收站相同的保护、覆盖与身份核验；文件夹移走其内容，空文件夹保留）；注册表键 `registry_backup_delete`：执行前再读一次，内容须与计划时一致，先把整棵键树写入审计存储的备份，**备份写入失败就跳过删除**（`backup_failed`），文件残留照常处理；只处理 `SOFTWARE\[<发布者>\]<产品>`、该应用自己的卸载注册项与 App Paths 注册，从不触及 Microsoft、Classes、Policies 等共享或系统键。HKLM 与系统位置需要管理员终端（本工具从不自行提权）。
- **收尾步骤（UNI-09）**：指向安装目录的启动项（失效后可在备份后移除）、开始菜单快捷方式（移入回收站）、App Paths 注册与卸载注册项逐项作为动作执行并逐项报告；桌面上的快捷方式只给出处理指引（绑定它需要扫描整个桌面文件夹）；安装目录记录为 8.3 短路径时也按长路径匹配；指向安装目录的服务只报告手动处理指引（本工具不删除服务）。任何一步失败或跳过，操作结果都不是 `completed`。
- **恢复（UNI-06）**：`apps uninstall restore --operation OP` 生成 `recovery` 计划，从备份重建被删除的键与启动值；目标键或值已被重新创建时跳过（`destination_exists`），绝不覆盖。
- **AI 命令行工具（UNI-07）**：Claude Code（`~/.claude`）、Codex CLI（`~/.codex`）、Gemini CLI（`~/.gemini`）的会话/历史与设置/凭据单独列出且默认不选，缓存与日志默认选中；数据目录中的 git worktree 一律 `blocked`（不运行 git，无法排除未提交修改）。
- **孤立残留（UNI-08）**：`apps uninstall orphans` 列出指向已不存在程序的失效启动项，以及与该程序目录同名、无已安装实例认领且闲置至少 30 天的数据目录（30 天内用过的标 `owner_unknown` 保留；通用名称如 `Common Files` 不匹配）；全部 `review_only`、不预选，并说明缺失的目标路径。

## 用法

```text
cleanmanager apps uninstall list --json                                   # app_uninstall_list：每个实例的卸载方式或原因
cleanmanager apps list --json                                             # app_list：uninstall_capability 与上一行同一判定（available/unverified/unsupported）
cleanmanager apps uninstall plan create --instance app_0123 --json        # plan（uninstall）+ leftover_estimate
cleanmanager plan review up_demo                                          # 终端内审阅
cleanmanager plan apply up_demo --json                                    # operation_report：退出码、耗时与重扫结果
cleanmanager apps uninstall leftovers --operation op_demo --json          # uninstall_leftovers：分组、证据、标记
cleanmanager apps uninstall leftovers plan create --operation op_demo --items lo_abc,lo_def --json
cleanmanager apps uninstall restore --operation op_demo2 --json           # plan（recovery）：恢复被删除的键与启动值
```

测试通过 `CM_UNINSTALL_TEST_ROOT`（必须是 `HKCU\Software\CleanManagerTest\` 下的键）与 `CM_UNINSTALL_TEST_HOME`（临时家目录）把注册表与数据目录重定向（此时 npm 工具只使用临时家目录 `Program Files\nodejs` 下的夹具 Node.js，从不使用本机的；winget 从不查找）；端到端测试 `crates/cm-cli/tests/uninstall_flow.rs` 用系统 `cmd.exe` 的副本充当“供应商卸载程序”和“运行中的应用”，不卸载任何真实应用，并在前后比对真实卸载键与 Run 值未变；移入回收站的夹具对象在测试结束时按回收站自身的 `$I` 记录精确移除，运行前后回收站不变。
