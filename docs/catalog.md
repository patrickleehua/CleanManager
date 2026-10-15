# 清理目录（Windows 第 1、2 波）

状态：变更 `add-mole-parity-suite` 任务 3.1–3.7 的 Windows 部分已实现，任务 3.8（第 2 波）的 Windows 规则已在本机验证（见[第 2 波](#第-2-波-windows-规则)）；macOS 一侧（含 Safari、Xcode DerivedData 等）尚无实测，任务 3.5 的“每条 enabled 规则在 Windows 与 Mac 各有实测记录”在本机无法满足。类目、档位与命令形状以代码与 [CLI 协议](cli-protocol.md) 为准，本页说明“清什么、为什么是这个档位、证据在哪”。

默认关闭（设计 D15）：清理目录尚未通过门禁 G1，所以它的 CLI 命令组、对应种类计划的审阅与执行、桌面页面和宿主命令都拒绝使用并说明原因。只为在这台电脑上评估时，用 `cleanmanager experimental --state on` 或桌面“设置 › 实验功能”开启（两者共用一个设置，见 [CLI 协议](cli-protocol.md) 的“实验功能”）；开启后能力仍报告为 `unverified`，不算已发布。

## 档位如何算出

每个候选的档位只由核心的一个函数 `cm_core::catalog::compute_tier` 计算，`analyze`、`clean` 与桌面端都经由 `analysis::analyze_catalog` 得到同一结果（CAT-02）：

```text
tier = min(规则上限, 实测登记, 保护, 证据)；任一占用探测为 in_use 或 unknown → 跳过（不可选）
```

- **规则上限**：规则的 `max_tier`。只有可本地自动重建（`local_rebuild`）且重建已在本机实测的对象才可能是 `default_selected`。
- **依赖/包存储**：`package_manager_cache` 解析器、开发/AI 类目中需联网重下的对象，无论规则如何声明，恒为 `review_only`（design D3）。
- **实测登记**（CAT-03）：规则引用的登记条目记录应用/工具名、版本区间、系统版本、测试日期、正反例与所测阈值。安装版本从安装记录读取（卸载注册表项、Node.js 安装键、系统版本），不在区间内或无法确定时为 `version_untested`，档位最多 `review_only`。
- **阈值版本化**（CAT-07）：登记条目保存实测时的阈值；规则阈值一变而版本未变，整个规则集加载失败。时间戳不可得的对象不满足年龄条件，不成为候选，计入 `undetermined` 说明。
- **保护**：硬拒绝、默认保护类目（凭据、数据库、浏览器站点存储、AI 会话与配置、工作树、模型）与用户保护名单使对象为 `blocked`。`.claude`、`.codex` 等 AI 工具目录永远不是目录类目的目标。
- **证据**：未读子树、弱身份、硬链接共享等使档位最多 `review_only`。

## 占用三态探测（CAT-06）

规则声明所需探测；`clean` 预览与计划创建时各探测一次，执行器在每个动作前再次核验。结果只有 `in_use`、`not_in_use`、`unknown`，`unknown` 从不当作未占用。

| 探测 | Windows 实现 | unknown 的情形 |
| --- | --- | --- |
| `owner_process` | Toolhelp 进程快照，按所属应用的进程名表 | 快照失败、所属应用无映射 |
| `open_file_handle` | 对候选下每个文件做无共享打开（只请求读数据权限参与共享检查，不读内容、不跟随链接），共享冲突即 `in_use` | 访问被拒、文件超过 20 000 个或 10 秒、遇到需下载的云占位 |
| `open_database_handle` | 同上，只看数据库/日志类文件 | 同上 |
| `incomplete_download_handle` | `.crdownload`/`.part`/`.partial`/`.download`/`.opdownload` 被其他进程打开即 `in_use` | 列目录失败 |
| `endpoint_security_path` | 路径位于终端安全/设备管理产品数据目录（Defender、CrowdStrike、SentinelOne 等）即 `in_use` | — |
| `mounted_image`、`vcs_tracked` | 本组未实现（分别属安装包与构建产物任务） | 恒为 `unknown`，对象跳过 |
| `elevated_token` | 进程令牌已提升**且** Administrators 组在令牌中启用（`probes::has_admin_rights`）；`requires_admin` 的规则必须声明它，否则规则集加载失败 | 未提升，或受限令牌（例如 `runas /trustlevel` 得到的令牌仍报告 `TokenIsElevated=1`，但 Administrators 仅用于拒绝）——即拒绝提权时对象跳过，执行前复核同样拒绝 |

执行前复核在 `crates/cm-platform/src/windows/actions.rs` 的既有检查之后调用 `probes::execution_check`（终端安全路径、同目录被占用的下载标记），并把所属进程表扩展到 `probes::owner_processes`。

## 所属工具委托（CAT-05）

适配器是编译进二进制的固定表（`cm_core::adapter::ADAPTERS`）：可执行文件只从安装程序写入的位置解析（HKLM `SOFTWARE\Node.js\InstallPath`、Program Files 已知文件夹），不查 `PATH`、不用当前目录；必须是普通文件（非链接），启动前记录 SHA-256；参数为固定数组、每项只含字母数字与 `-._`；不经 shell、不运行批处理；无窗口、空 stdin、工作目录为安装目录；有超时与输出上限。工具缺失、超时或失败均为 unavailable/timed_out/failed，**从不改为直接删除目录**。本轮只运行只读预览命令：

| 适配器 | 预览（只读） | 清理（已声明，本轮不执行） | 本机结果 |
| --- | --- | --- | --- |
| `npm.cache-verify` | `node npm-cli.js config get cache` | `cache verify`（回收未被引用的条目） | available，exit 0，报告的缓存根与解析器一致 |
| `go.build-cache-clean` | `go env GOCACHE` | `clean -cache` | unavailable：Go 不在 Program Files |
| `dotnet.nuget-http-cache` | `dotnet nuget locals http-cache --list` | `nuget locals http-cache --clear` | unavailable：未安装 .NET SDK |
| `docker.system-prune`（第 2 波） | `docker system df` | `system prune --force`（停止的容器、未用网络、悬空镜像、构建缓存；不含卷） | unavailable：本机 Docker Desktop 为按用户安装（`%LOCALAPPDATA%\Programs`，用户可写），不是受信任位置 |

适配器行（`tool_adapter`）没有文件模式，`clean` 计划会以 policy_denied 拒绝它们；执行通道属于计划 `tool_adapter` 动作的后续工作。

## 第 1 波 Windows 规则

来源列引用 [对标证据](../openspec/changes/add-mole-parity-suite/evidence.md)（功能矩阵 #N、Windows 分支 WN）。证据为 `docs/evidence/runs/` 下的运行记录，由 `python -I tools/validate_catalog_windows.py --scratch DIR`（浏览器代码缓存与用户临时文件见 `tools/validate_rules_windows.py`）生成；实测设备 Windows 11 Enterprise 10.0.26200 x64。

| 规则 | 类目 | 档位上限 | 动作 | 实测版本 | 证据 | 来源 |
| --- | --- | --- | --- | --- | --- | --- |
| `google.chrome.code-cache` | browsers | default_selected | 文件 | Chrome 155 | run_win_rule_google_chrome_code_cache | #1、W3 |
| `google.chrome.gpu-cache` | browsers | default_selected | 文件 | Chrome 155.0.8059.41 | run_win_catalog_google_chrome_gpu_cache | #1、W3 |
| `google.chrome.http-cache` | browsers | review_only（需联网重下） | 文件 | Chrome 155.0.8059.41 | run_win_catalog_google_chrome_http_cache | #1、W3 |
| `microsoft.edge.code-cache` | browsers | default_selected | 文件 | Edge 155 | run_win_rule_microsoft_edge_code_cache | #1、W3 |
| `microsoft.edge.gpu-cache` | browsers | default_selected | 文件 | Edge 155.0.4283.45 | run_win_catalog_microsoft_edge_gpu_cache | #1、W3 |
| `microsoft.edge.http-cache` | browsers | review_only（需联网重下） | 文件 | Edge 155.0.4283.45 | run_win_catalog_microsoft_edge_http_cache | #1、W3 |
| `microsoft.vscode.cached-data` | developer | review_only（重建未实测） | 文件 | VS Code 1.141.0 | run_win_catalog_microsoft_vscode_cached_data | D4 VS Code 系编辑器缓存 |
| `microsoft.vscode.code-cache` | developer | review_only | 文件 | VS Code 1.141.0 | run_win_catalog_microsoft_vscode_code_cache | 同上 |
| `microsoft.vscode.gpu-cache` | developer | review_only | 文件 | VS Code 1.141.0 | run_win_catalog_microsoft_vscode_gpu_cache | 同上 |
| `npm.cache-content` | developer | review_only（依赖存储） | 所属工具 `npm.cache-verify` | npm 11.19.0 | run_win_catalog_npm_cache_content | #4、W5 |
| `npm.cache-logs` | logs | review_only，7 天 | 文件 | npm 11.19.0 | run_win_catalog_npm_cache_logs | W5 |
| `cargo.registry-cache` | developer | review_only（依赖存储；`registry\src` 与 Git 检出保留） | 文件 | Cargo 1.99.0 | run_win_catalog_cargo_registry_cache | #5、D4 |
| `windows.user-temp.stale-files` | user_essentials | review_only，7 天 | 文件 | Windows 10.0.22000 起 | run_win_rule_user_temp_stale | W2 |
| `windows.wer.report-archive` | logs | review_only，7 天 | 文件 | Windows 10.0.26200 | run_win_catalog_windows_wer_report_archive | W2 |
| `windows.wer.report-queue` | logs | review_only，7 天 | 文件 | Windows 10.0.26200 | run_win_catalog_windows_wer_report_queue | W2 |
| `windows.crash-dumps.user` | logs | review_only，7 天 | 文件 | Windows 10.0.26200 | run_win_catalog_windows_crash_dumps_user | W2 |
| `windows.directx.shader-cache` | system_data | review_only（重建未实测） | 文件 | Windows 10.0.26200 | run_win_catalog_windows_directx_shader_cache | W4 |
| `windows.downloads.incomplete` | incomplete_downloads | review_only，7 天 | 文件 | Windows 10.0.26200 | run_win_catalog_windows_downloads_incomplete | #6、D4 |
| `microsoft.wsl.distro-disk` | large_items | 提示行（不可选） | 无 | WSL 3.0.3.0 | run_win_catalog_microsoft_wsl_distro_disk | #15、CAT-09 |

取证方式：浏览器规则使用真实浏览器、一次性 `--user-data-dir` 与 127.0.0.1 测试服务器（创建 → 只匹配目标目录 → 删除后重建且页面正常）；VS Code、DirectX、npm、Cargo、WSL 只对真实位置做只读扫描（不删除，所以重建未实测的保持 review_only）；WER、崩溃转储、未完成下载与 npm 日志用受控夹具经真实解析器（本机这些位置不存在或属私人文件）。`run_win_catalog_probe_e2e` 证明 `clean --dry-run` 的实时探测：被占用的 `.crdownload` 为 `in_use` 并跳过，释放后为 `not_in_use` 但仍是 review_only，因此不带 ID 的非交互计划无可选项（policy_denied）。

### 保持 experimental 的规则

| 规则 | 原因（运行记录状态 incomplete） |
| --- | --- |
| `mozilla.firefox.cache2` | 本机未安装 Firefox |
| `go.build-cache` | Go 不在受信任安装位置，所属工具适配器不可用；不会改为直接删除缓存 |
| `apple.metadata.ds-store`、`apple.metadata.appledouble`、`apple.metadata.spotlight` | 本机没有接入可移动卷 |

## 第 2 波 Windows 规则

范围（design D4 G2）：通讯/设计/媒体/游戏应用缓存、云与办公、虚拟化、模拟器运行时、包管理器缓存、需管理员的系统日志。验收同第 1 波：enabled 规则必须有实测登记（`rules/registry/<rule>@1.0.0.json`）与本机正反例运行记录；本机无法观察的保持 experimental 并在运行记录中写明原因（状态 incomplete）。

**本机盘点（只读）**：卸载注册表项（HKLM 两个视图与 HKCU）、`%APPDATA%`/`%LOCALAPPDATA%`/`%ProgramData%` 目录、各工具版本命令与 `wsl -l -v`。已安装：飞书 7.75.18、微信 4.1.13.12、UU 远程 4.42.0.2770、Docker Desktop 4.95.0（按用户安装、从未创建引擎数据盘）、OneDrive 26.178.0913.0007、winget 1.29.380、pnpm 12.10.1（用户目录 node_modules）、Python 安装管理器 26.3（MSIX）、AMD Radeon 驱动 32.0.21028.2002、WSL（仅 Ubuntu）。未安装：Teams、Slack、Discord、Zoom、钉钉、Adobe 创意应用、Figma、Office 桌面版、Steam、Epic、NVIDIA 驱动、Android 模拟器、VirtualBox、Hyper-V 虚拟机；pip/uv/yarn/Gradle/Maven/NuGet 缓存目录均不存在。

**取证方式**：`python -I tools/validate_catalog_windows_wave2.py --scratch DIR --cli PATH`。对每条规则：

1. 真实位置只读扫描到临时 store，再 `analyze --preview-rule`；候选集合必须与脚本中独立实现的匹配预言（只用 `os` 列目录与时间戳）完全一致，且全部为 review_only。没有删除，因此再生成未实测，所有第 2 波规则上限均为 `review_only`。
2. 同名干扰夹具经真实解析器（`CM_RESOLVE_APPDATA`/`LOCALAPPDATA`/`PROGRAMDATA`）预览：只命中预期对象（例如 `GrShaderCache` 命中，而同名文件、`ShaderCacheOld`、嵌套的 `Default\CodeCache` 不命中；OneDrive 的 `general.keystore` 与 `.otc` 遥测缓存不命中）。
3. 年龄阈值（7 天、修改时间）：本机系统于 2026-10-10 安装，真实日志都不足 7 天，真实扫描正确地得到 0 个候选（反例）；正例来自 30 天前的夹具。登记条目记录所测阈值（CAT-07）。
4. 实时探测：`run_win_catalog_wave2_real_dry_run` 对真实的飞书、微信网页视图、Docker Desktop、UU 远程、AMD、pnpm、Python 位置运行 `clean --dry-run`（只预览，不建计划）：18 行全部 review_only、无预选；UU 远程正在运行，其两行 `owner_process` 为 in_use 而跳过；AMD 有两个着色器缓存被占用，`open_file_handle` 为 in_use 而跳过。
5. 需管理员项：`run_win_catalog_admin_skip_e2e` 用夹具分别在已提升终端与受限令牌（`runas /trustlevel:0x20000`，等同拒绝提权）下运行 `clean --dry-run`：前者 `elevated_token` 为 not_in_use、行为 review_only；后者为 unknown、`skip_reason=probe_unknown`，按 ID 请求计划返回 `policy_denied`。

| 规则 | 类目 | 档位上限 | 动作 | 实测版本 | 真实位置观察 | 证据 | 来源 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `bytedance.feishu.code-cache` | app_caches | review_only（重建未实测） | 文件 | 飞书 7.75.18 | `LarkShell\CodeCache` 1 项 | run_win_catalog_bytedance_feishu_code_cache | #1 |
| `bytedance.feishu.shader-cache` | app_caches | review_only | 文件 | 飞书 7.75.18 | `GrShaderCache`、`ShaderCache` 2 项 | run_win_catalog_bytedance_feishu_shader_cache | #1、W4 |
| `bytedance.feishu.logs` | logs | review_only，7 天 | 文件 | 飞书 7.75.18 | 0 项（日志不足 7 天） | run_win_catalog_bytedance_feishu_logs | #7 |
| `tencent.wechat.logs` | logs | review_only，7 天 | 文件 | 微信 4.1.13.12 | 0 项（日志不足 7 天） | run_win_catalog_tencent_wechat_logs | #7 |
| `tencent.wechat.web-code-cache` | app_caches | review_only | 文件 | 微信 4.1.13.12 | 3 个网页配置档的 `Code Cache` | run_win_catalog_tencent_wechat_web_code_cache | #1 |
| `netease.uu-remote.code-cache` | app_caches | review_only | 文件 | UU 远程 4.42.0.2770 | 1 项（运行中则跳过） | run_win_catalog_netease_uu_remote_code_cache | #1 |
| `netease.uu-remote.http-cache` | app_caches | review_only（需联网重下） | 文件 | UU 远程 4.42.0.2770 | 1 项 | run_win_catalog_netease_uu_remote_http_cache | #1 |
| `docker.desktop.code-cache` | virtualization | review_only | 文件 | Docker Desktop 4.95.0 | 1 项 | run_win_catalog_docker_desktop_code_cache | #1、W6 |
| `docker.desktop.gpu-cache` | virtualization | review_only | 文件 | Docker Desktop 4.95.0 | 1 项 | run_win_catalog_docker_desktop_gpu_cache | #1、W6 |
| `docker.desktop.http-cache` | virtualization | review_only（需联网重下） | 文件 | Docker Desktop 4.95.0 | 1 项 | run_win_catalog_docker_desktop_http_cache | #1、W6 |
| `docker.desktop.logs` | logs | review_only，7 天 | 文件 | Docker Desktop 4.95.0 | 0 项（日志不足 7 天） | run_win_catalog_docker_desktop_logs | #7 |
| `microsoft.onedrive.logs` | cloud_office | review_only，7 天 | 文件 | OneDrive 26.178.0913.0007 | 0 项（日志不足 7 天） | run_win_catalog_microsoft_onedrive_logs | #1、#7 |
| `microsoft.onedrive.setup-logs` | cloud_office | review_only，7 天 | 文件 | OneDrive 26.178.0913.0007 | 0 项（日志不足 7 天） | run_win_catalog_microsoft_onedrive_setup_logs | #1、#7 |
| `microsoft.winget.logs` | logs | review_only，7 天 | 文件 | winget 1.29.380 | 0 项（日志不足 7 天） | run_win_catalog_microsoft_winget_logs | #7 |
| `pnpm.metadata-cache` | developer | review_only（依赖存储） | 文件 | pnpm 12.10.1 | `metadata-v1.3` 1 项 | run_win_catalog_pnpm_metadata_cache | #4、W5 |
| `python.install-manager.downloads` | developer | review_only（依赖存储） | 文件 | Python 安装管理器 26.3 | 1 个运行时安装包（45 MB） | run_win_catalog_python_install_manager_downloads | #4 |
| `amd.shader-cache` | system_data | review_only | 文件 | AMD 驱动 32.0.21028.2002 | 5 个着色器缓存目录 | run_win_catalog_amd_shader_cache | W4 |
| `windows.wer.machine-report-archive` | logs | review_only，7 天，需管理员 | 文件 | Windows 10.0.26200 | 0 项（报告不足 7 天） | run_win_catalog_windows_wer_machine_report_archive、run_win_catalog_admin_skip_e2e | W2 |
| `windows.wer.machine-report-queue` | logs | review_only，7 天，需管理员 | 文件 | Windows 10.0.26200 | 0 项（报告不足 7 天） | run_win_catalog_windows_wer_machine_report_queue、run_win_catalog_admin_skip_e2e | W2 |

说明：

- **版本判定**：飞书、微信、UU 远程、Docker Desktop、OneDrive 从卸载注册表项读取，当前版本在登记区间内（`covered`）。winget、pnpm、Python 安装管理器、AMD 驱动没有卸载注册表记录（MSIX、用户目录或驱动），运行时判定为 `version_untested`，候选恒为 review_only；登记中的版本来自取证时的工具/驱动查询。
- **日志与运行中的应用**：OneDrive、飞书、微信等常驻运行，日志规则只声明 `open_file_handle`，只有应用已不再持有的旧日志可选；缓存规则同时声明 `owner_process`，应用运行时整行跳过。
- **不匹配的数据**：微信聊天数据（`Documents\xwechat_files`）与插件（`xwechat\XPlugin`，578 MB）、飞书的数据库与登录态、OneDrive 的日志密钥（`general.keystore`）与同步设置、AMD Software 设置（`AMD\CN`）、pnpm 内容寻址存储都在任何规则的匹配之外。

### 第 2 波保持 experimental 的规则

| 规则 | 类目 | 原因（运行记录状态 incomplete） |
| --- | --- | --- |
| `docker.desktop.data-disk` | large_items（提示行） | Docker Desktop 已安装但从未创建引擎数据盘（无 `%LOCALAPPDATA%\Docker\wsl`） |
| `docker.engine.unused-data` | virtualization（所属工具 `docker.system-prune`） | 适配器不可用：按用户安装不在受信任位置，且没有引擎数据；清理命令从未运行，数据盘也不会被直接删除 |
| `microsoft.teams.code-cache` | app_caches | 本机未安装 Teams |
| `slack.code-cache` | app_caches | 本机未安装 Slack |
| `discord.code-cache` | app_caches | 本机未安装 Discord |
| `valve.steam.web-code-cache` | app_caches | 本机未安装 Steam |
| `epicgames.launcher.webcache` | app_caches | 本机未安装 Epic Games Launcher |
| `adobe.media-cache` | app_caches | 本机未安装 Adobe 创意应用 |
| `nvidia.shader-cache` | system_data | 本机没有 NVIDIA 驱动 |
| `google.android-emulator.avd-images` | large_items（提示行，模拟器运行时） | 本机没有 Android 模拟器虚拟设备 |

### 第 2 波未纳入的项

- **`C:\Windows\Logs`（CBS、DISM 等）、`C:\Windows\LiveKernelReports`（本机 4.3 GB）、`C:\Windows\Temp`**：位于 Windows 目录硬拒绝子树内，任何规则都只能得到 blocked；应由系统自身的存储感知/磁盘清理处理，属于维护或洞察指引，而不是文件规则。
- **`%ProgramData%\Package Cache`、`%ProgramData%\Microsoft\VisualStudio\Packages`**：安装程序修复与卸载所需，不作为清理目标。
- **Zoom、钉钉、Figma、Office 桌面版、VirtualBox、Hyper-V、pip/uv/yarn/Gradle/Maven**：本机未安装或无对应目录，暂未编写规则；等有可观察的安装再按同样流程加入。
- **winget/Scoop 下载缓存**：winget 下载在 `%TEMP%\WinGet`（由用户临时文件规则覆盖），本机无 Scoop。

## 未纳入本波的 Windows 项

- **回收站**：`trash` 类目在 Windows 为 unsupported；清空回收站是永久删除计划，留待手选回收站任务组（6.x）一并实现。
- **传递优化、Windows 更新下载、Prefetch**：需要系统接口或管理员（D4 为 `tool_adapter`，W2/W7），本轮不做；Prefetch 因影响启动性能不纳入。
- **缩略图缓存**：属于维护任务（MNT），不是文件规则。
- **通讯、媒体/游戏、云与办公、虚拟化、模拟器、GPU 着色器缓存、需管理员的系统日志**：见第 2 波。
- **Claude/Codex/Cursor 应用缓存**：本机未安装对应桌面应用；`.claude`、`.codex` 目录始终受保护，不作为类目目标。
- **有意保留**：`.nuget\packages`、`.m2\repository`、Cargo `registry\src` 与 `git`、模型缓存不在目录中（evidence #5、W6）。

## 大项提示与外接卷元数据

- **大项提示行**（CAT-09）：`review_only_row` 规则生成的行没有文件模式、从不预选，`clean` 计划直接拒绝，也不会吞并其下可执行的子候选；只给出大小与处理指引（例如 WSL 磁盘请在发行版内释放或用 WSL 自身工具压缩/注销）。
- **外接卷元数据**（CAT-08）：Windows 自身在外接卷上生成的元数据没有安全的清理对象——`System Volume Information` 与 `$RECYCLE.BIN` 属硬拒绝，`desktop.ini` 是用户的文件夹自定义，`Thumbs.db` 属数据库保护类。因此 Windows 一侧只针对 Mac 在该卷上留下的 `.DS_Store`、`._*`、`.Spotlight-V100`，且仅当用户以 `clean --volume E:` 明确选择一个**可移动**驱动器根（非系统盘、驱动器类型为 removable）时才解析出根；分析只在该卷扫描内按身份匹配，链接与挂载点是边界，不会进入其他卷。三条规则因本机没有可移动卷保持 experimental。

## 命令

```text
cleanmanager clean list --json                                         # 类目、规则、登记覆盖、适配器预览
cleanmanager scan --root "%LOCALAPPDATA%" --json                       # 先扫描（只写本工具索引）
cleanmanager clean --scan ID --dry-run --non-interactive --json        # 预览：同一筛选逻辑，不改目标、不耗授权
cleanmanager clean --scan ID --mode trash --non-interactive --json     # 固定计划：只含未跳过的 default_selected
cleanmanager clean --scan ID --mode quarantine --candidates c_1,c_2 --json  # 显式 ID 可加入 review_only，blocked/跳过/提示/工具行被拒
cleanmanager plan review PLAN_ID                                       # 授权仍需人工审阅或已启用的策略
```
