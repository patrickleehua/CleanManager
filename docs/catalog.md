# 清理目录（Windows 第 1 波）

状态：变更 `add-mole-parity-suite` 任务 3.1–3.7 的 Windows 部分已实现；macOS 一侧（含 Safari、Xcode DerivedData 等）尚无实测，任务 3.5 的“每条 enabled 规则在 Windows 与 Mac 各有实测记录”在本机无法满足。类目、档位与命令形状以代码与 [CLI 协议](cli-protocol.md) 为准，本页说明“清什么、为什么是这个档位、证据在哪”。

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

执行前复核在 `crates/cm-platform/src/windows/actions.rs` 的既有检查之后调用 `probes::execution_check`（终端安全路径、同目录被占用的下载标记），并把所属进程表扩展到 `probes::owner_processes`。

## 所属工具委托（CAT-05）

适配器是编译进二进制的固定表（`cm_core::adapter::ADAPTERS`）：可执行文件只从安装程序写入的位置解析（HKLM `SOFTWARE\Node.js\InstallPath`、Program Files 已知文件夹），不查 `PATH`、不用当前目录；必须是普通文件（非链接），启动前记录 SHA-256；参数为固定数组、每项只含字母数字与 `-._`；不经 shell、不运行批处理；无窗口、空 stdin、工作目录为安装目录；有超时与输出上限。工具缺失、超时或失败均为 unavailable/timed_out/failed，**从不改为直接删除目录**。本轮只运行只读预览命令：

| 适配器 | 预览（只读） | 清理（已声明，本轮不执行） | 本机结果 |
| --- | --- | --- | --- |
| `npm.cache-verify` | `node npm-cli.js config get cache` | `cache verify`（回收未被引用的条目） | available，exit 0，报告的缓存根与解析器一致 |
| `go.build-cache-clean` | `go env GOCACHE` | `clean -cache` | unavailable：Go 不在 Program Files |
| `dotnet.nuget-http-cache` | `dotnet nuget locals http-cache --list` | `nuget locals http-cache --clear` | unavailable：未安装 .NET SDK |

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

## 未纳入本波的 Windows 项

- **回收站**：`trash` 类目在 Windows 为 unsupported；清空回收站是永久删除计划，留待手选回收站任务组（6.x）一并实现。
- **传递优化、Windows 更新下载、Prefetch**：需要系统接口或管理员（D4 为 `tool_adapter`，W2/W7），本轮不做；Prefetch 因影响启动性能不纳入。
- **缩略图缓存**：属于维护任务（MNT），不是文件规则。
- **Teams、Slack、NVIDIA/AMD 着色器缓存、云与办公、虚拟化**：第 2 波；本机未安装 Teams/Slack。
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
