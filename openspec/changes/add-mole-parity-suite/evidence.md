# Mole 对标证据与功能矩阵

本文件是规划依据，不是产品验收证据。所有“Mole 行为”均来自下列来源；CleanManager 列为本变更的拟实现规格，尚未实现或测试。

## 来源与边界

| 来源 | 内容 | 类型与限制 |
| --- | --- | --- |
| `.research/Mole`（tw93/Mole `main` @ `eae1043`，v1.59.1，2026-10-14） | `mole`、`bin/*.sh`、`lib/**`、`cmd/analyze`、`cmd/status`、`AGENTS.md`、`.claude/skills/{mole,bugs}` | 完整源码静态阅读（GPL-3.0，仅参考行为，不复制代码）；未运行 |
| `.research/Mole-windows`（`windows` @ `7c841a9`，v1.34.0） | `mole.ps1`、`bin/*.ps1`、`lib/**`、`cmd/analyze`、`cmd/status`、`SECURITY_AUDIT.md` | 同上；见下文“Mole CLI Windows 分支”一节 |
| REA `ev_12597c985231bb1290c4b972b9298ca62a4b7beb8030454ee0aa2670d30b74b6`（2026-10-15） | `mole.fit/zh/`、`mole.fit/zh/windows`、`mole.fit/blog/mole-cli-vs-mac-app` 无障碍树 | 受控浏览器回放，仅官网文字；未下载或运行 Mole Mac/Windows GUI，GUI 内部实现未知 |
| REA `ev_9f6646a91db82b68d551a30410f4d94d2dbb72b10d4f91e6c707472fdf1db2e0`（既有） | `mac-cleaner`、`mac-optimizer`、`mac-disk-analyzer`、`mac-system-monitor` 页 | 同上；已在原型 `research.json` 引用 |
| REA `ev_bb1307e5…`（既有） | `mac-app-uninstaller` 页 | 同上 |

观察与推断区分：表中“Mole”列为观察（源码行号或官网原文概括）；“CleanManager”列为设计选择。官网宣传数字（如“200 多类缓存”“824/369 款实测软件”）未经独立核实，只作为覆盖目标的参考量级。

## 功能矩阵

| # | Mole 功能（来源） | CleanManager 对应 | 差异/取舍 |
| --- | --- | --- | --- |
| 1 | `mo clean` 15 段清理：系统、用户必需、应用缓存、浏览器、云与办公、开发工具、GUI 应用、虚拟化、App Support 日志、残留、Rosetta、固件、TM 未完成备份、大文件仅审阅、purge 提示（`bin/clean.sh:1716-1832`） | `cleanup-catalog` CAT-01/02/03 | 以声明式规则 + 固定计划替代 shell 段；Windows 同构分组 |
| 2 | GUI 三档：默认可选 / 仅审阅不勾 / 拒绝（CLI vs App 博文） | CAT-02 | GUI 与 CLI 共用同一分类，不做“CLI 更宽” |
| 3 | AI 工具：缓存默认勾、旧会话/worktree 逐条确认、闲置 AI CLI 连同会话卸载（官网“月亮”） | CAT-04、`app-uninstall` UNI-07 | 会话/worktree 仍默认保护，仅逐对象人工确认，不可预授权 |
| 4 | 所属工具委托：`go clean -modcache`、`brew cleanup/autoremove`、`simctl` 孤立运行时、npm 仅清未引用（`lib/clean/dev.sh`、`brew.sh:294`、官网博文） | CAT-05 | 固定 argv 适配器 + 预览，不执行任意 shell |
| 5 | 有意保留：`~/.m2`、`~/.nuget/packages`、Cargo src/git、HF/torch 缓存等（`AGENTS.md:126`） | CAT-02 拒绝/仅审阅档 | 采用 |
| 6 | 运行中应用三态探测、SQLite 句柄、未完成下载句柄、EDR/MDM 缓存拒绝（`lib/core/file_ops.sh:880-1326`、`base.sh:1599`） | CAT-06、PRT-03 | 未知即拒绝 |
| 7 | 固定阈值：temp/log/崩溃 7 天、孤立 30 天、GPU 1 天、Mail 30 天且 5 MB（`base.sh:148-160`） | CAT-07 | 阈值随规则版本声明，不是全局常量 |
| 8 | 外接卷 `._*`、`.DS_Store`、`.Spotlight-V100` 等（`user.sh:1551-1700`） | CAT-08 | 仅用户选中的卸载卷 |
| 9 | whitelist：精确/glob/父子匹配、读失败即停、与 GUI 共享、每行可加入保护（`app_protection.sh:639`、`base.sh:365-376`、官网） | `protection-policy` PRT-01/02 | 保护名单只收紧，不能削弱硬拒绝 |
| 10 | 关键路径拒绝 + inode 比较 + 祖先 symlink 只拒不放 + 129 行危险路径模糊语料（`file_ops.sh:1020-1134`、`tests/fuzz_corpus`） | PRT-03/04 | Windows 增加 `C:\Windows`、`WinSxS`、`System Volume Information`、用户根等 |
| 11 | `mo purge`：~35 类产物、项目标记、Git 跟踪保护、7 天近期不预选、CACHEDIR.TAG（`lib/clean/purge_shared.sh`、`project.sh:507-560`） | `project-artifact-purge` | 依 GUI 原则：本地可重建产物默认可审阅，需联网恢复的依赖目录仅审阅 |
| 12 | `mo installer`：dmg/pkg/mpkg/iso/xip、含安装载荷的 zip、来源标签、身份绑定计划（`bin/installer.sh`） | `installer-cleanup` | Windows 增加 msi/msix/exe 安装包判定 |
| 13 | analyze：treemap 下钻、路径栏、Finder/Explorer 显示、移到废纸篓/回收站可放回、系统目录只读、废纸篓内确认后永久删除（官网 Mac/Win、`cmd/analyze/delete.go`） | `manual-trash` | 采用；仍生成固定计划并人工授权 |
| 14 | Windows：文件被占用时显示占用程序（官网） | MTR-04 | Restart Manager 查询，只读 |
| 15 | 洞察：iOS 备份、旧下载、快照计数、休眠文件、WSL 映像、iPhone 备份、WinSxS/pagefile 文章（`insights.go`、官网） | `disk-insights` | 只解释与指引，不默认删除 |
| 16 | 硬链接 (dev,inode) 去重、APFS clone 计一次（`scanner.go:1246`、`clone_darwin.go`） | 已在 MVP SCN-06 | 无新增 |
| 17 | 卸载：本体 + 残留（Library/系统路径/pkgutil BOM）、同 bundle 兄弟保护、强退运行中应用、launchctl/lsregister/登录项、Homebrew cask zap、个人数据默认不选、敏感模式告警（`bin/uninstall.sh`、`lib/uninstall/*`） | `app-uninstall` | Mac 默认移入废纸篓 |
| 18 | Windows 卸载：运行供应商卸载器、安装包/商店应用/Scoop、残留目录/快捷方式/注册表、注册表先备份、残留移回收站、其他方式卸载后的残留（官网 Win） | `app-uninstall` UNI-03/05/06 | 采用 |
| 19 | 更新：Sparkle/App Store/Homebrew/Electron feed，无法验证则打开页面；Windows winget（官网、博文） | `app-updates` | 采用 |
| 20 | 启动项：校验过的 launchd 作业与 SM 项可开关，未匹配的打开系统设置；Windows 备份后删除程序已不存在的启动项（博文、官网 Win） | `startup-items` | 采用 |
| 21 | `mo optimize` 20 项任务及六种结果、一次 sudo、whitelist_optimize、性能诊断、禁用 LaunchServices 重建（`lib/optimize/catalog.sh:31-90`、`outcomes.sh`） | `system-maintenance` | 采用；Windows 任务：重启开始菜单/搜索、图标缓存、DNS、到期 TRIM、DISM 检查、失效启动项 |
| 22 | “不改注册表优化项、不关服务、不精简”（官网 Win） | MNT-05 | 采用为禁止项 |
| 23 | `mo status`：CPU/GPU/内存/磁盘(SMART、可清除)/网络/电池/温度/风扇/蓝牙电量、健康分权重、诊断行、持续高 CPU 告警、`--json/--watch`（`cmd/status/*`） | `system-status` | 采用；只读，不杀进程 |
| 24 | 进程可排序/置顶并解释用途（官网） | STS-04 | 解释来自本地签名/路径/已知目录，可选 AI 诊断 |
| 25 | 菜单栏/托盘 HUD、保持常亮、清洁屏幕、隐私提醒、电池养护、风扇控制（SMJobBless helper）、WiFi 低延迟、蓝牙低电量（官网、博文） | `resident-utilities` | 常驻进程为新边界；特权项受限 helper |
| 26 | `mo touchid`（`pam_tid.so` → `/etc/pam.d/sudo_local`） | RES-06 | 仅 macOS，系统配置变更须备份与回退 |
| 27 | `mo history --json`、operations.log/deletions.log、`unknown` 而非 0（`lib/core/log.sh`） | 已在 MVP REC-04 | 无新增；Mole 无撤销，本产品有隔离恢复 |
| 28 | `mo update`（SHA256SUMS + Sigstore、安装锁）、`mo remove`（保留自定义配置）、`mo completion` | `product-lifecycle` | 采用 |
| 29 | Doctor 只读诊断报告（博文） | LIF-04 | 默认脱敏，用户预览后导出 |
| 30 | 不在本工具内的：常驻调度/通知（CLI 非目标）、隐私重置、包管理、按厂商前缀匹配（`AGENTS.md:35-43`） | 设计 Non-Goals | 采用相同边界；状态提醒属于用户开启的常驻工具 |
| 31 | 授权/试用/计费（官网） | 不在范围 | 本项目许可未定 |
| 32 | 无遥测（`SECURITY_DESIGN.md:246`、博文） | 沿用 MVP D12 | 无新增 |

## Mole CLI Windows 分支（v1.34.0 @ `7c841a9`）

来源：`.research/Mole-windows`（`git fetch origin windows`，2026-10-15 静态阅读，未运行）。路径相对该 worktree。它是开源 CLI，与官网描述的付费 Windows GUI 是不同实现；官网中的注册表备份、winget 更新、启动项管理、传递优化清理等在 CLI 中**不存在或未接入**。

| # | Mole Windows CLI 观察 | CleanManager 对应/影响 |
| --- | --- | --- |
| W1 | 路由只接受 clean/uninstall/analyze/status/optimize/update/remove/purge（`mole.ps1:355`）；无 installer/history/completion/启动项 | 这些能力的 Windows 依据只来自 GUI 官网与自有实测 |
| W2 | `mo clean` 实际运行：`%TEMP%`（7 天、仅顶层）、`%WINDIR%\Temp`（管理员）、WER 与 CrashDumps（无年龄过滤）、`%APPDATA%`/Temp 日志、Prefetch（14 天、管理员）（`lib/clean/user.ps1:35-286`） | CAT-01 第 1 波 Windows 用户临时/崩溃报告；Prefetch 不纳入（影响启动性能，证据不足） |
| W3 | 浏览器缓存仅 Default 配置档：Chrome/Edge `Cache/Code Cache/GPUCache/Service Worker CacheStorage`、Firefox `cache2/startupCache/shader-cache`、Brave/Opera（`lib/clean/caches.ps1:107-177`） | CAT-01 浏览器类目须遍历全部配置档；Service Worker CacheStorage 属站点存储，按 ANA-03 默认保护 |
| W4 | GPU 着色器缓存：NVIDIA DX/GL/NV_Cache、AMD、Intel、D3DSCache、Vulkan（`caches.ps1:309-376`）；无驱动安装包缓存 | 第 2 波；官网所述 NVIDIA 安装包清理在 CLI 中不存在 |
| W5 | 所属工具命令在隔离子进程运行（npm/pnpm/pip/go/mise），先用工具查询缓存根、检查保护与重解析点、执行前再次查询并绑定（`lib/clean/dev.ps1:93-147,616-671`） | 直接印证 CAT-05 与 design D1 `tool_adapter` 的预览—复核—执行 |
| W6 | 有意保留：Gradle、Docker（仅提示）、`.nuget\packages`、`.cargo`、`.rustup`、`.m2\repository`、Ollama 模型、JetBrains（`dev.ps1:352,370-375`，`lib/core/base.ps1:168-181`）；内置白名单与部分缓存规则自相矛盾（推断） | CAT-02 blocked/large_items；规则与保护必须由同一核心计算，避免此类矛盾 |
| W7 | 代码存在但未被调用：Windows Update 下载与传递优化（需停服务）、回收站、旧下载、孤立 AppData（`caches.ps1:382`、`user.ps1:46-108`、`apps.ps1:59-163`）；`Clear-RecycleBin` 自递归疑似缺陷（`user.ps1:82,108`） | 传递优化需经系统接口适配而非停服务删目录；未接入代码不作为“Mole 已支持”的证据 |
| W8 | 受保护前缀：Windows/System32/SysWOW64/Program Files/Defender；凭据/证书/开始菜单段；clean 额外保护 `.ssh/.aws/.kube/.docker`、浏览器登录/Cookie/IndexedDB、`.claude/.codex` 会话（`base.ps1:186-244`） | PRT-03 硬拒绝与默认保护类目的 Windows 起点 |
| W9 | 用户白名单 `~\.config\mole\whitelist.txt`，字面路径或 `-like` 通配，读失败全部保留（`base.ps1:162,440-490`） | PRT-01/02 一致 |
| W10 | 目标及每级父目录绑定卷+文件 ID 并在删除前复核，父链为链接即拒绝，目录内任一子项受保护则保留整个目录，目录遍历 15 秒上限（`lib/core/file_ops.ps1:32-332`）；`SECURITY_AUDIT.md:32,65` 承认检查与删除非原子 | 与 MVP EXE-01/02 一致；D6 对手选回收站接受同类残余竞态 |
| W11 | 自动清理与 purge **永久删除**；只有卸载残留经 `ITransferSource.RecycleItem` 进回收站且不降级（`file_ops.ps1:389-476`）；analyze 用 `SHFileOperationW` 进回收站并复核（`cmd/analyze/platform_windows.go:240-294`） | 本产品清理/purge 默认回收站或隔离，永久删除需显式模式 |
| W12 | 无运行进程/占用检测，占用失败只计为失败；无 `\\?\` 长路径处理；无删除历史 | CAT-06、MTR-04、MVP REC-04 均为本产品新增 |
| W13 | 卸载：HKLM/HKCU（含 WOW6432Node）卸载键 + AppX；MSI `msiexec /x /passive`、QuietUninstallString、Inno/NSIS 静默参数；以注册键消失判成功；残留仅限 InstallLocation 且需完整清单证明无其他所有者；无 winget/Scoop/注册表残留/备份（`bin/uninstall.ps1:74-796,1082-1088`） | UNI-01/03/04；静默参数仅在适配验证后使用；注册表残留与备份为 GUI 级能力，本产品自行实现 |
| W14 | 维护：Optimize-Volume、DNS、ARP、启动项计数（只读）、磁盘健康、更新检查、DISM CheckHealth、`sfc /scannow`（需确认）；只有计数器无逐项状态（`bin/optimize.ps1:144-614`） | MNT-01/03 采用逐项六态；ARP 清空不纳入首批（收益不明） |
| W15 | analyze 受保护根：Windows/Program Files/ProgramData/Users 默认与公共/Recovery/System Volume Information/`$Recycle.Bin`/Config.Msi/卷根/各用户根/Docker，规范化 `\\?\`、UNC、Volume GUID，拒绝 GLOBALROOT（`cmd/analyze/protect_windows.go:12-95`） | PRT-03/05 危险路径语料的 Windows 输入 |
| W16 | 状态：Win32/注册表采集，无 WMI；无 SMART/温度；健康分 CPU30/内存25/磁盘20/IO10 + 电池/开机时长扣分（`cmd/status/metrics_windows.go`、`metrics_health.go:55-191`） | STS-01 温度/SMART 在 Windows 默认 unavailable；健康分权重公开 |
| W17 | purge：用户目录与 `D:\` 下扫描根、标记文件深度 4、Git 跟踪保留、无近期过滤、永久删除（`bin/purge.ps1:37-608`） | PUR-04 近期过滤与 PUR-05 默认可恢复模式为本产品更保守的选择 |
| W18 | 更新：git 安装才自更新，winget/Scoop 提示对应命令；TUI 二进制校验 `SHA256SUMS.txt` 不符即中止（`bin/update.ps1:196-207`、`lib/core/tui_binaries.ps1:76-261`） | LIF-01 一致 |
| W19 | 测试：Pester 5 约 336 个用例、17 个 Go 测试；回收站集成测试需显式开启 | 本产品验证计划需真机回收站/卸载夹具 |

## 仍待取证

- ~~Mole CLI `windows` 分支的类目、路径与安全函数~~：已于 2026-10-15 读取，见上节。
- Mole Mac/Windows GUI 的实际界面与动效：仅有官网截图描述；视觉对齐以公开信息与原创实现为限。
- 官网“200 多类缓存”“实测软件”清单逐项内容未抓取；本变更以自有实测登记代替。

## 对标完成度复核（2026-10-15）

任务 18.5 的逐行复核见 [parity-review.md](parity-review.md)：矩阵 #1–#32（#25 拆为 25a/25b）、Windows 分支 W1–W19 都标注了状态（已实现(Windows) / 部分 / 待 macOS/真机/CI/签名 / 不采用）与依据；另补登了矩阵未列出的 Mole 功能 N1–N16 并指定归属。本文件开头“CleanManager 列为拟实现规格，尚未实现或测试”的说法反映的是规划时的状态，现在的实现状态以 parity-review.md 与 tasks.md 的勾选为准。到复核日为止，没有任何能力通过 G1–G4 门禁，也没有任何能力已发布。
