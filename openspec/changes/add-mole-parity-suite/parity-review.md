# Mole 对标完成度复核（任务 18.5）

复核日期：2026-10-15。快照：分支 `feat/mvp-core-and-mole-parity` @ `03eaa06`。本文逐行核对 [对标证据](evidence.md) 的功能矩阵（#1–#32）与 Mole CLI Windows 分支观察（W1–W19），并补登矩阵未列出的 Mole 功能（N1–N16），让每项功能都有归属。它是规划/验收工件，不是产品运行证据；任务 18.5 的勾选仍以 [tasks.md](tasks.md) 为准，本文不改动任何勾选。

## 判定口径

- **证据来源**：只认 [tasks.md](tasks.md) 与 `define-cleanmanager-mvp/tasks.md` 中**已勾选**的任务，加上 `docs/` 中对应的说明文档与其中列出的测试/运行记录。代码存在但任务未勾选的部分，不算已实现。
- **没有任何一行是“已发布”**：G0（MVP 6.1–6.3）与本变更的 G1–G4（18.1–18.4）都没有勾选，CI（MVP 1.5）未实际运行，没有签名发行包（MVP 14.2）。`cleanmanager capabilities --json` 在本快照中把全部 7 项执行/扫描能力报告为 `unverified`，也没有列出任何对标能力（清理目录、卸载、维护等）的能力项，这和 18.3 验收要求的“未通过能力在能力矩阵为不可用”还有差距。
- **四种状态**：
  - `已实现(Windows)`：该行 Windows 部分对应的任务全部已勾选，有文档与测试；macOS 部分可能仍待实现（在“缺口/阻塞”列写明）。仍未通过门禁，也没有发布。
  - `部分`：Windows 一侧本身就缺能力（缺什么写在“缺口/阻塞”列），或该行没有任何已勾选任务。
  - `待 macOS/真机/CI/签名`：Windows 实现与文档已有，但对应任务因所列门槛未勾选。
  - `不采用`：理由已经写在 design/specs/MVP 任务中（引用出处）。凡是规格中**没有记录**取舍理由的未覆盖功能，一律不判为“不采用”，而是标为 `部分`，并注明需要用 `/opsx:update` 补记。

## 汇总

| 范围 | 已实现(Windows) | 部分 | 待 macOS/真机/CI/签名 | 不采用 | 合计 |
| --- | --- | --- | --- | --- | --- |
| 功能矩阵 #1–#32（#25 拆为 25a/25b） | 14 | 6 | 11 | 2 | 33 |
| Windows 分支 W1–W19 | 11 | 6 | 2 | 0 | 19 |
| 新补登 N1–N16 | 7 | 0 | 3 | 6 | 16 |

## 功能矩阵复核（#1–#32）

| # | Mole 功能（简称） | 归属 | 状态 | 依据（已勾选任务 / 文档 / 测试） | 缺口 / 阻塞 |
| --- | --- | --- | --- | --- | --- |
| 1 | `mo clean` 15 段清理 | cleanup-catalog CAT-01/02/03 | 部分 | 3.1–3.4、3.6、3.7；[catalog.md](../../../docs/catalog.md)：Windows 第 1 波 19 条、第 2 波 19 条规则启用，均附 `docs/evidence/runs` 记录 | Windows：回收站清空类目为 unsupported，传递优化 `tool_adapter` 未实现，Claude/Codex/Cursor 缓存规则缺失（本机未安装），所属工具的执行通道未接入。macOS：Safari、Xcode、Rosetta、固件、Time Machine 等分段都没有。3.5、3.8 未勾选（需要 Mac 实测），G1 18.1 未通过 |
| 2 | GUI 三档（默认可选 / 仅审阅 / 拒绝） | CAT-02 | 已实现(Windows) | 3.1（`compute_tier` 由 GUI/CLI 共用）、16.3（`tierBadge`）；catalog.md“档位如何算出” | G1/G2 门禁（18.1、18.2）未通过 |
| 3 | AI 工具：缓存、会话/worktree 逐条确认、闲置 AI CLI 卸载 | CAT-04、UNI-07 | 部分 | 2.5（`protected_reviewable`）、9.7（Windows npm 全局 Claude Code/Codex/Gemini CLI 卸载，会话默认不选）；[app-uninstall.md](../../../docs/app-uninstall.md) | 目录中没有 AI 工具缓存或“被替换旧版本”规则；`.claude`/`.codex` 永远不是目录目标，因此目录里没有 CAT-04 要求的“旧会话逐条确认”入口；macOS 未实现 |
| 4 | 所属工具委托（go/brew/simctl/npm） | CAT-05 | 部分 | 3.3（固定适配框架，只运行只读预览）；catalog.md“所属工具委托” | catalog.md 写明清理执行通道是后续工作；go/.NET/Docker 适配器在本机不可用；brew、simctl 不存在 |
| 5 | 有意保留（.m2、.nuget、Cargo src/git、模型） | CAT-02 | 已实现(Windows) | 3.1；catalog.md“有意保留” | — |
| 6 | 运行中应用三态、数据库/下载句柄、EDR 拒绝 | CAT-06、PRT-03 | 已实现(Windows) | 3.2、2.1；catalog.md CAT-06 表；`run_win_catalog_probe_e2e` | macOS 探测未实现 |
| 7 | 年龄/大小阈值 | CAT-07 | 已实现(Windows) | 3.4；登记中保存所测阈值 | — |
| 8 | 外接卷 `._*`/`.DS_Store`/`.Spotlight-V100` | CAT-08 | 待 macOS/真机 | 3.6（实现与 `clean --volume`） | 三条规则因本机没有可移动卷，保持 experimental；Mac 一侧未实现 |
| 9 | whitelist（精确/通配、读失败即停、GUI/CLI 共享） | PRT-01/02 | 已实现(Windows) | 2.3、2.4、2.6；MVP 13.5（保护设置页）；[safety-and-rules.md](../../../docs/safety-and-rules.md) | — |
| 10 | 关键路径拒绝、身份比较、祖先链接、危险路径语料 | PRT-03/04/05 | 待 CI | 2.1；语料 `tests/corpus/dangerous-paths.json` 与 `.github/workflows/ci.yml` 已在仓库中 | 2.2 未勾选：CI 未实际运行（MVP 1.5 未勾选）；macOS 硬拒绝没有真机记录 |
| 11 | `mo purge` 项目产物 | project-artifact-purge | 已实现(Windows) | 4.1–4.4；[cli-protocol.md](../../../docs/cli-protocol.md)“构建产物与安装包”；`purge_flow.rs` | G1 18.1 未通过（需要双平台） |
| 12 | `mo installer` 安装包 | installer-cleanup | 已实现(Windows) | 5.1–5.3；`installer_flow.rs` | macOS 的 dmg/pkg/xip 位置与挂载判定没有 Mac 记录；18.1 |
| 13 | analyze 移到回收站、系统目录只读、回收站内永久删除 | manual-trash | 待 macOS | 6.1、6.3、6.4、6.5；MVP 13.2（treemap）；[manual-trash.md](../../../docs/manual-trash.md) | 6.2 未勾选（macOS 废纸篓与 D6 竞态夹具）；桌面端浏览/永久删除回收站仅命令行可用（[visual-identity.md](../../../docs/visual-identity.md) 信息架构表） |
| 14 | Windows 显示占用程序 | MTR-04 | 已实现(Windows) | 6.4（Restart Manager） | — |
| 15 | 空间洞察（iOS 备份、快照、休眠、WSL、WinSxS） | disk-insights | 待 macOS | 7.1、7.3；[system-status.md](../../../docs/system-status.md)“概览洞察卡片” | 7.2 未勾选（APFS 快照、设备备份等 macOS 对象） |
| 16 | 硬链接去重、APFS clone 计一次 | MVP SCN-06 | 待 macOS | MVP 3.3、4.2（Windows 硬链接） | MVP 5.3 未勾选（APFS clone） |
| 17 | macOS 卸载（本体、残留、launchd、Homebrew zap） | app-uninstall | 待 macOS | — | 9.1 未勾选；MVP 11.2 未勾选；app-uninstall.md：macOS 返回 `unsupported` |
| 18 | Windows 卸载（供应商卸载器、MSI/商店/Scoop、残留、注册表备份） | UNI-02..10 | 已实现(Windows) | 9.2–9.8；app-uninstall.md；`uninstall_flow.rs` | 来源适配属于 9.1（未勾选，因含 macOS 部分）；没有在测试中移除过真实的 MSIX 包；G3 18.3 未通过 |
| 19 | 更新（Sparkle/App Store/Homebrew/Electron；winget） | app-updates | 待 macOS | 10.2、10.3；[app-updates.md](../../../docs/app-updates.md)（winget/商店/Scoop/自更新应用检测） | 10.1 未勾选（macOS 通道）；18.3 |
| 20 | 启动项开关、备份后删除失效项 | startup-items | 部分 | 11.2、11.3；[startup-items.md](../../../docs/startup-items.md) | 11.1 未勾选。Windows 的计划任务与商店启动任务没有开关（`unverified`，D9 规划的任务计划程序启停未实现）；macOS SM/launchd 未实现 |
| 21 | `mo optimize` 任务、六种结果、一次 sudo、性能诊断 | system-maintenance | 待 真机/macOS | 13.1、14.1、14.4；[system-maintenance.md](../../../docs/system-maintenance.md)；[privilege-boundary.md](../../../docs/privilege-boundary.md) | 14.2 已勾选（五项任务都有真机记录：TRIM applied，图标缓存与 Shell 重启 attention）；真实 UAC 路径没有记录；14.3 未勾选（macOS）；未入首批的 macOS 任务见 N15 |
| 22 | 不做注册表优化、关服务、精简 | MNT-05 | 已实现(Windows) | 14.4（禁止项清单，`optimize list --search` 只返回原因） | — |
| 23 | `mo status` 指标、健康分、诊断行、高 CPU 告警、`--json/--watch` | system-status | 部分 | 12.2、12.4；system-status.md“状态流” | 12.1 未勾选：缺 OEM 可得性真机记录；Windows 温度、SMART、风扇默认 unavailable（W16）；macOS 未实现 |
| 24 | 进程排序/置顶/用途解释 | STS-04 | 已实现(Windows) | 12.3；system-status.md“进程列表” | — |
| 25a | 托盘 HUD、保持亮屏、清洁屏幕、隐私提示、蓝牙低电量 | resident-utilities RES-01..04、07 | 部分 | 无已勾选任务 | 15.1、15.2 未勾选（本快照中正在另行实现，尚未验收）；18.4 |
| 25b | 风扇控制、电池养护、WiFi 低延迟（特权 helper） | RES-05 | 待 macOS/签名（Windows：不采用） | — | Windows 不提供：design Non-Goals 与 D12（“Windows 首批不提供风扇/电池写入”）。macOS 依赖 13.2 签名 helper、15.3 与 18.4 硬件矩阵 |
| 26 | `mo touchid` | RES-06 | 待 macOS | — | 15.4 未勾选，只适用 macOS |
| 27 | `mo history --json`、操作/删除日志、`unknown` | MVP REC-04 | 已实现(Windows) | MVP 10.1–10.3；`operations list`、`optimize history` | MVP 10.4 未勾选（G1 双平台核心集成） |
| 28 | `mo update`（校验和+签名、锁）、`mo remove`、`mo completion` | product-lifecycle LIF-01..03 | 待 签名 | 17.1–17.3；[install.md](../../../docs/install.md) | 没有签名发行包，也没有发布密钥/通道（MVP 14.1、14.2 未勾选）；桌面更新器只有设计（install.md“桌面更新器”） |
| 29 | Doctor 只读诊断 | LIF-04 | 已实现(Windows) | 17.4；`lifecycle_tools.rs` | — |
| 30 | 不在工具内：常驻调度/通知、隐私重置、包管理、按厂商前缀匹配 | design Non-Goals | 不采用 | design Non-Goals（隐私痕迹重置；不按厂商前缀批量匹配残留）；常驻提醒只属于用户开启的 RES-01 | — |
| 31 | 授权/试用/计费 | — | 不采用 | design Non-Goals（不做授权码/试用/计费） | — |
| 32 | 无遥测 | MVP D12 | 已实现(Windows) | [privacy.md](../../../docs/privacy.md)；AI 诊断只发往用户自配的提供方（MVP 15.2/15.3） | — |

## Windows 分支观察复核（W1–W19）

| # | 对应能力 | 状态 | 依据 / 缺口 |
| --- | --- | --- | --- |
| W1 | installer/history/completion/启动项（Mole Windows CLI 中没有） | 已实现(Windows) | 5.x、MVP 10.3、17.3、11.2/11.3 |
| W2 | 用户临时、WER、崩溃转储；`%WINDIR%\Temp`；Prefetch | 已实现(Windows) | 第 1 波规则（catalog.md）。`%WINDIR%\Temp` 位于硬拒绝子树内，Prefetch 按 W2 不纳入（catalog.md“第 2 波未纳入的项”） |
| W3 | 浏览器缓存遍历全部配置档 | 部分 | Chrome/Edge 已启用；Firefox 保持 experimental（本机未安装）；Brave/Opera 没有规则 |
| W4 | GPU 着色器缓存 | 部分 | DirectX、AMD 已启用；NVIDIA experimental（本机没有该驱动）；Intel/Vulkan 没有规则 |
| W5 | 所属工具在子进程中执行并复核 | 部分 | 同 #4：只有预览，没有执行 |
| W6 | 有意保留与保护同源计算 | 已实现(Windows) | 3.1、2.5 |
| W7 | WU 下载/传递优化、回收站、旧下载、孤立 AppData | 部分 | 旧下载审阅（7.3）、孤立残留（9.7 `apps uninstall orphans`）、回收站内永久删除（6.3）已实现；传递优化与 WU 下载未实现，回收站类目在 Windows 为 unsupported |
| W8 | 受保护前缀与凭据/会话保护 | 已实现(Windows) | 2.1、2.5 |
| W9 | 用户白名单 | 已实现(Windows) | 2.3、2.4 |
| W10 | 目标与父链身份绑定、执行前复核 | 已实现(Windows) | MVP 9.3、9.4 |
| W11 | 默认可恢复模式 | 已实现(Windows) | 4.4、MVP 9.4、6.1 |
| W12 | 占用检测、长路径、删除历史 | 已实现(Windows) | 3.2、6.4、MVP 4.2、10.3 |
| W13 | 卸载：注册项、AppX、MSI | 已实现(Windows) | 9.2–9.6；来源适配属于 9.1（未勾选，见 #18） |
| W14 | 维护：TRIM、DNS、DISM（ARP 不纳入） | 已实现(Windows) | 14.2 已勾选（见 #21）；`sfc /scannow` 等修复动作不纳入（D10：只读检查，不自动修复） |
| W15 | analyze 受保护根、`\\?\`/UNC/GLOBALROOT 规范化 | 待 CI | 2.1 已勾选；语料与 CI 属 2.2，未勾选 |
| W16 | 状态：无 SMART/温度，健康分权重 | 部分 | 12.2 已勾选（公开权重）；12.1 未勾选 |
| W17 | purge 近期过滤、可恢复模式 | 已实现(Windows) | 4.4 |
| W18 | 自更新校验和、包管理器提示 | 待 签名 | 17.1 已勾选；没有签名发行包（MVP 14.2） |
| W19 | 真机回收站/卸载夹具 | 部分 | `uninstall_flow.rs` 用 `cmd.exe` 副本充当卸载程序；真实供应商卸载器、MSIX 移除与注册表恢复的真机记录属于 18.3，未完成 |

## 矩阵未列出的 Mole 功能（新补登）

来源均为 `.research/Mole`（`eae1043`）的静态阅读。

| # | Mole 功能（来源） | 归属 | 状态 | 依据 / 说明 |
| --- | --- | --- | --- | --- |
| N1 | 无参数交互主菜单与 TUI 键盘导航（`mole` `interactive_main_menu`、`lib/ui/*`） | 本变更范围外 | 不采用 | MVP tasks“Workflow follow-up”：TUI/MCP 另建 proposal；本产品以桌面端与 CLI 文本输出代替 |
| N2 | `--debug` 详细日志（`lib/core/help.sh`） | agent-cli CLI-14 | 已实现(Windows) | 8.4；design D16；`--debug` 轨迹只写标准错误、经 Doctor 脱敏，`crates/cm-cli/tests/debug_trace.rs` 证明标准输出逐字节不变、密钥不出现 |
| N3 | `mo uninstall --permanent` 绕过废纸篓 | app-uninstall | 不采用 | design D1：`uninstall` 计划种类不含 `fs_delete`，残留只进回收站或隔离区 |
| N4 | 一次卸载多个应用（`mo uninstall slack zoom`、`lib/uninstall/batch.sh`） | app-uninstall UNI-11 | 已实现(Windows) | 9.9；design D16；`apps uninstall plan create` 可重复 `--instance`（≤20），桌面多选，按应用分组审阅与逐应用结果；`crates/cm-cli/tests/uninstall_batch.rs`（一个失败另一个完成、受保护类被拒） |
| N5 | 识别 Steam 生成的启动器 `.app`（`lib/uninstall/steam.sh`） | app-uninstall UNI-01 | 待 macOS | 只出现在 macOS；按 UNI-01，不支持的来源应给出能力说明（9.1 未勾选） |
| N6 | `mo purge --yes` 非交互确认删除 | agent-cli CLI-12 | 不采用 | CLI-12 与 8.2（已勾选）：移除任何类似 `--yes` 的授权语义，非交互调用不扩大选择 |
| N7 | `mo purge --paths` 自定义扫描根 | PUR-01 | 已实现(Windows) | 4.1；`purge roots add\|remove`（cli-protocol.md） |
| N8 | `mo analyze --json` 与 `scan_status` 完整性 | MVP CLI-02/06、SCN-03 | 已实现(Windows) | MVP 2.3、12.1：`scan_result` 区分 verified_scope/partial_scope，未知不写 0 |
| N9 | analyze 中“可手动清理”目录标记（`cmd/analyze/cleanable.go`，CACHEDIR.TAG 等） | disk-insights DIN-05 | 已实现(Windows) | 7.4；design D16；`entries list` 与桌面磁盘视图的 `cleanable_hint`（构建产物签名、目录规则、CACHEDIR.TAG），只读、不预选，保护优先；`crates/cm-cli/tests/cleanable_hints.rs` |
| N10 | 安装渠道与更新通道：`install.sh` 指定版本、`mo update --nightly`、Homebrew、Nix flake、启动时检查新版本 | product-lifecycle LIF-01、MVP 14.1 | 待 签名 | design D16 已补记：指定版本随签名发行包提供（MVP 14.1/14.2），包管理器渠道在签名发行后另行提交；nightly 不采用（只发布签名版本化通道），启动时自动检查不采用（UPD-04） |
| N11 | Raycast/Alfred 快速启动器（`scripts/setup-quick-launchers.sh`） | agent-cli CLI-01 | 不采用 | design D16：启动器脚本是第三方扩展格式，任何启动器都可直接调用 `cleanmanager`，随附 Skill（CLI-13）是集成面 |
| N12 | 随附 Agent Skill（`.claude/skills/mole`） | agent-cli CLI-13 | 已实现(Windows) | 8.3、MVP 12.4；`skills/cleanmanager` |
| N13 | status 细节：代理/隧道流量去重、僵尸进程计数、显示偏好持久化（`cmd/status/*`、`prefs.go`） | system-status STS-07 | 已实现(Windows) | 12.5；design D16；网络合计排除回环、隧道/虚拟适配器单列（本机 WSL vEthernet 与第三方隧道驱动单列）；状态页显示偏好跨会话保留。僵尸进程计数仅 macOS，随 12.1 |
| N14 | status 中的吉祥物动画（README“toggle the cat”） | desktop-management GUI-11 | 不采用 | design D14 与 GUI-11：不使用对标产品的形象，使用原创视觉 |
| N15 | 未入首批的 macOS 维护任务：SQLite VACUUM（Mail/Safari/Messages）、阻止网络卷写 `.DS_Store`、清除旧调优覆盖、网络栈/ARP 刷新、用户目录权限修复、Spotlight 重建与孤立规则、periodic 脚本、共享文件列表修复（`lib/optimize/catalog.sh`） | system-maintenance MNT-01 | 待 macOS | design D16 已补记：采用网络卷不写 `.DS_Store` 与指定卷 Spotlight 重建（14.5，需真 Mac）；SQLite VACUUM、旧调优覆盖、ARP、权限修复、periodic、共享文件列表不采用并写明理由 |
| N16 | 清除 Gatekeeper 下载记录、通知数据库、CoreDuet 使用数据（`lib/optimize/catalog.sh`） | system-maintenance | 不采用 | design Non-Goals：不提供隐私痕迹重置 |

## 仍受阻的行（按阻塞门槛）

- **macOS 实现与真 Mac 实测**：#1（3.5/3.8）、#3、#8、#13（6.2）、#15（7.2）、#16（MVP 5.3）、#17（9.1、MVP 11.2）、#19（10.1）、#20（11.1）、#21（14.3）、#25b（13.2、15.3）、#26（15.4）、N5；以及 MVP 5.x/6.x 的 G0 macOS 后端。
- **Windows 真机记录**：#21（真实 UAC 路径）、#23/W16（12.1：OEM 指标可得性）、#8（可移动卷）、#18/W19（真实 MSIX 与供应商卸载器、注册表恢复，18.3）。
- **CI**：#10/W15（2.2、MVP 1.5：工作流已有，但未实际运行）。
- **签名与发布**：#28、W18、N10（MVP 14.1/14.2，没有签名发行包与发布密钥）。
- **Windows 功能缺口**：#1/W7（回收站类目、传递优化）、#4/W5（所属工具执行通道）、#3（AI 工具缓存规则与会话逐条确认入口）、#20（计划任务启停）、W3/W4（更多浏览器与 GPU 厂商）、#25a（15.1/15.2 正在实现）。
- **取舍已补记**（2026-10-16，design D16）：N2、N4、N9、N13 采用并已在 Windows 实现（8.4、9.9、7.4、12.5）；N10 部分采用（nightly 与启动时检查不采用）；N11 不采用；N15 部分采用（14.5 待真 Mac）。
- **门禁验收**：G1 18.1、G2 18.2（及 MVP 13.6 无障碍）、G3 18.3、G4 18.4 都未完成，所以本文中没有一行可以标为“已发布”。

## 对外文案审计（2026-10-15）

口径：README 与 `docs/` 只能宣称已通过门禁的能力；否则必须明确标注“仅 Windows / 实验 / 待验收”。由于没有任何能力通过门禁，凡是说“已实现”的地方都需要同时说明平台和未勾选的任务。下表中的行号指修订前的位置。

| 位置 | 原文要点 | 问题 | 处理 |
| --- | --- | --- | --- |
| `README.md:9` | Rust 核心、CLI 与桌面端“已在 Windows 11 x64 上实现并测试” | 没有说明测试只是单机自动化测试与本机运行记录，也没有说明门禁状态 | 已修订：注明 G0–G4 门禁都未通过、未发布，并链接本文 |
| `README.md:11` | “对标 Mole 全部功能……在 MVP 之后按 G1–G4 门禁实施” | 读起来像规划，但多数 Windows 部分已经实现，进度不清楚 | 已修订：说明 Windows 已实现的部分与仍在等待的门槛，并链接本文 |
| `docs/README.md:52–65`（“当前已经完成与仍待验证”） | “Rust 扫描器、桌面、CLI、授权与清理执行器：尚未实现”“下一步领取 TASK-001” | 已过时（低报，而不是夸大），但它是文档索引的状态表 | 已修订：更新状态行，并在阅读路径中加入本文链接 |
| `docs/startup-items.md:3` | “任务 11.1 已实现只读清单” | 11.1 未勾选，属于夸大 | 已修订：Windows 只读清单已实现；11.1 因缺 macOS 部分未勾选 |
| `docs/app-uninstall.md:3` | “任务 9.1–9.8 的 Windows 部分已实现” | 9.1 未勾选；没有移除过真实 MSIX 包；G3 未通过 | 已修订：注明 9.1 未勾选与 G3 状态 |
| `docs/app-updates.md:3` | “任务 10.1–10.3 的 Windows 部分已实现” | 10.1 未勾选 | 已修订 |
| `docs/manual-trash.md:3` | “任务 6.1–6.5 的 Windows 部分已实现” | 6.2 未勾选 | 已修订 |
| `docs/system-maintenance.md:3` | “对应……任务 14.1/14.2/14.4（Windows）” | 14.2 未勾选，三项任务没有真机执行记录（文末已有说明，但页首没有） | 已修订：在页首注明 |
| `docs/ai-diagnostics.md:3` | “本文记录 OpenSpec 任务 15.1–15.8 的实现” | MVP 15.4 未勾选（CLI 适配的 Win/Mac 模式证据待补） | 已修订 |
| `docs/privilege-boundary.md:3` | “13.3（本文）” | 13.3 未勾选（macOS 部分依赖 13.2） | 已修订：注明 13.3 未勾选 |
| `docs/visual-identity.md:119–122` | 软件“全部可用”，优化/分析/状态“可用” | 指的是 Windows 开发构建中页面已接入，但读起来像能力已发布；更新只有 Windows 通道，计划任务启动项只读 | 已修订：表下加注释，并改写软件行 |
| `docs/distribution-and-licensing.md:42` | “CLI 的签名自更新、存储迁移与自卸载已实现” | 机制已实现，但没有签名发行包与发布密钥 | 已修订 |
| `docs/verification-plan.md:3` | “CleanManager 尚未实现” | 已过时（低报，不是夸大）；文中注明 2026-10-14 是快照日期 | 未改（不是夸大，属于验证计划的历史快照）；建议下次修订该文时更新 |
| `openspec/changes/add-mole-parity-suite/evidence.md:3`、`tasks.md:3` | “尚未实现或测试”“全部任务未执行” | 已过时（低报） | 未改：按约定只追加 evidence.md，不改 tasks.md；evidence.md 末尾加了指向本文的说明 |

`docs/catalog.md`、`docs/system-status.md`、`docs/cli-protocol.md`（逐行标注“已实现 Windows”）、`docs/install.md`（明确写“尚未提供签名的发行包”）与 `docs/safety-and-rules.md`（“已实现的 Windows 行为”）都已经标明平台与未完成项，未改动。`docs/resident-utilities.md` 正由并行工作编写，本次没有审阅或修改。

## 18.5 验收判断

- **“不存在无归属的 Mole 功能”**：满足。矩阵 33 行、W1–W19 与新补登的 N1–N16 都有能力归属和状态；原先只有归属的 N2、N4、N9、N10、N11、N13、N15 已在 design D16 写明采用或不采用的理由（2026-10-16 补记）。
- **“对外文案只宣称已通过门禁的能力”**：经本次修订，README 与 `docs/` 中能找到的说法都已标明“仅 Windows / 未勾选任务 / 门禁未通过”。不过没有任何能力通过门禁，所以严格说没有能力可以“宣称已发布”；`capabilities` 输出也还没有列出对标能力的状态项。
- **补记：未通过门禁即关闭（D15）**：快照之后实现了设计 D15 的“关闭”。每项对标能力在 `cm_core::capability::GATED` 登记门禁与 `gate_passed`（全部为 false）；未通过的能力默认关闭：CLI 命令组与对应种类 `plan/v2` 的审阅/执行返回 `unsupported` 并写明门禁与开启方法，`capabilities` 把它们报告为 `unsupported`（“off until gate Gx passes; experimental features are disabled”），桌面端分区保留原位但只显示说明页，宿主命令同样拒绝。原先桌面工具顶部只标“实验 · Gx 未通过”而功能照常可用，现在这个标记只在用户显式打开“实验功能”（`cleanmanager experimental --state on` 或“设置 › 实验功能”，两端共用、默认关闭）后出现，此时能力仍为 `unverified`，从不为 `available`；关闭开关会停止常驻工具并删除其登录项。上文所说“`capabilities` 没有列出对标能力”与“未通过能力在能力矩阵为不可用”的差距由此补上。
- **结论**：本次复核按 2026-10-15 快照完成，但 18.5 还不应勾选。它的验收前提（每行标为“已发布 / 关闭原因 / 不采用”）要等 18.1–18.4 的门禁结果出来才能成立；目前只能标为“已实现(Windows) / 部分 / 待门槛 / 不采用”。门禁完成后，应以本文为底稿再做一次复核。
