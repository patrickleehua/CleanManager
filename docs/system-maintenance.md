# 系统维护

对应 `add-mole-parity-suite` 的 MNT-01..07、PRT-06 与任务 14.1/14.2/14.4（Windows）。macOS 首批任务（14.3）尚未实现：本构建在 macOS 上目录为空，`optimize plan create` 无可选任务。

维护不是“一键加速”。目录中每项任务是核心内的固定实现，说明解决什么问题、影响什么、是否需要管理员、何时适用；执行前先只读探测，执行后逐项报告真实结果。产品不承诺任何百分比提升。

## 运行方式

```text
cleanmanager optimize list                                   # 目录、只读适用性、用户保护、禁止项
cleanmanager optimize list --search 注册表优化                 # 无对应任务，只返回不提供的原因
cleanmanager optimize plan create --tasks windows.dns-flush,windows.image-health-check
cleanmanager plan review PLAN_ID                             # 终端审阅；非交互调用方永远不能批准
cleanmanager plan apply PLAN_ID                              # 结果为 maintenance_report
cleanmanager optimize diagnose --window 3                    # 只读性能诊断
cleanmanager optimize history                                # 维护运行记录（无撤销）
```

- 一次运行就是一个 `maintenance` 种类的 `plan/v2` 计划（D1/D10）：摘要覆盖任务 ID、版本与是否需管理员；授权只来自对这份计划的交互审阅，预授权策略永远不匹配维护计划。
- `optimize plan create` 不带 `--tasks` 时只选“未受保护且未判定为不适用/无需执行”的任务（批量选择）；点名受保护任务或禁止项返回 `policy_denied`。
- 计划的 `rule_set_digest` 是本构建目录（ID、版本、管理员标记）的摘要。执行时若某个动作的任务 ID/版本/管理员标记与本构建不同，该任务记为 skipped（`task_version_changed`），不会运行“审阅时没见过的代码”。

## 任务目录（Windows 首批）

| 任务 ID@版本 | 解决的问题 | 动作 | 管理员 | 适用性探测（只读） |
| --- | --- | --- | --- | --- |
| `windows.shell-restart@1.0.0` | 开始菜单/搜索打不开、结果陈旧 | 通过 Restart Manager **无强制**关闭本会话中位于 `%windir%\SystemApps` 的 StartMenuExperienceHost/SearchHost（旧版 SearchApp），Windows 在下次使用时重新启动它们；从不 TerminateProcess，Explorer 不在范围内 | 否 | 本会话中正在运行的宿主（名称与映像路径都必须匹配） |
| `windows.icon-cache-rebuild@1.0.0` | 图标/缩略图空白或过期 | 删除 `%LOCALAPPDATA%\Microsoft\Windows\Explorer` 直接子级中的 `iconcache_*.db`/`thumbcache_*.db`：每个文件以独占方式、不跟随链接打开后才标记删除；被 Explorer 占用的文件保留 | 否 | 缓存目录存在且不是链接；缓存文件数与当前占用数 |
| `windows.dns-flush@1.0.0` | 网络/DNS 变更后仍解析到旧地址 | System32 `dnsapi.dll` 的 `DnsFlushResolverCache` | 否 | 接口存在 |
| `windows.trim-when-due@1.0.0` | 固态盘长期未 retrim（计划优化没跑） | 对到期卷运行 System32 `defrag.exe X: /L`（仅 retrim，不整理碎片） | 是 | 固定 NTFS/ReFS 卷、无寻道惩罚且 TRIM 已启用；读取 Defrag 统计 `LastRunTime`，超过 30 天或从未优化为到期，否则 unchanged 并给出上次时间 |
| `windows.image-health-check@1.0.0` | 更新/功能安装失败，怀疑组件存储损坏 | DISM API `DismCheckImageHealth`（不扫描，等价 `/CheckHealth`），只读，从不自动修复 | 是 | DISM API 存在 |

失效启动项按 D10 交给 [启动项](startup-items.md)，不在维护目录中重复。

## 六种结果（MNT-03）

| 结果 | 含义 | 例子 |
| --- | --- | --- |
| applied | 执行了并有改变 | DNS 已刷新；缓存文件已删除；SSD 已 retrim |
| unchanged | 检查过，无需改变 | TRIM 未到期（附上次时间）；组件存储健康 |
| skipped | 未执行 | 不适用、用户保护（`protected`）、拒绝提权（`elevation_declined`）、状态不明（`state_unknown`，从不猜测着执行） |
| unavailable | 平台接口缺失 | 系统没有 DISM API |
| attention | 执行或检查后需要用户跟进 | 缓存文件被占用（注销后再运行）；宿主未同意关闭；组件存储可修复/不可修复（附手动修复途径） |
| failed | 尝试了但没成功 | DNS 客户端拒绝；defrag 退出码非 0 |

报告列出六种计数；存在 failed 或 attention 时 `all_clear` 为 false，界面不得显示“全部完成”。`plan apply` 的退出码：全部 applied/unchanged 为 0，有 skipped/unavailable/attention 为 5（partial），全部 failed 为 8。

## 集中提权（MNT-04）

需要管理员的任务在**任何任务运行之前**集中列出，并只请求一次提权：

- 进程已是管理员：在进程内执行（`elevation: already_elevated`）。
- 否则通过 UAC 启动一个短生命周期工作进程，只接受本计划摘要内这些任务 ID（`granted`）；细节与威胁模型见 [提权边界](privilege-boundary.md)。
- 用户拒绝 UAC（`declined`）或工作进程无法启动/校验（`unavailable`）：这些任务全部 skipped，用户级任务照常执行。

## 用户保护（PRT-06）

`cleanmanager protect add --task windows.icon-cache-rebuild` 把单个任务加入保护：目录中显示 `user_protected: true`，批量计划不再选它，已存计划执行时该任务记为 skipped（`protected`），其余任务照常。保护名单不可用时按失败关闭处理（任务 skipped）。

## 禁止项（MNT-05）

以下操作不是任务，也永远不会成为任务；`optimize list` 返回 `prohibited` 清单，搜索命中时只给出原因：

| ID | 不提供的操作 | 原因（摘要） |
| --- | --- | --- |
| `registry-optimizer` | 注册表清理/“优化” | 无可测收益，可能破坏应用或系统 |
| `performance-privacy-tweaks` | 注册表性能/隐私调优 | 效果不可证、难以撤销；应在系统设置中修改 |
| `service-disable` | 禁用或删除系统服务 | 更新、安全与功能会在之后出问题 |
| `search-indexing-disable` | 关闭 Windows 搜索索引 | 搜索、Outlook、设置依赖索引；在设置中管理索引位置 |
| `system-debloat` | 系统精简/组件移除 | 破坏服务与更新；卸载自己装的应用请用应用页 |
| `memory-release` | 内存“释放”/加速 | Windows 自行管理内存；诊断会显示真实内存压力 |
| `pagefile-tweak` | 页面文件/虚拟内存调整 | 尺寸不当导致崩溃与转储丢失 |
| `rebuild-app-registrations` | 重新注册全部应用/重建应用注册数据库 | 可能使应用不可用且不可撤销 |
| `ssd-full-defrag` | 固态盘完整碎片整理 | SSD 只需 retrim（TRIM 任务到期时执行） |
| `one-click-speedup` | 一键加速 | 没有诚实的百分比可以承诺 |

## 只读性能诊断（MNT-06）

`optimize diagnose` 以 `--window`（默认 2 s，1–30 s）间隔两次读取每个进程的 CPU 时间（只以 `PROCESS_QUERY_LIMITED_INFORMATION` 打开），并读取内存负载、提交量、固定卷可用空间、启动项数量与已附加的磁盘映像。发现按优先级列出：

- `runaway_process`：窗口内占用 ≥ 80% 单核，且启动以来平均 ≥ 50% 单核、运行 ≥ 10 分钟；`busy_process`：仅窗口内繁忙。列出进程名、PID、映像路径（所属应用）与持续时间，下一步为状态页的进程视图；**从不结束进程**。
- `memory_pressure`（物理内存 ≥ 85%）、`commit_pressure`（提交量 ≥ 90%）、`low_disk_space`（< 10% 或 < 10 GiB，下一步为磁盘分析）。
- `idle_virtual_machine`（虚拟机宿主进程空闲）、`attached_disk_image`（仍附加的磁盘映像，在资源管理器中推出）、`many_startup_items`（≥ 15 个已启用启动项，下一步 `startup list`）。

阈值是本产品可调设计值。诊断不更改任何设置，记录中 `read_only` 恒为 true。

## 历史（MNT-07）

每次运行写入一条 `maintenance_report`：计划与摘要、入口（cli/desktop）、执行者、提权情况、每项任务的版本、结果、代码、原因、是否在提权上下文执行以及起止时间。`optimize history` 按时间倒序列出；`operations list` 中该操作的 `action` 为 `maintenance`、恢复可用性为 `unavailable`。维护动作没有通用撤销；对可逆设置变更（`settings_change`）的恢复入口属后续任务，本构建不执行 `settings_change` 动作（记为 skipped `unsupported`）。

## 实测记录

Windows 11 企业版 10.0.26200 x64 上的真实记录见 [维护实测汇总](evidence/maintenance-windows.json) 与 `docs/evidence/runs/run_win_maint_*.json`：

- `windows.dns-flush`：applied（`flushed`）。
- `windows.image-health-check`：unchanged（`healthy`），与 `Dism.exe /Online /Cleanup-Image /CheckHealth` 的输出一致。
- `windows.trim-when-due`、`windows.icon-cache-rebuild`、`windows.shell-restart`：只记录了只读探测（TRIM 到期：卷从未优化；缓存 30 个文件、11 个占用；两个宿主在运行）。这三项动作未在真实用户配置上执行，真机 applied/attention 记录待人工补充。
- 自动化测试只在临时目录上执行缓存删除，任务一律脚本化，从不重启真实的 Shell 宿主、不刷新真实缓存、不触发 UAC。
