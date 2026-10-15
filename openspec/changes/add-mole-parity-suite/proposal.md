# Proposal

## Why

`define-cleanmanager-mvp` 已把扫描事实、固定计划、授权、执行核对与恢复做成严谨的安全底座，HTML 原型也初步对标了 Mole 的外观，但与 Mole 相比仍缺“实际清什么、还能管什么”：规则目录为空、卸载/维护/状态只是概念、磁盘页不能处理手选对象，Mole CLI 独有的 purge/installer 与 GUI 独有的更新/启动项/托盘工具也没有规格。用户要求以 Mole（开源 CLI 与 Mac/Windows GUI）全部功能为对标基线，并保留我们已有的 Agent CLI、固定计划和 AI 诊断扩展，统一交互与视觉。

## What Changes

- 建立跨平台**清理目录**：Mac/Windows 分组覆盖系统数据、用户必需、应用缓存、浏览器、云与办公、开发工具、AI 工具数据、虚拟化、日志、残留、固件/备份、外接卷元数据等；每项归入“默认可选 / 仅审阅 / 拒绝”三档，附实测应用登记、占用三态探测、阈值与“交由所属工具清理”的固定动作。
- 建立**保护策略**：用户保护名单（任何行可一键加入，CLI/GUI 共享，读取失败即停止写操作）与不可覆盖的平台硬拒绝清单及危险路径模糊语料。
- 新增 Mole CLI 独有能力：**项目构建产物清理**（项目根识别、Git 跟踪文件保护、近 7 天不预选）和**安装包清理**（来源标签、仅包含安装载荷的压缩包）。
- 新增 GUI 能力：**手选移入回收站**（仅回收站模式、系统目录只读、回收站内对象才可另行永久删除、显示占用进程）与**空间洞察**（休眠/分页文件、WSL/虚拟磁盘、设备备份、快照、WinSxS、旧下载等只解释不默认删除）。
- 新增软件管理：**应用卸载**（运行中应用退出、供应商卸载器/包管理器/商店来源、残留分组预览、个人数据默认不选、Windows 注册表项先备份、孤立残留）、**应用更新**（Sparkle/App Store/Homebrew/Electron/winget 等可验证通道，无法验证时打开官方页面）、**启动项与后台项**管理。
- 新增**系统维护**任务目录（逐项 applied/unchanged/skipped/unavailable/attention/failed，管理员任务一次集中授权，不改注册表优化项、不关服务）、**系统状态**（CPU/GPU/内存/磁盘/网络/温度/电池、健康分与诊断、进程用途解释、持续高占用提醒）。
- 新增**常驻工具**：菜单栏/托盘状态面板、保持亮屏、清洁屏幕、摄像头/麦克风隐私提示、蓝牙设备低电量提醒；风扇控制、电池养护、WiFi 低延迟与 sudo Touch ID 等特权项经受限 helper 与硬件能力门禁后开放。
- 新增**产品生命周期**：自更新签名/校验和、自卸载保留用户配置、shell 补全、只读 Doctor 诊断报告、界面本地化。
- 扩展桌面与 CLI：统一“清理/软件/优化/分析/状态”五工具与“计划/活动/恢复/Agent/AI 诊断”扩展的信息架构、三档选择、审阅/结果样式与原创视觉；CLI 增加对应命令组与 `--dry-run` 等价的计划预览。所有新增写动作仍走现有固定计划、授权、现场核验、日志与恢复语义，GUI 与 CLI 共用一套候选（不出现 Mole “CLI 更宽、GUI 更窄”的分叉）。
- 以对标矩阵追踪 Mole 每项功能到本变更的规格或明确的不采用理由，见 [对标证据](evidence.md)。不复制 Mole 品牌、星球素材或 GPL 源码；授权/计费/试用次数不在范围内。

## Capabilities

### New Capabilities

- `cleanup-catalog`：平台分组的清理目录、三档分类、实测应用登记、占用探测、阈值与所属工具委托动作。
- `protection-policy`：用户保护名单、平台硬拒绝清单、失败即停止与危险路径语料。
- `project-artifact-purge`：项目根识别、构建产物候选、Git 跟踪保护、近期项目与可重建性分级。
- `installer-cleanup`：安装包/镜像发现、来源标签、压缩包载荷判定与占用跳过。
- `manual-trash`：从磁盘分析手选对象移入回收站、回收站内永久删除、占用进程提示。
- `disk-insights`：大型系统/虚拟化/备份文件与快照的解释、处理指引与只读洞察。
- `app-uninstall`：卸载来源适配、运行中应用处理、残留发现与分组预览、注册表备份、孤立残留与 AI CLI 卸载。
- `app-updates`：多通道版本检测、可验证更新执行与官方页面回退。
- `startup-items`：登录项、后台项、计划任务/启动项的来源识别、启停、失效项清理与备份。
- `system-maintenance`：平台固定维护任务目录、适用性判定、集中提权与逐项结果。
- `system-status`：实时指标、健康分、诊断提示、进程解释与持续高占用提醒。
- `resident-utilities`：菜单栏/托盘面板及保持亮屏、清洁屏幕、隐私提示、电量提醒、风扇/电池/网络/Touch ID 等常驻或特权工具。
- `product-lifecycle`：自更新与完整性校验、自卸载、shell 补全、Doctor 报告与本地化。

### Modified Capabilities

无：`openspec/specs/` 仍为空。以下为对进行中变更 `define-cleanmanager-mvp` 所引入能力的 **ADDED** 增量（新编号，不改写其既有要求）：

- `desktop-management`：五工具与扩展入口的统一信息架构、三档选择与视觉对齐（GUI-08 起）。
- `agent-cli`：新增命令组、计划预览等价 dry-run 与只读 JSON 状态流（CLI-10 起）。

## Impact

- 依赖 `define-cleanmanager-mvp` 的核心、计划、授权、执行与恢复；本变更须在其后归档。若 MVP 的设计/产品文档中“磁盘页不处理未命中规则对象”“卸载/维护/状态/风扇属后续”“AI 会话一律保护”等表述与本变更冲突，以任务 0 通过 `/opsx:update` 同步修订，而非静默覆盖。
- 新增平台适配：Windows 注册表/卸载器/Store/winget/Scoop/计划任务/Restart Manager/WMI 传感器；macOS LaunchServices/launchd/Service Management/SMJobBless 或 SMAppService helper/IOKit/pmset。特权 helper 是新的签名、安装与威胁边界。
- 规则与实测登记需要大量真机证据，按类目分批进入 enabled；HTML 原型需按统一信息架构更新，现有 “扩展概念” 标记逐项替换为规格对应状态。
- 发布门禁沿用 G0/G1/G2，并为软件管理、维护、状态与常驻工具增设 G3/G4；未通过门禁的能力保持不可用而非降级执行。
