# 应用卸载

状态：变更 `add-mole-parity-suite` 任务 9.2–9.8 已勾选，9.1（来源适配）的 Windows 部分已实现，但因 macOS 来源未实现而未勾选。没有在测试中移除过真实的 MSIX 包，也未通过 G3 门禁（任务 18.3），没有发布。代码位置：核心 `cm_core::uninstall`（macOS 判定在 `cm_core::uninstall::mac`）、执行与计划 `cm_app::uninstall`、平台 `windows/uninstall.rs` 与 `macos/uninstall.rs`、命令 `apps uninstall …`。macOS 部分已实现（见下文“macOS”），只在托管 CI runner 上用夹具验证过，没有真机证据，任务 9.1 仍未勾选。命令形状以 [CLI 协议](cli-protocol.md) 为准。

默认关闭（设计 D15）：应用卸载尚未通过门禁 G3，所以它的 CLI 命令组、对应种类计划的审阅与执行、桌面页面和宿主命令都拒绝使用并说明原因。只为在这台电脑上评估时，用 `cleanmanager experimental --state on` 或桌面“设置 › 实验功能”开启（两者共用一个设置，见 [CLI 协议](cli-protocol.md) 的“实验功能”）；开启后能力仍报告为 `unverified`，不算已发布。

## 两段计划（D7）

1. **第一段** `apps uninstall plan create --instance ID`：`uninstall` 计划只含 `app_quit`（退出请求）和一个 `tool_adapter`（按来源选择的卸载方式）；批量时每个应用各一对（见下文 UNI-11）。审阅时附带 `leftover_estimate`（按组计数的**估计**，不是计划对象，也不被授权）。
2. 执行后核心**重新扫描**安装目录与卸载注册项，而不是根据退出码推断；仍有剩余时结果为 `partial`（动作代码 `leftovers_remain`，消息含退出码与耗时）。
3. **第二段** `apps uninstall leftovers --operation OP` 给出分组预览；`apps uninstall leftovers plan create --operation OP [--items ID,…]` 生成另一个 `uninstall` 计划，单独审阅、单独执行。

两段都需要 `plan review`（终端人工审阅）后才能 `plan apply`；策略永远不能匹配卸载计划，非交互调用者无法批准。

## 一份计划卸载多个应用（UNI-11，任务 9.9）

Windows 已实现；命令形状见 [CLI 协议](cli-protocol.md)。

- **选择**：CLI 重复 `--instance`（`apps uninstall plan create --instance A --instance B`，最多 20 个，同一实例重复为 `invalid_argument`）；桌面在“软件 › 卸载”中勾选多个“可卸载”的应用，选择栏显示已选数量，一次生成一份计划。受保护类与只能在系统设置中卸载的应用不能勾选。
- **受保护类不因批量放宽（UNI-10）**：每个应用都按单独卸载时完全相同的路径核验（来源适配、程序路径/哈希/签名者、保护类）。被拒绝的应用列在 `uninstall_refused` 中、不进入计划，其余应用的计划不受影响（CLI 退出码 5）；全部被拒绝时整个请求被拒绝，单个应用时错误与以前相同。
- **每个应用自己的阶段（D7）**：计划中每个应用各有一个 `app_quit` 和一个 `tool_adapter`，按选择顺序排列（第一个应用的动作 ID 与单独卸载相同：`q0000000`/`u0000001`，第 n 个为 `u{n:03}0000`/`u{n:03}0001`，因此动作 ID 的顺序就是执行顺序）。`plan/v2` 正文、摘要和种类规则没有改变，单应用计划仍完全有效。审阅按应用分组（CLI `plan review` 逐个列出“Application i/n”，桌面按应用分表）。
- **互不阻塞**：某个应用的卸载程序只等待它**自己**的退出请求；一个应用没有退出、卸载程序失败、被取消或登记在计划后变化（`record_changed`）只影响它自己，其余应用照常执行。整体状态在有应用失败时为 `partial`，从不报告为全部完成。
- **逐应用结果**：`plan apply` 与 `operations show` 的 `operation_report` 附带 `uninstall_apps`，每个应用一个 `status`（completed、partial、failed、skipped、not_started、unknown）、决定它的步骤代码与消息，以及 `leftovers_reviewable`；桌面结果页显示“逐个应用”。
- **残留只属于卸载成功的应用**：第二段的残留预览按应用分组（`apps`），只有卸载程序成功运行的应用列出对象；卸载失败、被跳过或没有运行的应用仍然安装着，它的文件、注册表项和启动项全部保留，残留计划从不包含它们，并且它仍作为已安装实例保护与其他应用共用的数据（`shared_owner`）。同一对象被两个应用认领时只在第一个应用下出现一次。没有任何应用卸载成功时（包括单个应用的卸载程序失败），`apps uninstall leftovers` 返回 `invalid_argument` 并说明原因。批量残留计划的说明文字以应用名开头，便于按应用审阅。残留默认选择规则不变：只有缓存与日志默认选中，个人数据与设置从不预选。
- **测试**：`crates/cm-app/src/uninstall/tests.rs`（两应用计划中一个卸载程序失败另一个完成、一个应用不退出只影响它自己、逐应用结果与整体拒绝）；端到端 `crates/cm-cli/tests/uninstall_batch.rs`（与 `uninstall_flow.rs` 相同的隔离：注册表与家目录重定向、`cmd.exe` 副本充当卸载程序，一个以退出码 3 失败；同一批中的终端安全代理被拒绝；不应用残留计划，回收站与真实卸载键不变）；桌面 `apps/desktop/src/views/uninstallBatch.test.ts`。

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
| macOS | 普通应用包（`app_bundle`） | 退出请求后把应用包移入当前用户的废纸篓（`macos.trash-app-bundle`，绑定应用包身份、`Contents/Info.plist` 哈希与签名发布者；从不删除） | 已实现（仅托管 runner 验证） |
| macOS | Homebrew cask / formula | 受信任的 `brew uninstall --cask <token>` / `brew uninstall --formula <name>`（固定参数，名称不能被读成选项；从不使用 `--zap`） | 已实现（夹具 brew） |
| macOS | npm 全局 AI CLI（Claude Code、Codex CLI、Gemini CLI） | 受信任的 Node.js 执行 `node <npm-cli.js> uninstall --global --prefix <该包所在前缀> --ignore-scripts <package>`（固定参数，包名受限不能被读成选项；从不运行包脚本） | 已实现（夹具 Node.js） |
| macOS | App Store、当前用户不能修改的应用包（属主是 root 或其他用户、所在文件夹只有管理员能改，pkg 安装的应用通常如此） | 本构建在 macOS 没有提权路径：`permission_required`，指向启动台或“访达 › 移到废纸篓”（访达会要求管理员密码） | 能力说明 |

## macOS

- **记录**：与 `apps list` 相同的清单（相同实例 ID）加上按需安装的 Homebrew formula；每条记录带 bundle ID、签名声称的 Team ID、应用包位置身份，以及当前用户能否移动它。`apps list` 的 `uninstall_capability` 与 `apps uninstall list` 是同一判定（可移动的应用包为 `available`；需要管理员的为 `unsupported`）。
- **保护类（UNI-10）**：`/System/Applications` 与封存系统卷上的应用、安全/设备管理代理、CleanManager 自身没有卸载入口（`policy_denied`）；批量时同样逐个拒绝。用户从 App Store 购买的应用不是保护类，但需要管理员，见上表。
- **退出（UNI-02）**：可执行文件位于应用包（formula 为其 keg）内的进程。LaunchServices 登记的应用收到 `NSRunningApplication terminate`（系统的退出请求，应用可以询问保存或拒绝）；没有应用事件循环的辅助程序/命令行程序收到 `SIGTERM`（可处理或忽略）。有限等待后仍在运行则第一段失败为 `still_running`，什么都不移动；强制结束（`forceTerminate`/`SIGKILL`）只在为此单独审阅的计划中、终端输入 `END` 后进行，CleanManager、关键进程与 macOS 自带进程永不结束。
- **移入废纸篓（N3）**：执行前重新读取记录（摘要须与计划一致）、`Info.plist` 哈希与签名发布者；移动时不跟随链接打开应用包所在文件夹并做命名空间门禁（属主为 root 或本用户、无他人可写；`/Applications` 是 `root:admin 0775`，`admin`/`wheel` 组可写被接受，因为其成员本就能以 root 身份操作），按计划身份无跟随地核对应用包，再走文件动作已有的已核验废纸篓路径（持有事件描述符、最后一次查看名称、`NSFileManager trashItemAtURL`、持有的描述符须指向结果项）。被拒绝时为 `trash_unavailable`/`access_denied` 等代码，无法确认时为 `unknown` 并提示到废纸篓查看；从不改为删除。
- **残留（UNI-04/05/09）**：`~/Library` 下 `Application Support`、`Caches`、`Logs`、`Preferences`（含 `ByHost`）、`Saved Application State`、`HTTPStorages`、`WebKit`、`LaunchAgents`、`Containers`、`Group Containers` 中按 bundle ID（同名或 `<bundle id>.` 前缀、launchd 作业标签或指向已移除应用包内的程序）或 Team ID（Group Containers）证据找到的对象。只按应用名匹配的对象为 `review_only`，从不预选；`Containers` 受隐私保护（App Data），只报告（`blocked`、`manual`，不读大小与内容）；`Group Containers` 由同一开发者的应用共享，`shared_owner` 保留；另有一个应用的登录项/后台项手动提示（指向“系统设置 › 通用 › 登录项与扩展”）。只有缓存与日志默认选中。残留计划为 `fs_trash`（移入废纸篓）；macOS 的废纸篓动作逐个移动已核验的文件，所以残留文件夹按其中的文件计划（清空后的文件夹保留），文件超过 5000 个的文件夹被拒绝并指向访达。LaunchAgents 定义移入废纸篓后下次登录不再加载，当前已加载的作业运行到那时为止（不运行 `launchctl`）。macOS 没有注册表步骤，也不收集“外部引用”提示。
- **AI 命令行工具（UNI-07）**：记录来自本用户 Node.js 使用的 npm 全局前缀下的 `<前缀>/lib/node_modules/<包>/package.json`：Homebrew（`/opt/homebrew`、`/usr/local`，后者也是 nodejs.org 安装器的位置）、nvm 的每个版本（`~/.nvm/versions/node/*`）、npm 文档中的用户前缀 `~/.npm-global` 与 `~/.npmrc` 的 `prefix=`；列出时不运行任何程序（不调用 `npm prefix -g`）。每个前缀中的每个包是一条记录（同一工具装在两个 nvm 版本中就是两条，按位置区分）；`npm link` 的链接包是开发检出，不列出；Volta 有自己的包存储，只作为 Node.js 来源，Volta 安装的包不列出；Claude Code 原生安装器的安装也不列出。卸载方式为 `node <npm-cli.js> uninstall --global --prefix <前缀> --ignore-scripts <package>`：前缀显式传入（执行器运行的是解析后的 `node`，Homebrew 的位于 Cellar 中，npm 由它推出的全局前缀会不同），从不运行包脚本。`node` 优先用该前缀自己的，否则依次为 Homebrew、`/usr/local`、nvm（新版本优先）、Volta；`node` 及其上每一级文件夹、`npm-cli.js` 及其上每一级文件夹都必须属于 root 或当前用户且无他人可写（组可写只接受 admin/wheel），与 Homebrew 的 `brew` 同一信任策略；环境清空后只加 `HOME`、`USER`/`LOGNAME` 并关闭 npm 的更新提示、fund 与 audit，有时限。计划绑定解析后 `node` 的 SHA-256，执行前重新读取记录、核对哈希并再次检查 `npm-cli.js`。运行中判定（UNI-02）：Node 运行这些工具，所以看本用户进程的可执行文件是否在包内（Codex 带原生程序），或前几个参数（`sysctl(KERN_PROCARGS2)`）解析后是否落在包内（`node /opt/homebrew/bin/claude` → 包内的 `cli.js`），随后与应用相同的退出请求（`SIGTERM`）与有限等待。执行后按包文件夹是否还在判定结果。`apps uninstall list` 的 `last_used_at` 取数据目录中最新条目的修改时间。残留只看该工具的数据目录（`~/.claude`、`~/.codex`、`~/.gemini`）逐个条目与家目录中的设置文件（`~/.claude.json`，只作手动提示）：只有缓存与日志默认选中；会话、历史、设置与凭据从不预选，必须逐个点名（ANA-03）；含会话名称（sessions、chats、checkpoint、history、`.jsonl` 记录等）的缓存目录按会话数据处理、不预选；含 git worktree 的条目 `blocked`；同一工具还有另一个安装时全部为 `shared_owner`、保留。包文件夹若仍在，作为本体组列出、不预选。
- **孤立残留（UNI-08）**：`apps uninstall orphans` 看当前用户 `~/Library` 下的 `Application Support`、`Caches`、`Logs`、`Preferences`、`Saved Application State`、`LaunchAgents`（`Containers` 只报告）。只有名称（Preferences 去掉 `.plist`、Saved Application State 去掉 `.savedState`、LaunchAgents 用作业标签）是 bundle ID 形状（至少三段反向域名）且没有任何已安装实例（`/Applications`、`~/Applications`、`/System/Applications` 中的应用包、App Store、Homebrew cask 与 formula、npm 全局工具）或安装收据（`/var/db/receipts`）认领的条目才是候选；认领包括同名、`<bundle id>.` 前缀的组件与反向前缀。Apple 的 `com.apple.*` 与 CleanManager 自己的条目从不是候选；只凭名称（不是 bundle ID 形状的文件夹）从不成为候选；`Group Containers` 由同一开发者共享，不看。launchd 作业只有在它启动的程序已不存在时才是候选，并说明缺失的路径。闲置期：条目自身及其直接子项的最新修改时间至少 30 天（`Containers` 不读）。候选一律不预选（缓存也不例外）；与已安装应用同一开发者前缀（如 `com.vendor.`）的为 `owner_unknown`，最后一段与已安装应用同名的为 `shared_owner`，30 天内改过或时间不明的为 `owner_unknown`，这些都保留、不能进入计划。系统级位置（`/Library`）不看：本构建在 macOS 不使用管理员权限。只扫描上述应用位置，其他卷或文件夹中的应用看不到，这也是候选必须闲置且只能逐个点名的原因。
- **孤立残留的计划**：`apps uninstall orphans plan create --items ID,…`（仅 macOS；Windows 为 `unsupported`，孤立残留只供审阅）生成一份单独审阅的 `uninstall` 计划，只含点名的对象，每个对象在重新发现时必须仍是候选；`kept` 为 `policy_denied`，`blocked`（Containers）为 `permission_required`，手动项为 `unsupported`，未点名为 `invalid_argument`。文件走与残留计划相同的 `fs_trash` 路径（重新扫描、保护与覆盖核验，文件夹按其中的文件计划）。
- **测试**：`crates/cm-core/src/uninstall/mac.rs`（路由、保护类、残留证据与默认选择）、`crates/cm-platform/src/macos/uninstall/tests.rs`（夹具应用包的记录与路由、`/bin/sleep` 副本被请求退出、忽略 `SIGTERM` 的 `/bin/bash` 副本只报告不结束、身份不符与链接不移动、夹具 brew、`~/Library` 夹具）、端到端 `crates/cm-cli/tests/mac_uninstall_flow.rs`（`CM_UNINSTALL_TEST_ROOT`/`HOME` 与 `CM_UPDATES_TEST_BREW` 重定向；两个 cask 的批量计划一个失败一个完成、受保护应用与 root 属主替身被拒绝、残留中的名称诱饵只供审阅）；AI 命令行工具与孤立残留：`crates/cm-core/src/uninstall/mac.rs`（npm 路由与包名/前缀校验、会话/设置/凭据从不预选、worktree 阻止、孤立残留的身份规则）、`crates/cm-platform/src/macos/uninstall/tests_ai.rs`（夹具前缀中的 `bin/node` 脚本：按前缀列出、经信任策略运行固定参数、他人可写的 `npm-cli.js` 被拒绝、`/bin/sh <包>/cli.js` 被识别为运行中、`~/Library` 夹具的孤立判定）、端到端 `crates/cm-cli/tests/mac_uninstall_ai_flow.rs`（实验开关、npm 卸载后数据目录只预选缓存、点名 worktree 被拒绝；孤立残留中已安装应用、`com.apple.*` 与名称诱饵都不出现，不点名不生成计划，点名后移入废纸篓）。移入真实废纸篓的步骤只在 `CM_ALLOW_MAC_DEVICE_TESTS=1` 或 `GITHUB_ACTIONS=true` 时运行，并按唯一名称移除自己移入的对象；否则只检查计划。

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
cleanmanager apps uninstall plan create --instance app_0123 --instance app_4567 --json  # 一份计划卸载两个应用：uninstall_apps、uninstall_refused
cleanmanager plan review up_demo                                          # 终端内审阅
cleanmanager plan apply up_demo --json                                    # operation_report：退出码、耗时与重扫结果
cleanmanager apps uninstall leftovers --operation op_demo --json          # uninstall_leftovers：分组、证据、标记
cleanmanager apps uninstall leftovers plan create --operation op_demo --items lo_abc,lo_def --json
cleanmanager apps uninstall restore --operation op_demo2 --json           # plan（recovery）：恢复被删除的键与启动值
cleanmanager apps uninstall orphans --json                                # uninstall_leftovers：孤立残留候选（review_only，从不预选）
cleanmanager apps uninstall orphans plan create --items lo_abc --json     # 仅 macOS：plan（uninstall，fs_trash）只含点名的孤立残留
```

测试通过 `CM_UNINSTALL_TEST_ROOT`（必须是 `HKCU\Software\CleanManagerTest\` 下的键）与 `CM_UNINSTALL_TEST_HOME`（临时家目录）把注册表与数据目录重定向（此时 npm 工具只使用临时家目录 `Program Files\nodejs` 下的夹具 Node.js，从不使用本机的；winget 从不查找）；端到端测试 `crates/cm-cli/tests/uninstall_flow.rs` 用系统 `cmd.exe` 的副本充当“供应商卸载程序”和“运行中的应用”，不卸载任何真实应用，并在前后比对真实卸载键与 Run 值未变；移入回收站的夹具对象在测试结束时按回收站自身的 `$I` 记录精确移除，运行前后回收站不变。
